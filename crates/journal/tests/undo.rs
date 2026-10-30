//! Journal and undo against the simulated machine.

use std::path::PathBuf;

use sancho_core::plan::PlannedEffect;
use sancho_core::{EffectKind, Plan, PlanStep, Reversibility, Risk, Tier};
use sancho_exec::{MockExecutor, MockState};
use sancho_journal::state::State;
use sancho_journal::{
    apply_undo, execute, last_undoable, plan_undo, Journal, RunOutcome, StepStatus, TxStatus,
};

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
        .join("journal")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    Journal::open(dir)
}

fn step(argv: &[&str], effects: &[(EffectKind, &str)]) -> PlanStep {
    PlanStep {
        primitive: "test".into(),
        argv: argv.iter().map(|s| s.to_string()).collect(),
        stdin: None,
        needs_root: true,
        interactive: false,
        read_only: effects.is_empty(),
        risk: Risk::Medium,
        explanation: argv.join(" "),
        effects: effects
            .iter()
            .map(|(kind, target)| PlannedEffect {
                kind: *kind,
                target: target.to_string(),
            })
            .collect(),
        reversibility: Reversibility::Reversible,
        learned: false,
    }
}

fn plan(steps: Vec<PlanStep>) -> Plan {
    Plan {
        tier: Tier::VerifiedRecipe,
        recipe_id: Some("test.recipe".into()),
        recipe_version: Some(1),
        summary: "test".into(),
        risk: Risk::Medium,
        steps,
        skipped: vec![],
        notes: vec![],
    }
}

fn enable_and_start(unit: &str) -> Plan {
    plan(vec![
        step(
            &["systemctl", "enable", unit],
            &[(EffectKind::ServiceEnablement, unit)],
        ),
        step(
            &["systemctl", "start", unit],
            &[(EffectKind::ServiceRuntime, unit)],
        ),
    ])
}

fn with_unit(unit: &str, enabled: &str, active: bool) -> MockState {
    let mut s = MockState::default();
    s.units.insert(unit.into(), (enabled.into(), active));
    s
}

fn completed(o: RunOutcome) -> String {
    match o {
        RunOutcome::Completed { tx } => tx,
        other => panic!("expected completion, got {other:?}"),
    }
}

/// Undo everything in `tx` and return the undo transaction's ID.
fn undo(j: &Journal, ex: &mut MockExecutor, tx: &str) -> String {
    let p = plan_undo(j, ex, &[tx.to_string()], None).unwrap();
    completed(apply_undo(j, ex, &p, "undo", false).unwrap())
}

#[test]
fn undo_of_enable_on_an_already_enabled_service_changes_nothing() {
    let j = journal("pre_enabled");
    let mut ex = MockExecutor::new(with_unit("sshd.service", "enabled", true));
    let tx = completed(
        run(
            &j,
            &mut ex,
            &enable_and_start("sshd.service"),
            "enable sshd",
        )
        .unwrap(),
    );
    let before_undo = ex.log.len();
    undo(&j, &mut ex, &tx);
    assert_eq!(ex.state.units["sshd.service"], ("enabled".into(), true));
    let undo_cmds: Vec<String> = ex.commands()[before_undo..].to_vec();
    assert!(
        !undo_cmds
            .iter()
            .any(|c| c.contains("disable") || c.contains(" stop ")),
        "a guessed inverse ran: {undo_cmds:?}"
    );
}

#[test]
fn undo_of_enable_restores_a_disabled_stopped_service() {
    let j = journal("disabled");
    let mut ex = MockExecutor::new(with_unit("sshd.service", "disabled", false));
    let tx = completed(
        run(
            &j,
            &mut ex,
            &enable_and_start("sshd.service"),
            "enable sshd",
        )
        .unwrap(),
    );
    assert_eq!(ex.state.units["sshd.service"], ("enabled".into(), true));
    undo(&j, &mut ex, &tx);
    assert_eq!(ex.state.units["sshd.service"], ("disabled".into(), false));
}

#[test]
fn undo_of_undo_redoes_the_change() {
    let j = journal("redo");
    let mut ex = MockExecutor::new(with_unit("sshd.service", "disabled", false));
    let tx = completed(
        run(
            &j,
            &mut ex,
            &enable_and_start("sshd.service"),
            "enable sshd",
        )
        .unwrap(),
    );
    let u = undo(&j, &mut ex, &tx);
    assert_eq!(ex.state.units["sshd.service"], ("disabled".into(), false));
    undo(&j, &mut ex, &u);
    assert_eq!(ex.state.units["sshd.service"], ("enabled".into(), true));
    // The original is marked undone; the undo is marked undone by the redo.
    assert!(j.load(&tx).unwrap().fully_undone());
    assert!(j.load(&u).unwrap().fully_undone());
}

fn write_file(path: &str, content: &str) -> Plan {
    let mut s = step(
        &["install", "-m", "0644", "/dev/stdin", path],
        &[(EffectKind::FileWrite, path)],
    );
    s.stdin = Some(content.into());
    plan(vec![s])
}

#[test]
fn undo_of_a_new_file_deletes_it_and_of_an_overwrite_restores_the_backup() {
    let j = journal("files");
    let mut ex = MockExecutor::default();
    let new = completed(run(&j, &mut ex, &write_file("/etc/a.rules", "A"), "write a").unwrap());
    undo(&j, &mut ex, &new);
    assert!(!ex.state.files.contains_key("/etc/a.rules"));

    ex.state
        .files
        .insert("/etc/b.conf".into(), ("old".into(), "0640".into()));
    let over = completed(run(&j, &mut ex, &write_file("/etc/b.conf", "new"), "write b").unwrap());
    assert_eq!(ex.state.files["/etc/b.conf"].0, "new");
    // The backup is root-owned under /var/lib/sancho, not in the journal.
    let manifest = j.load(&over).unwrap();
    let Some(State::File {
        backup: Some(b), ..
    }) = &manifest.steps[0].effects[0].before
    else {
        panic!("no backup recorded")
    };
    assert!(b.starts_with("/var/lib/sancho/backups/"), "{b}");
    undo(&j, &mut ex, &over);
    assert_eq!(ex.state.files["/etc/b.conf"], ("old".into(), "0640".into()));
}

#[test]
fn file_drift_is_detected_and_can_be_left_alone() {
    let j = journal("drift");
    let mut ex = MockExecutor::default();
    let tx = completed(run(&j, &mut ex, &write_file("/etc/a.rules", "A"), "write").unwrap());
    // Someone edits the file after sancho wrote it.
    ex.state.files.get_mut("/etc/a.rules").unwrap().0 = "edited by hand".into();
    let p = plan_undo(&j, &mut ex, std::slice::from_ref(&tx), None).unwrap();
    assert!(p.has_drift());
    let d = p.actions[0].drift.as_ref().unwrap();
    assert_ne!(d.expected, d.found);
    // Skipping drifted resources leaves the hand edit in place.
    completed(apply_undo(&j, &mut ex, &p, "undo", true).unwrap());
    assert_eq!(ex.state.files["/etc/a.rules"].0, "edited by hand");
}

#[test]
fn later_transactions_on_the_same_resource_are_dependents() {
    let j = journal("deps");
    let mut ex = MockExecutor::new(with_unit("sshd.service", "disabled", false));
    let t1 = completed(
        run(
            &j,
            &mut ex,
            &enable_and_start("sshd.service"),
            "enable sshd",
        )
        .unwrap(),
    );
    let disable = plan(vec![step(
        &["systemctl", "disable", "sshd.service"],
        &[(EffectKind::ServiceEnablement, "sshd.service")],
    )]);
    let t2 = completed(run(&j, &mut ex, &disable, "disable sshd").unwrap());
    let unrelated = completed(run(&j, &mut ex, &write_file("/etc/x", "x"), "write x").unwrap());

    let p = plan_undo(&j, &mut ex, std::slice::from_ref(&t1), None).unwrap();
    assert_eq!(
        p.dependents,
        std::slice::from_ref(&t2),
        "the file write is unrelated: {unrelated}"
    );
    // Undoing both together has no outside dependents.
    let both = plan_undo(&j, &mut ex, &[t1, t2], None).unwrap();
    assert!(both.dependents.is_empty());
}

#[test]
fn a_failed_step_stops_the_run_and_completed_steps_can_be_rolled_back() {
    let j = journal("rollback");
    let mut ex = MockExecutor::new(with_unit("sshd.service", "disabled", false));
    ex.fail_on.push("systemctl start".into());
    let out = run(
        &j,
        &mut ex,
        &enable_and_start("sshd.service"),
        "enable sshd",
    )
    .unwrap();
    let RunOutcome::Failed {
        tx,
        step: 1,
        exit: 1,
    } = out
    else {
        panic!("expected step 2 to fail, got {out:?}")
    };
    let m = j.load(&tx).unwrap();
    assert_eq!(m.status, TxStatus::Failed { step: 1 });
    assert_eq!(m.steps[0].status, StepStatus::Done);
    assert_eq!(m.steps[1].status, StepStatus::Failed { exit: 1 });
    assert_eq!(ex.state.units["sshd.service"].0, "enabled");

    // Rollback = undo of what completed.
    ex.fail_on.clear();
    let p = plan_undo(&j, &mut ex, std::slice::from_ref(&tx), None).unwrap();
    assert_eq!(p.actions.len(), 1, "only the completed step is undone");
    completed(apply_undo(&j, &mut ex, &p, "rollback", false).unwrap());
    assert_eq!(ex.state.units["sshd.service"], ("disabled".into(), false));
}

fn package_state() -> MockState {
    let mut s = MockState::default();
    s.installed.insert("vlc".into(), "3.0.21-8".into());
    s.cache.insert("vlc-3.0.21-8-x86_64.pkg.tar.zst".into());
    s.repo
        .insert("gimp".into(), ("2.10.38-4".into(), vec!["babl".into()]));
    s.repo.insert("babl".into(), ("0.1.110-1".into(), vec![]));
    s
}

#[test]
fn undo_of_install_removes_the_package_and_its_new_dependencies() {
    let j = journal("pkg_install");
    let mut ex = MockExecutor::new(package_state());
    let install = plan(vec![step(
        &["pacman", "-S", "--needed", "gimp"],
        &[(EffectKind::PackageInstall, "gimp")],
    )]);
    let tx = completed(run(&j, &mut ex, &install, "install gimp").unwrap());
    assert!(ex.state.installed.contains_key("babl"));
    let p = plan_undo(&j, &mut ex, std::slice::from_ref(&tx), None).unwrap();
    // The preview names the dependency too: no silent scope expansion.
    assert!(
        p.actions[0].restores[0].explanation.contains("babl, gimp"),
        "{:?}",
        p.actions
    );
    assert!(
        p.actions[0].restores[0].invocation.interactive,
        "pacman asks the user"
    );
    completed(apply_undo(&j, &mut ex, &p, "undo", false).unwrap());
    assert_eq!(ex.state.installed.keys().collect::<Vec<_>>(), ["vlc"]);
}

#[test]
fn undo_of_remove_reinstalls_from_the_cache_or_says_why_it_cannot() {
    let j = journal("pkg_remove");
    let mut ex = MockExecutor::new(package_state());
    let remove = plan(vec![step(
        &["pacman", "-R", "vlc"],
        &[(EffectKind::PackageRemove, "vlc")],
    )]);
    let tx = completed(run(&j, &mut ex, &remove, "remove vlc").unwrap());
    assert!(!ex.state.installed.contains_key("vlc"));

    // With the cache cleaned, it can't be undone, and the preview says why.
    let cache = std::mem::take(&mut ex.state.cache);
    let p = plan_undo(&j, &mut ex, std::slice::from_ref(&tx), None).unwrap();
    assert!(p.actions.is_empty());
    assert!(
        p.cannot[0].contains("no longer in the package cache"),
        "{:?}",
        p.cannot
    );

    ex.state.cache = cache;
    undo(&j, &mut ex, &tx);
    assert_eq!(ex.state.installed["vlc"], "3.0.21-8");
}

#[test]
fn undo_of_group_add_only_removes_groups_this_change_added() {
    let j = journal("groups");
    let mut s = MockState::default();
    s.groups
        .insert("alice".into(), ["wheel".to_string()].into());
    let mut ex = MockExecutor::new(s);
    let add = |g: &str| {
        plan(vec![step(
            &["usermod", "-a", "-G", g, "alice"],
            &[(EffectKind::GroupMembership, "alice")],
        )])
    };
    let video = completed(run(&j, &mut ex, &add("video"), "add me to video").unwrap());
    // Already a member: undo must not remove alice from wheel.
    let wheel = completed(run(&j, &mut ex, &add("wheel"), "add me to wheel").unwrap());
    undo(&j, &mut ex, &wheel);
    assert!(ex.state.groups["alice"].contains("wheel"));
    undo(&j, &mut ex, &video);
    assert!(!ex.state.groups["alice"].contains("video"));
}

#[test]
fn irreversible_steps_are_reported_and_runtime_steps_need_nothing() {
    let j = journal("irreversible");
    let mut ex = MockExecutor::new(with_unit("sshd.service", "enabled", true));
    let p = plan(vec![
        step(
            &["paccache", "-r", "-k", "2"],
            &[(EffectKind::PackageCachePrune, "/var/cache/pacman/pkg")],
        ),
        step(
            &["systemctl", "restart", "sshd.service"],
            &[(EffectKind::Runtime, "sshd.service")],
        ),
    ]);
    let tx = completed(run(&j, &mut ex, &p, "clean and restart").unwrap());
    let u = plan_undo(&j, &mut ex, &[tx], None).unwrap();
    assert!(u.actions.is_empty());
    assert_eq!(u.cannot.len(), 1);
    assert!(u.cannot[0].contains("deleted cache files are gone"));
}

#[test]
fn partial_undo_only_touches_the_named_steps() {
    let j = journal("partial");
    let mut ex = MockExecutor::new(with_unit("sshd.service", "disabled", false));
    let tx = completed(
        run(
            &j,
            &mut ex,
            &enable_and_start("sshd.service"),
            "enable sshd",
        )
        .unwrap(),
    );
    // Keep it enabled at boot, but undo starting it now.
    let p = plan_undo(&j, &mut ex, std::slice::from_ref(&tx), Some(&[1])).unwrap();
    let u = completed(apply_undo(&j, &mut ex, &p, "undo step 2", false).unwrap());
    assert_eq!(ex.state.units["sshd.service"], ("enabled".into(), false));
    let m = j.load(&tx).unwrap();
    assert!(m.steps[0].undone_by.is_none() && m.steps[1].undone_by.is_some());
    assert!(!m.fully_undone(), "step 1 is still live");
    // The newest undoable change is the undo itself: `sancho undo` again redoes.
    assert_eq!(last_undoable(&j).unwrap().unwrap().id, u);
}

#[test]
fn the_journal_is_written_after_every_step() {
    let j = journal("consistency");
    let mut ex = MockExecutor::new(with_unit("sshd.service", "disabled", false));
    ex.fail_on.push("systemctl start".into());
    let RunOutcome::Failed { tx, .. } =
        run(&j, &mut ex, &enable_and_start("sshd.service"), "x").unwrap()
    else {
        panic!()
    };
    let m = j.load(&tx).unwrap();
    let e = &m.steps[0].effects[0];
    assert_eq!(
        e.before,
        Some(State::Enablement {
            value: "disabled".into()
        })
    );
    assert_eq!(
        e.after,
        Some(State::Enablement {
            value: "enabled".into()
        })
    );
    // No half-written manifests are left behind.
    let leftovers: Vec<_> = std::fs::read_dir(j.dir.join(&tx))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name() != "manifest.json")
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
