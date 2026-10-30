//! The real executor, journal and undo against systemd and coreutils in an
//! `archlinux:latest` podman container. Nothing runs on the host except
//! `podman`. Run with: cargo test -p container-tests -- --ignored

use std::path::PathBuf;
use std::process::Command;

use sancho_core::plan::PlannedEffect;
use sancho_core::{EffectKind, Plan, PlanStep, Reversibility, Risk, Tier};
use sancho_exec::{ContainerExecutor, Executor, Invocation};
use sancho_journal::{apply_undo, execute, plan_undo, Journal, RunOutcome, TxStatus};

const IMAGE: &str = "docker.io/library/archlinux:latest";

/// A running container with systemd as PID 1, removed on drop.
struct Container {
    name: String,
}

impl Container {
    fn start(test: &str) -> Container {
        let name = format!("sancho-test-{test}-{}", std::process::id());
        let _ = Command::new("podman").args(["rm", "-f", &name]).output();
        let ok = Command::new("podman")
            // --privileged lets dbus-broker start, so unprivileged systemctl
            // queries work as on a real host. Under rootless podman this stays
            // inside the user's namespace: no privileges on the host.
            .args([
                "run",
                "-d",
                "--rm",
                "--privileged",
                "--systemd=always",
                "--name",
                &name,
                IMAGE,
                "/usr/lib/systemd/systemd",
            ])
            .output()
            .expect("podman is installed")
            .status
            .success();
        assert!(ok, "could not start {IMAGE}");
        let c = Container { name };
        // Wait until systemd accepts commands.
        for _ in 0..200 {
            let out = c.sh("systemctl is-system-running || true");
            if out.contains("running") || out.contains("degraded") {
                break;
            }
        }
        c.sh(
            "useradd -m tester && \
             printf '[Unit]\\nDescription=test\\n[Service]\\nExecStart=/usr/bin/sleep infinity\\n[Install]\\nWantedBy=multi-user.target\\n' \
               > /etc/systemd/system/sancho-test.service && \
             printf '[Service]\\nType=oneshot\\nExecStart=/usr/bin/false\\n' > /etc/systemd/system/sancho-fail.service && \
             systemctl daemon-reload",
        );
        c
    }

    /// Test setup and inspection inside the container (not through sancho).
    fn sh(&self, script: &str) -> String {
        let out = Command::new("podman")
            .args(["exec", &self.name, "sh", "-c", script])
            .output()
            .expect("podman exec");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn executor(&self) -> ContainerExecutor {
        ContainerExecutor {
            container: self.name.clone(),
            user: "tester".into(),
        }
    }

    fn unit(&self, unit: &str) -> (String, String) {
        (
            self.sh(&format!("systemctl is-enabled {unit} || true")),
            self.sh(&format!("systemctl is-active {unit} || true")),
        )
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new("podman")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

/// Runs a plan without progress output.
fn run(
    j: &Journal,
    ex: &mut dyn sancho_exec::Executor,
    plan: &Plan,
    request: &str,
) -> Result<RunOutcome, sancho_journal::JournalError> {
    execute(j, ex, plan, request, &mut |_| {})
}

fn journal(name: &str) -> Journal {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("container-journal")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    Journal::open(dir)
}

fn step(argv: &[&str], kind: EffectKind, target: &str) -> PlanStep {
    PlanStep {
        primitive: "test".into(),
        argv: argv.iter().map(|s| s.to_string()).collect(),
        stdin: None,
        needs_root: true,
        interactive: false,
        read_only: false,
        risk: Risk::Medium,
        explanation: argv.join(" "),
        effects: vec![PlannedEffect {
            kind,
            target: target.into(),
        }],
        reversibility: Reversibility::Reversible,
        learned: false,
    }
}

fn plan(steps: Vec<PlanStep>) -> Plan {
    Plan {
        tier: Tier::VerifiedRecipe,
        recipe_id: Some("test".into()),
        recipe_version: Some(1),
        summary: "test".into(),
        risk: Risk::Medium,
        steps,
        skipped: vec![],
        notes: vec![],
        learned: vec![],
    }
}

fn enable_and_start(unit: &str) -> Plan {
    plan(vec![
        step(
            &["systemctl", "enable", unit],
            EffectKind::ServiceEnablement,
            unit,
        ),
        step(
            &["systemctl", "start", unit],
            EffectKind::ServiceRuntime,
            unit,
        ),
    ])
}

fn completed(o: RunOutcome) -> String {
    match o {
        RunOutcome::Completed { tx } => tx,
        other => panic!("expected completion, got {other:?}"),
    }
}

fn undo(j: &Journal, ex: &mut dyn Executor, tx: &str) -> String {
    let p = plan_undo(j, ex, &[tx.to_string()], None).unwrap();
    completed(apply_undo(j, ex, &p, "undo", false).unwrap())
}

#[test]
#[ignore = "needs podman"]
fn undo_keeps_a_service_that_was_already_enabled() {
    let c = Container::start("pre-enabled");
    c.sh("systemctl enable --now sancho-test.service");
    let j = journal("pre-enabled");
    let mut ex = c.executor();
    let tx = completed(
        run(
            &j,
            &mut ex,
            &enable_and_start("sancho-test.service"),
            "enable",
        )
        .unwrap(),
    );
    undo(&j, &mut ex, &tx);
    assert_eq!(
        c.unit("sancho-test.service"),
        ("enabled".into(), "active".into())
    );
}

#[test]
#[ignore = "needs podman"]
fn undo_restores_a_disabled_service_and_undo_of_undo_redoes() {
    let c = Container::start("redo");
    let j = journal("redo");
    let mut ex = c.executor();
    let tx = completed(
        run(
            &j,
            &mut ex,
            &enable_and_start("sancho-test.service"),
            "enable",
        )
        .unwrap(),
    );
    assert_eq!(
        c.unit("sancho-test.service"),
        ("enabled".into(), "active".into())
    );
    let u = undo(&j, &mut ex, &tx);
    assert_eq!(
        c.unit("sancho-test.service"),
        ("disabled".into(), "inactive".into())
    );
    undo(&j, &mut ex, &u);
    assert_eq!(
        c.unit("sancho-test.service"),
        ("enabled".into(), "active".into())
    );
}

#[test]
#[ignore = "needs podman"]
fn file_writes_are_backed_up_restored_and_drift_is_detected() {
    let c = Container::start("files");
    c.sh("printf 'original\\n' > /etc/sancho-test.conf && chmod 640 /etc/sancho-test.conf");
    let j = journal("files");
    let mut ex = c.executor();
    let mut write = step(
        &[
            "install",
            "-m",
            "0644",
            "/dev/stdin",
            "/etc/sancho-test.conf",
        ],
        EffectKind::FileWrite,
        "/etc/sancho-test.conf",
    );
    write.stdin = Some("written by sancho\n".into());
    let tx = completed(run(&j, &mut ex, &plan(vec![write.clone()]), "write").unwrap());
    assert_eq!(c.sh("cat /etc/sancho-test.conf"), "written by sancho");
    // The backup is root-only.
    assert_eq!(c.sh("stat -c %a /var/lib/sancho/backups/*/"), "700");

    // Drift: edited after sancho wrote it.
    c.sh("echo 'hand edit' >> /etc/sancho-test.conf");
    let p = plan_undo(&j, &mut ex, std::slice::from_ref(&tx), None).unwrap();
    assert!(p.has_drift(), "{p:?}");
    // Overwriting the drift restores the original content and mode.
    completed(apply_undo(&j, &mut ex, &p, "undo", false).unwrap());
    assert_eq!(c.sh("cat /etc/sancho-test.conf"), "original");
    assert_eq!(c.sh("stat -c %a /etc/sancho-test.conf"), "640");

    // A file that didn't exist is removed again on undo.
    let mut new = write;
    new.argv[4] = "/etc/sancho-new.conf".into();
    new.effects[0].target = "/etc/sancho-new.conf".into();
    let tx = completed(run(&j, &mut ex, &plan(vec![new]), "write new").unwrap());
    undo(&j, &mut ex, &tx);
    assert_eq!(
        c.sh("test -e /etc/sancho-new.conf && echo yes || echo no"),
        "no"
    );
}

#[test]
#[ignore = "needs podman"]
fn a_later_change_to_the_same_service_is_reported_as_dependent() {
    let c = Container::start("deps");
    let j = journal("deps");
    let mut ex = c.executor();
    let t1 = completed(
        run(
            &j,
            &mut ex,
            &enable_and_start("sancho-test.service"),
            "enable",
        )
        .unwrap(),
    );
    let disable = plan(vec![step(
        &["systemctl", "disable", "sancho-test.service"],
        EffectKind::ServiceEnablement,
        "sancho-test.service",
    )]);
    let t2 = completed(run(&j, &mut ex, &disable, "disable").unwrap());
    let p = plan_undo(&j, &mut ex, std::slice::from_ref(&t1), None).unwrap();
    assert_eq!(p.dependents, [t2]);
}

#[test]
#[ignore = "needs podman"]
fn a_failed_step_stops_the_run_and_rollback_restores_completed_steps() {
    let c = Container::start("rollback");
    let j = journal("rollback");
    let mut ex = c.executor();
    let p = plan(vec![
        step(
            &["systemctl", "enable", "sancho-test.service"],
            EffectKind::ServiceEnablement,
            "sancho-test.service",
        ),
        step(
            &["systemctl", "start", "sancho-fail.service"],
            EffectKind::ServiceRuntime,
            "sancho-fail.service",
        ),
        step(
            &["systemctl", "start", "sancho-test.service"],
            EffectKind::ServiceRuntime,
            "sancho-test.service",
        ),
    ]);
    let out = run(&j, &mut ex, &p, "enable and start").unwrap();
    let RunOutcome::Failed { tx, step: 1, .. } = out else {
        panic!("expected step 2 to fail, got {out:?}")
    };
    assert_eq!(j.load(&tx).unwrap().status, TxStatus::Failed { step: 1 });
    assert_eq!(
        c.unit("sancho-test.service"),
        ("enabled".into(), "inactive".into()),
        "step 3 never ran"
    );
    undo(&j, &mut ex, &tx);
    assert_eq!(c.unit("sancho-test.service").0, "disabled");
}

#[test]
#[ignore = "needs podman"]
fn group_membership_is_restored_exactly() {
    let c = Container::start("groups");
    let j = journal("groups");
    let mut ex = c.executor();
    let add = plan(vec![step(
        &["usermod", "-a", "-G", "video", "tester"],
        EffectKind::GroupMembership,
        "tester",
    )]);
    let tx = completed(run(&j, &mut ex, &add, "add tester to video").unwrap());
    assert!(c.sh("id -nG tester").contains("video"));
    undo(&j, &mut ex, &tx);
    assert!(!c.sh("id -nG tester").contains("video"));
    // Unprivileged queries ran as the unprivileged user.
    let who = ex.run(&Invocation::query(&["id", "-un"], false)).unwrap();
    assert_eq!(who.stdout.trim(), "tester");
}
