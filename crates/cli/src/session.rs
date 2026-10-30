//! `sancho` on its own: a conversation. Each line is natural language;
//! a few phrasings reach history, undo and the learned store, and everything
//! else is a request (or a question about a command). Nothing runs without
//! the usual plan and confirmation.

use std::io::{self, BufRead, IsTerminal};
use std::path::Path;
use std::process::ExitCode;

use sancho_core::Library;

use crate::history::{self, Target};
use crate::{learned, print_profile, run_request, system};

/// What a line in a session asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Exit,
    Help,
    History,
    Undo(UndoWhat),
    Redo,
    Profile,
    Learned,
    Forget(u32),
    /// A request, or a question about a command.
    Request(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoWhat {
    Last,
    Count(usize),
    Since(String),
    Search(String),
}

/// Recognizes the session's own phrasings; everything else is a request.
pub fn route(line: &str) -> Line {
    let text = line.trim().trim_end_matches(['?', '!', '.']).trim();
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    match lower.as_str() {
        "exit" | "quit" | "q" | "bye" | "goodbye" | "thanks, bye" => return Line::Exit,
        "help" | "?" | "what can you do" | "what can i say" | "commands" => return Line::Help,
        "history"
        | "show history"
        | "show my history"
        | "what did you do"
        | "what have you done"
        | "what did you change"
        | "what have you changed" => return Line::History,
        "redo" | "redo that" | "redo it" | "undo the undo" => return Line::Redo,
        "profile"
        | "show my system"
        | "what system is this"
        | "what do you know about my system"
        | "describe my system" => return Line::Profile,
        "learned" | "what have you learned" | "what did you learn" | "show what you learned" => {
            return Line::Learned
        }
        _ => {}
    }
    if let Some(id) = lower
        .strip_prefix("forget ")
        .and_then(|r| r.trim().trim_start_matches('#').parse().ok())
    {
        return Line::Forget(id);
    }
    if matches!(words.first(), Some(&"undo" | &"revert" | &"reverse")) {
        let rest: Vec<&str> = words[1..]
            .iter()
            .copied()
            .filter(|w| {
                ![
                    "the", "my", "that", "it", "this", "change", "changes", "thing", "please",
                ]
                .contains(w)
            })
            .collect();
        return Line::Undo(match rest.as_slice() {
            [] | ["last"] => UndoWhat::Last,
            ["last", n] | ["last", n, ..] if n.parse::<usize>().is_ok() => {
                UndoWhat::Count(n.parse().unwrap_or(1))
            }
            ["everything", "since", when @ ..] | ["since", when @ ..] if !when.is_empty() => {
                UndoWhat::Since(when.join(" "))
            }
            words => UndoWhat::Search(words.join(" ")),
        });
    }
    Line::Request(text.to_string())
}

/// Where lines come from: a line editor with in-memory history (arrow keys,
/// Ctrl-R) on a terminal, plain stdin otherwise. History is never written
/// to disk: it would leave a file behind and keep your requests.
enum Input {
    Editor(Box<rustyline::DefaultEditor>),
    Plain,
}

impl Input {
    fn new(tty: bool) -> Input {
        match tty.then(rustyline::DefaultEditor::new) {
            Some(Ok(editor)) => Input::Editor(Box::new(editor)),
            _ => Input::Plain,
        }
    }

    /// The next line, or `None` at the end of input (Ctrl-D).
    fn next_line(&mut self) -> Option<String> {
        use rustyline::error::ReadlineError;
        match self {
            Input::Editor(editor) => loop {
                println!();
                match editor.readline("sancho> ") {
                    Ok(line) => {
                        if !line.trim().is_empty() {
                            let _ = editor.add_history_entry(line.as_str());
                        }
                        return Some(line);
                    }
                    // Ctrl-C clears the line, like a shell.
                    Err(ReadlineError::Interrupted) => continue,
                    Err(_) => return None,
                }
            },
            Input::Plain => {
                let mut line = String::new();
                match io::stdin().lock().read_line(&mut line) {
                    Ok(0) | Err(_) => None,
                    Ok(_) => Some(line),
                }
            }
        }
    }
}

const HELP: &str = "\
Tell me what you want done, in plain words. For example:
  update my system              install vlc and gimp
  restart wifi                  enable sshd
  add me to the video group     how much disk space is left
  let chromium configure my keyboard

Ask about commands:   what does pacman -Syu do · explain sudo rm -rf ~/x
Your changes:         what did you do · undo that · undo the last 3
                      undo the keyboard thing · undo since 2h · redo
Also:                 show my system · what have you learned · forget 3 · exit

Up and down arrows recall earlier lines. Ctrl-C clears the line, Ctrl-D leaves.
Nothing changes without your confirmation, and every change can be listed and undone.";

pub fn run(lib: &Library, fixture: Option<&Path>, dry_run: bool) -> ExitCode {
    let tty = io::stdin().is_terminal();
    if tty {
        println!(
            "sancho {} — tell me what you want done. Type \"help\" for examples, \"exit\" to leave.",
            env!("CARGO_PKG_VERSION")
        );
        if dry_run || fixture.is_some() {
            println!("Practice mode: plans are shown but never run.");
        }
    }
    let dry_run = dry_run || fixture.is_some();
    let mut input = Input::new(tty);
    while let Some(line) = input.next_line() {
        if line.trim().is_empty() {
            continue;
        }
        let _ = match route(&line) {
            Line::Exit => break,
            Line::Help => {
                println!("{HELP}");
                ExitCode::SUCCESS
            }
            Line::History => history::history(),
            Line::Undo(what) => {
                let target = match what {
                    UndoWhat::Last => Target::Last,
                    UndoWhat::Count(n) => Target::Count(n),
                    UndoWhat::Search(words) => Target::Search(words),
                    UndoWhat::Since(when) => match history::parse_since(&when.replace(' ', "")) {
                        Ok(secs) => Target::Since(secs),
                        Err(e) => {
                            eprintln!("sancho: {e}");
                            continue;
                        }
                    },
                };
                history::undo(target, &[], dry_run)
            }
            Line::Redo => history::undo(Target::Redo, &[], dry_run),
            Line::Profile => {
                print_profile(system(fixture).as_ref());
                ExitCode::SUCCESS
            }
            Line::Learned => learned::list(),
            Line::Forget(id) => learned::remove(id),
            Line::Request(text) => run_request(lib, fixture, dry_run, &text),
        };
    }
    if tty {
        println!();
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_phrasings() {
        assert_eq!(route("exit"), Line::Exit);
        assert_eq!(route("Quit!"), Line::Exit);
        assert_eq!(route("what can you do?"), Line::Help);
        assert_eq!(route("What did you do?"), Line::History);
        assert_eq!(route("redo that"), Line::Redo);
        assert_eq!(route("show my system"), Line::Profile);
        assert_eq!(route("what have you learned"), Line::Learned);
        assert_eq!(route("forget #3"), Line::Forget(3));
    }

    #[test]
    fn undo_phrasings() {
        assert_eq!(route("undo"), Line::Undo(UndoWhat::Last));
        assert_eq!(route("undo that"), Line::Undo(UndoWhat::Last));
        assert_eq!(route("revert the last change"), Line::Undo(UndoWhat::Last));
        assert_eq!(route("undo the last 3"), Line::Undo(UndoWhat::Count(3)));
        assert_eq!(
            route("undo the last 2 changes"),
            Line::Undo(UndoWhat::Count(2))
        );
        assert_eq!(
            route("undo everything since 2h"),
            Line::Undo(UndoWhat::Since("2h".into()))
        );
        assert_eq!(
            route("undo the keyboard thing"),
            Line::Undo(UndoWhat::Search("keyboard".into()))
        );
    }

    #[test]
    fn everything_else_is_a_request() {
        assert_eq!(
            route("update my system"),
            Line::Request("update my system".into())
        );
        assert_eq!(route("forget it"), Line::Request("forget it".into()));
        assert_eq!(
            route("what does pacman -Syu do?"),
            Line::Request("what does pacman -Syu do".into())
        );
    }
}
