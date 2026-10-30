//! End-to-end tests of the `sancho` binary.

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_sancho");

/// Data and state directories for one test, so tests never touch the
/// user's real learned store or journal.
fn home(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli-home")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_in(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        // Explicit, so tests don't depend on how the build was configured
        // (release-like builds look in /usr/share/sancho/data).
        .env(
            "SANCHO_DATA",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../data"),
        )
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .output()
        .expect("spawn sancho")
}

fn run(args: &[&str]) -> Output {
    // Shared by tests that only read: created, never cleared.
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-home/shared");
    std::fs::create_dir_all(&dir).unwrap();
    run_in(&dir, args)
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

fn seed_preference(dir: &std::path::Path, key: &str) {
    let store = dir.join("data/sancho");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(
        store.join("learned.json"),
        format!(
            r#"{{"next_id": 7, "items": [{{"id": 7, "kind": "preference",
                "recipe": "device.grant_hid_browser_access", "slot": "browser",
                "mention": "my browser", "key": "{key}", "display": "Chromium", "uses": 0}}]}}"#
        ),
    )
    .unwrap();
}

#[test]
fn a_learned_answer_skips_the_question_and_is_labeled() {
    let dir = home("learned");
    seed_preference(&dir, "chromium.desktop");
    let fixture = fixture();
    let out = run_in(
        &dir,
        &[
            "--fixture",
            &fixture,
            "let",
            "my",
            "browser",
            "configure",
            "my",
            "keyboard",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Learned from your earlier answers:"),
        "{stdout}"
    );
    assert!(stdout.contains("(forget it: sancho learned rm 7)"));
    assert!(stdout.contains("from browsers such as Chromium"));

    let listed = String::from_utf8_lossy(&run_in(&dir, &["learned"]).stdout).to_string();
    assert!(listed.contains("   7  for device.grant_hid_browser_access, browser with \"my browser\" means Chromium  (used 1 times)"), "{listed}");
    assert!(run_in(&dir, &["learned", "rm", "7"]).status.success());
    assert!(!run_in(&dir, &["learned", "rm", "7"]).status.success());
    // Forgotten: the question comes back.
    let out = run_in(
        &dir,
        &[
            "--fixture",
            &fixture,
            "let",
            "my",
            "browser",
            "configure",
            "my",
            "keyboard",
        ],
    );
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_learned_value_that_no_longer_exists_is_ignored() {
    let dir = home("stale");
    seed_preference(&dir, "vivaldi.desktop");
    let fixture = fixture();
    let out = run_in(
        &dir,
        &[
            "--fixture",
            &fixture,
            "let",
            "my",
            "browser",
            "configure",
            "my",
            "keyboard",
        ],
    );
    assert_eq!(out.status.code(), Some(2), "asks again: {out:?}");
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
