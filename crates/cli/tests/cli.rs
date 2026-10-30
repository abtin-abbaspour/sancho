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

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../eval/fixtures/systems/eval_desktop"
    )
    .to_string()
}

fn request(words: &[&str]) -> Output {
    let fixture = fixture();
    let mut args = vec!["--fixture", fixture.as_str()];
    args.extend_from_slice(words);
    run(&args)
}

#[test]
fn a_request_prints_its_plan() {
    let out = request(&["restart", "sshd"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("[verified recipe · service.restart v1]"),
        "{stdout}"
    );
    assert!(stdout.contains("$ sudo systemctl restart sshd.service"));
}

#[test]
fn typo_corrections_are_shown() {
    let out = request(&["restrat", "sshd"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Read \"restrat\" as \"restart\"."),
        "{stdout}"
    );
}

#[test]
fn questions_without_a_terminal_list_options_and_exit_2() {
    let out = request(&["let", "my", "browser", "configure", "my", "keyboard"]);
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "Which browser?\n  1) Brave\n  2) Chromium\n");
}

#[test]
fn unsupported_requests_explain_and_exit_1() {
    let out = request(&["make", "me", "a", "sandwich"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "sancho: no recipe knows how to do that yet\n"
    );
}

#[test]
fn explain_describes_flags_and_flags_unknown_ones() {
    let out = run(&["explain", "pacman -Syu --frobnicate"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = |start: &str| {
        stdout
            .lines()
            .find(|l| l.trim_start().starts_with(start))
            .unwrap_or_else(|| panic!("no {start} line in {stdout}"))
            .to_string()
    };
    assert!(line("-y ").ends_with("Download fresh package databases first"));
    assert!(line("--frobnicate").contains("?? not in sancho's notes on pacman"));
    assert!(stdout.ends_with("Risk: unknown (something above is not in sancho's notes)\n"));
}

#[test]
fn natural_language_explain_and_help_routing() {
    let out = request(&["what", "does", "systemctl", "status", "sshd", "do"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("$ systemctl status sshd\n"), "{stdout}");
    assert!(stdout.ends_with("Risk: low\n"));
    let out = request(&["help", "me", "with", "sshd"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("What kind of help?\n"));
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
