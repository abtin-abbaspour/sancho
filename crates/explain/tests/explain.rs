//! Explanations of command lines from the shipped command notes.

use std::path::{Path, PathBuf};

use sancho_core::{format_errors, FixtureSystem, Library, Risk, ValueType};
use sancho_explain::{explain, explain_words, render, route, ItemKind, Route};

fn library() -> Library {
    Library::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
        .unwrap_or_else(|e| panic!("{}", format_errors(&e)))
}

/// A system whose only knowledge is a `man -f` answer for `frob`.
/// Created once: tests run in parallel and must not rewrite it under each other.
fn system() -> FixtureSystem {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("explain-sys");
        std::fs::create_dir_all(dir.join("queries")).unwrap();
        std::fs::write(
            dir.join("queries/man-f-frob.txt"),
            "frob (1)             - frobnicate the widgets\n",
        )
        .unwrap();
        dir
    });
    FixtureSystem::new(dir)
}

fn kinds(e: &sancho_explain::Explanation, program: &str) -> Vec<(String, ItemKind)> {
    e.commands
        .iter()
        .find(|c| c.program == program)
        .unwrap_or_else(|| panic!("no {program} in {e:?}"))
        .items
        .iter()
        .map(|i| (i.text.clone(), i.kind))
        .collect()
}

#[test]
fn every_primitive_command_is_fully_explained() {
    let lib = library();
    let sys = system();
    for (id, prim) in &lib.primitives {
        for program in programs(prim) {
            let argv: Vec<String> = prim
                .command
                .iter()
                .enumerate()
                .map(|(i, t)| match t.as_single_ref() {
                    None => t.as_literal().unwrap(),
                    Some(_) if i == 0 => program.clone(),
                    Some(r) => sample(prim.args[&r.name].ty, &prim.args[&r.name].values),
                })
                .collect();
            let e = explain_words(&lib, &sys, &argv, false, false);
            let c = &e[0];
            assert!(c.known, "{id}: no notes on {program}");
            let unknown: Vec<&str> = c
                .items
                .iter()
                .filter(|i| i.kind == ItemKind::Unknown)
                .map(|i| i.text.as_str())
                .collect();
            assert!(
                unknown.is_empty(),
                "{id}: unexplained {unknown:?} in {argv:?}"
            );
        }
    }
}

/// Program names a primitive can run (a choice arg may pick the program).
fn programs(prim: &sancho_core::Primitive) -> Vec<String> {
    match prim.command[0].as_single_ref() {
        Some(r) => prim.args[&r.name].values.clone(),
        None => vec![prim.command[0].as_literal().unwrap()],
    }
}

fn sample(ty: ValueType, values: &[String]) -> String {
    match ty {
        ValueType::Choice => values[0].clone(),
        ValueType::Count => "2".into(),
        ValueType::FileMode => "0644".into(),
        ValueType::Path => "/etc/example".into(),
        _ => "example".into(),
    }
}

#[test]
fn flag_clusters_values_and_subcommands() {
    let lib = library();
    let sys = system();
    let e = explain(&lib, &sys, "pacman -Syu --needed");
    assert_eq!(
        kinds(&e, "pacman"),
        [
            ("-S".into(), ItemKind::Flag),
            ("-y".into(), ItemKind::Flag),
            ("-u".into(), ItemKind::Flag),
            ("--needed".into(), ItemKind::Flag),
        ]
    );
    let e = explain(&lib, &sys, "paccache -rk2");
    let texts: Vec<String> = kinds(&e, "paccache").into_iter().map(|k| k.0).collect();
    assert_eq!(texts, ["-r", "-k 2"]);
    let e = explain(
        &lib,
        &sys,
        "udevadm trigger --action=change --subsystem-match hidraw",
    );
    let texts: Vec<String> = kinds(&e, "udevadm").into_iter().map(|k| k.0).collect();
    assert_eq!(
        texts,
        ["trigger", "--action=change", "--subsystem-match hidraw"]
    );
    let e = explain(&lib, &sys, "ip -j link show");
    assert_eq!(
        kinds(&e, "ip")[1],
        ("link show".into(), ItemKind::Subcommand)
    );
    assert_eq!(
        e.risk(),
        Some(Risk::Low),
        "a read-only subcommand lowers ip's risk"
    );
}

#[test]
fn unknown_flags_are_flagged_never_guessed() {
    let lib = library();
    let sys = system();
    let e = explain(&lib, &sys, "pacman -Syu --frobnicate -Sz");
    assert_eq!(
        e.unknown_flags(),
        ["--frobnicate", "-Sz"],
        "-Sz: z is unknown, so the whole cluster is"
    );
    assert_eq!(e.risk(), None, "risk is unknown when anything is unknown");
    let text = render::explanation(&e);
    assert!(
        text.contains("?? not in sancho's notes on pacman — see `man pacman`"),
        "{text}"
    );
    assert!(text.contains("Risk: unknown"));
}

#[test]
fn unknown_programs_use_the_man_page_summary() {
    let lib = library();
    let e = explain(&lib, &system(), "frob --widgets 3");
    let c = &e.commands[0];
    assert!(!c.known);
    assert_eq!(c.summary.as_deref(), Some("frobnicate the widgets"));
    assert_eq!(e.unknown_flags(), ["--widgets"]);
    let e = explain(&lib, &system(), "nosuchtool");
    assert!(render::explanation(&e).contains("found no man page"));
}

#[test]
fn sudo_runs_the_rest_as_root() {
    let lib = library();
    let e = explain(&lib, &system(), "sudo -E pacman -Syu");
    assert_eq!(e.commands.len(), 2);
    assert_eq!(e.commands[0].program, "sudo");
    assert_eq!(e.commands[0].items[0].text, "-E");
    assert!(e.commands[1].as_root && e.commands[1].program == "pacman");
    assert!(render::explanation(&e).contains("pacman (as root):"));
}

#[test]
fn piping_downloads_into_a_shell_is_high_risk() {
    let lib = library();
    let e = explain(
        &lib,
        &system(),
        "curl -fsSL https://example.com/install.sh | sh",
    );
    assert_eq!(e.risk(), Some(Risk::High));
    assert!(
        e.notes[0].text.contains("into `sh`, which runs it as code"),
        "{:?}",
        e.notes
    );
    // A script file argument is not "running the pipe".
    let e = explain(&lib, &system(), "echo hi | bash -x ./script.sh");
    assert!(e.notes.is_empty(), "{:?}", e.notes);
}

#[test]
fn dangerous_arguments_redirects_and_substitutions() {
    let lib = library();
    let sys = system();
    let e = explain(&lib, &sys, "sudo rm -rf /");
    assert_eq!(e.risk(), Some(Risk::High));
    assert!(
        e.notes.iter().any(|n| n.text.starts_with("`rm /`")),
        "{:?}",
        e.notes
    );

    let e = explain(&lib, &sys, "echo 'nameserver 1.1.1.1' > /etc/resolv.conf");
    let n = &e.notes[0];
    assert_eq!(n.risk, Risk::High);
    assert!(
        n.text.contains("replaces the contents of /etc/resolv.conf"),
        "{}",
        n.text
    );

    let e = explain(&lib, &sys, "sudo pacman -Rns $(pacman -Qtdq) 2>/dev/null");
    let nested: Vec<&str> = e
        .commands
        .iter()
        .filter(|c| c.nested)
        .map(|c| c.program.as_str())
        .collect();
    assert_eq!(nested, ["pacman"]);
    assert!(e.notes.iter().any(|n| n.text.contains("runs first")));
    assert!(e
        .notes
        .iter()
        .any(|n| n.text.contains("throws that output away")));
}

#[test]
fn unparseable_input_is_marked_incomplete() {
    let e = explain(&library(), &system(), "pacman -S 'unterminated");
    assert!(e.incomplete);
    assert_eq!(e.risk(), None);
}

#[test]
fn requests_are_routed_to_explain_help_or_do() {
    let lib = library();
    let r = |t: &str| route(t, |w| lib.commands.contains_key(w));
    assert_eq!(
        r("what does pacman -Syu do?"),
        Route::Explain("pacman -Syu".into())
    );
    assert_eq!(
        r("Explain sudo rm -rf /"),
        Route::Explain("sudo rm -rf /".into())
    );
    assert_eq!(r("what is paccache"), Route::Explain("paccache".into()));
    assert_eq!(r("help me with my wifi"), Route::HelpWith("my wifi".into()));
    assert_eq!(r("restart sshd"), Route::Do);
    assert_eq!(r("explain"), Route::Do);
    // Not a program: an ordinary request, matched (and declined) as usual.
    assert_eq!(r("what's the weather like"), Route::Do);
    assert_eq!(r("what is my ip"), Route::Do);
}
