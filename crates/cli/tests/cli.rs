//! End-to-end tests of the `sancho` binary.

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_sancho");

fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().expect("spawn sancho")
}

#[test]
fn version_prints_name_and_version() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        format!("sancho {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_describes_the_tool() {
    let out = run(&["--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Usage: sancho"));
    assert!(stdout.contains("--version"));
}

#[test]
fn profile_lists_every_fact() {
    let out = run(&["profile"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let keys: Vec<&str> = stdout
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert_eq!(
        keys,
        [
            "distro",
            "init",
            "network",
            "audio",
            "display",
            "aur_helper",
            "root_fs",
            "snapshot_tool",
            "bootloader"
        ]
    );
}

/// Runs the binary as uid 0 inside an unprivileged user namespace
/// (`unshare --map-root-user`). This grants no real privileges on the host.
/// Returns `None` when user namespaces are unavailable.
fn run_as_root(args: &[&str]) -> Option<Output> {
    let probe = Command::new("unshare")
        .args(["--user", "--map-root-user", "true"])
        .output();
    if !matches!(probe, Ok(ref o) if o.status.success()) {
        eprintln!("skipping: unprivileged user namespaces unavailable");
        return None;
    }
    let out = Command::new("unshare")
        .args(["--user", "--map-root-user", BIN])
        .args(args)
        .output()
        .expect("spawn sancho via unshare");
    Some(out)
}

#[test]
fn refuses_to_run_as_root() {
    let Some(out) = run_as_root(&[]) else { return };
    assert!(!out.status.success(), "ran as root");
    assert!(out.stdout.is_empty(), "printed output as root");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("refusing to run as root"), "{stderr}");
}

#[test]
fn help_and_version_are_allowed_as_root() {
    for args in [&["--help"][..], &["--version"][..]] {
        let Some(out) = run_as_root(args) else { return };
        assert!(out.status.success(), "{args:?} failed as root");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("refusing"), "{stderr}");
    }
}
