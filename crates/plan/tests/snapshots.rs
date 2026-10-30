//! Rendered plans for every starter recipe, compared with golden files in
//! `tests/snapshots/`. Run with `UPDATE_SNAPSHOTS=1` to rewrite them, then
//! review the diff.

use std::path::{Path, PathBuf};

use sancho_core::plan::Confirmation;
use sancho_core::{format_errors, FixtureSystem, Library, Reversibility};
use sancho_match::{interpret, Interpretation, Matcher};
use sancho_plan::{build, render, PlanError};
use sancho_profile::detect;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn library() -> Library {
    Library::load(&root().join("data")).unwrap_or_else(|e| panic!("{}", format_errors(&e)))
}

fn system(name: &str) -> FixtureSystem {
    FixtureSystem::new(root().join("eval/fixtures/systems").join(name))
}

/// Request → plan on a fixture system.
fn plan_for(
    lib: &Library,
    system_name: &str,
    request: &str,
) -> Result<sancho_core::Plan, PlanError> {
    let sys = system(system_name);
    let profile = detect(&sys);
    let matcher = Matcher::new(lib);
    match interpret(&matcher, &profile, &sys, request) {
        Interpretation::Ready { recipe, values, .. } => {
            build(lib, &recipe, &values, &profile, &sys)
        }
        other => panic!("{request:?} on {system_name}: expected a plan, got {other:?}"),
    }
}

const CASES: &[(&str, &str, &str)] = &[
    (
        "system_update_snapper_paru",
        "eval_desktop",
        "update my system",
    ),
    ("system_update_no_snapshot", "arch_iwd", "update my system"),
    (
        "package_install",
        "eval_desktop",
        "install gimp and obs-studio",
    ),
    ("package_remove", "eval_desktop", "remove vlc"),
    ("service_restart", "eval_desktop", "restart sshd"),
    ("service_start", "eval_desktop", "start nginx"),
    ("service_stop", "eval_desktop", "stop bluetooth"),
    ("service_enable", "eval_desktop", "enable sshd"),
    ("service_disable", "eval_desktop", "disable tlp"),
    (
        "restart_wifi_networkmanager",
        "eval_desktop",
        "restart my wifi",
    ),
    ("restart_wifi_iwd", "arch_iwd", "restart my wifi"),
    (
        "grant_hid_browser_access",
        "eval_desktop",
        "let chromium configure my keyboard",
    ),
    ("add_to_group", "eval_desktop", "add me to the video group"),
    ("disk_usage", "eval_desktop", "show disk usage"),
];

#[test]
fn rendered_plans_match_snapshots() {
    let lib = library();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    let mut failures = Vec::new();
    for (name, sys, request) in CASES {
        let plan = plan_for(&lib, sys, request).unwrap_or_else(|e| panic!("{name}: {e}"));
        let text = format!(
            "$ sancho {request}   # system: {sys}\n\n{}",
            render::plan(&plan)
        );
        let file = dir.join(format!("{name}.txt"));
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&file, &text).unwrap();
            continue;
        }
        match std::fs::read_to_string(&file) {
            Ok(expected) if expected == text => {}
            Ok(expected) => failures.push(format!(
                "{name}: differs\n--- expected\n{expected}\n--- got\n{text}"
            )),
            Err(_) => failures.push(format!(
                "{name}: missing {}; run with UPDATE_SNAPSHOTS=1",
                file.display()
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn every_starter_recipe_has_a_snapshot() {
    let lib = library();
    let covered: std::collections::BTreeSet<String> = CASES
        .iter()
        .map(|(_, sys, req)| plan_for(&lib, sys, req).unwrap().recipe_id.unwrap())
        .collect();
    // Answer recipes don't plan anything; sancho-facts tests cover them.
    let all: std::collections::BTreeSet<String> = lib
        .recipes
        .iter()
        .filter(|(_, r)| r.answer.is_none())
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(covered, all);
}

#[test]
fn upgrade_without_snapshot_cannot_be_undone() {
    let lib = library();
    let with = plan_for(&lib, "eval_desktop", "update my system").unwrap();
    assert_eq!(
        with.reversibility(),
        Reversibility::Irreversible,
        "paccache prune is irreversible"
    );
    assert!(with
        .steps
        .iter()
        .any(|s| s.reversibility == Reversibility::SnapshotOnly));
    let without = plan_for(&lib, "arch_iwd", "update my system").unwrap();
    let upgrade = without
        .steps
        .iter()
        .find(|s| s.primitive == "pacman.sync_upgrade")
        .unwrap();
    assert_eq!(upgrade.reversibility, Reversibility::Irreversible);
}

#[test]
fn confirmation_follows_risk_and_read_only() {
    let lib = library();
    let c = |sys, req| plan_for(&lib, sys, req).unwrap().confirmation();
    assert_eq!(c("eval_desktop", "show disk usage"), Confirmation::None);
    assert_eq!(c("eval_desktop", "restart sshd"), Confirmation::YesNo);
    assert_eq!(c("eval_desktop", "update my system"), Confirmation::TypeYes);
}

#[test]
fn skipped_steps_are_listed_with_reasons() {
    let lib = library();
    let p = plan_for(&lib, "arch_iwd", "update my system").unwrap();
    let reasons: Vec<&str> = p.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(
        reasons,
        [
            "needs snapshot_tool snapper, this system has none",
            "needs snapshot_tool timeshift, this system has none",
            "needs aur_helper paru or yay, this system has none",
            "`paccache` is not installed; it comes with pacman-contrib",
        ]
    );
}

#[test]
fn missing_required_program_or_object_is_an_error() {
    let lib = library();
    // iwd's unit list lacks iwd.service in this copy of the fixture.
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no_iwd_unit");
    let _ = std::fs::remove_dir_all(&tmp);
    copy_dir(&root().join("eval/fixtures/systems/arch_iwd"), &tmp);
    std::fs::write(
        tmp.join("queries/unit-files.txt"),
        "sshd.service disabled disabled\n",
    )
    .unwrap();
    let sys = FixtureSystem::new(&tmp);
    let profile = detect(&sys);
    let values = Default::default();
    assert_eq!(
        build(&lib, "network.restart_wifi", &values, &profile, &sys)
            .unwrap_err()
            .to_string(),
        "systemd_unit 'iwd.service' does not exist on this system"
    );
    std::fs::remove_file(tmp.join("root/usr/bin/systemctl")).unwrap();
    std::fs::write(
        tmp.join("queries/unit-files.txt"),
        "iwd.service enabled disabled\n",
    )
    .unwrap();
    assert_eq!(
        build(&lib, "network.restart_wifi", &values, &profile, &sys).unwrap_err(),
        PlanError::NotInstalled {
            program: "systemctl".into(),
            package: Some("systemd".into())
        }
    );
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}
