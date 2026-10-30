//! The `sancho` binary.

mod ask;
mod history;

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use sancho_core::{format_errors, FixtureSystem, Library, LiveSystem, System};
use sancho_match::{interpret_with, Answers, Interpretation, Matcher};
use sancho_profile::detect;

/// Turn plain-language requests into verified, reversible Arch Linux tasks.
///
/// Example: sancho update my system
#[derive(Parser, Debug)]
#[command(
    name = "sancho",
    version,
    about,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Show the plan and exit without changing anything.
    #[arg(long, short = 'n')]
    dry_run: bool,

    /// Plan against a captured system fixture instead of this machine
    /// (for testing; implies --dry-run).
    #[arg(long, hide = true, value_name = "DIR")]
    fixture: Option<PathBuf>,

    /// What you want done, in plain words.
    #[arg(trailing_var_arg = true, value_name = "REQUEST")]
    request: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show what sancho detected about this system.
    Profile,
    /// List what sancho has done.
    History,
    /// Undo the last change, or one you name.
    ///
    /// Examples: `sancho undo`, `sancho undo keyboard`, `sancho undo --last 3`,
    /// `sancho undo --since 2h`, `sancho undo <id> --step 2`.
    Undo {
        /// A transaction ID or words from the request ("keyboard", "sshd").
        what: Vec<String>,
        /// Undo the last N changes.
        #[arg(long, value_name = "N", conflicts_with_all = ["what", "since"])]
        last: Option<usize>,
        /// Undo everything since a time ago: 30m, 2h, 3d or today.
        #[arg(long, value_name = "WHEN", conflicts_with = "what")]
        since: Option<String>,
        /// Undo only this step (1-based; repeatable).
        #[arg(long = "step", value_name = "N")]
        steps: Vec<usize>,
        /// Show what would be undone and exit.
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
    /// Explain a command line, flag by flag, without running it.
    ///
    /// Quote it so your shell doesn't run parts of it:
    /// sancho explain "curl -fsSL https://example.com/x.sh | sh"
    Explain {
        /// The command line.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Undo the last undo.
    Redo {
        /// Show what would be redone and exit.
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
}

const ROOT_REFUSAL: &str = "sancho: refusing to run as root.\n\
Run sancho as your normal user; it asks for sudo for each command that needs it \
and shows you the exact command first.";

/// sancho never runs as root; it escalates per command instead.
fn check_not_root(euid: u32) -> Result<(), &'static str> {
    if euid == 0 {
        Err(ROOT_REFUSAL)
    } else {
        Ok(())
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid() has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn main() -> ExitCode {
    // clap handles --help/--version (and usage errors) inside parse() and exits,
    // so those are allowed as root. Everything else is checked before it runs.
    let cli = Cli::parse();

    if let Err(msg) = check_not_root(effective_uid()) {
        eprintln!("{msg}");
        return ExitCode::FAILURE;
    }

    match cli.command {
        Some(Command::Profile) => {
            print_profile();
            ExitCode::SUCCESS
        }
        Some(Command::History) => history::history(),
        Some(Command::Undo {
            what,
            last,
            since,
            steps,
            dry_run,
        }) => {
            let target = match (last, since) {
                (Some(n), _) => history::Target::Count(n),
                (_, Some(s)) => match history::parse_since(&s) {
                    Ok(secs) => history::Target::Since(secs),
                    Err(e) => {
                        eprintln!("sancho: {e}");
                        return ExitCode::FAILURE;
                    }
                },
                _ if what.is_empty() => history::Target::Last,
                _ => history::Target::Search(what.join(" ")),
            };
            history::undo(target, &steps, dry_run)
        }
        Some(Command::Redo { dry_run }) => history::undo(history::Target::Redo, &[], dry_run),
        Some(Command::Explain { command }) => explain(&command.join(" ")),
        None if cli.request.is_empty() => {
            let _ = Cli::command().print_help();
            ExitCode::SUCCESS
        }
        None => run_request(&cli),
    }
}

/// Exit code when sancho needs an answer but stdin is not a terminal.
const NEEDS_ANSWER: u8 = 2;

fn run_request(cli: &Cli) -> ExitCode {
    let lib = match Library::load(&data_dir()) {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!(
                "sancho: the recipe library failed to load:\n{}",
                format_errors(&e)
            );
            return ExitCode::FAILURE;
        }
    };
    let sys: Box<dyn System> = match &cli.fixture {
        Some(dir) => Box::new(FixtureSystem::new(dir)),
        None => Box::new(LiveSystem),
    };
    let dry_run = cli.dry_run || cli.fixture.is_some();
    let mut text = cli.request.join(" ");
    let is_program = |w: &str| {
        lib.commands.contains_key(w) || sancho_explain::man_summary(sys.as_ref(), w).is_some()
    };
    match sancho_explain::route(&text, is_program) {
        sancho_explain::Route::Explain(what) => return explain_with(&lib, sys.as_ref(), &what),
        sancho_explain::Route::HelpWith(what) => {
            let options = vec![
                format!("Explain: what is \"{what}\"?"),
                format!("Do: \"{what}\""),
                format!("Fix a problem with \"{what}\""),
            ];
            if !io::stdin().is_terminal() {
                println!("What kind of help?");
                for (i, o) in options.iter().enumerate() {
                    println!("  {}) {o}", i + 1);
                }
                eprintln!(
                    "sancho: this needs an answer; run it in a terminal or be more specific."
                );
                return ExitCode::from(NEEDS_ANSWER);
            }
            match ask::choose(
                &mut io::stdin().lock(),
                &mut io::stdout(),
                "What kind of help?",
                &options,
            ) {
                Ok(Some(0)) => return explain_with(&lib, sys.as_ref(), &what),
                Ok(Some(1)) => text = what,
                Ok(Some(_)) => {
                    println!("Fixing problems isn't available yet. Try describing what you want done, e.g. \"restart wifi\".");
                    return ExitCode::SUCCESS;
                }
                _ => {
                    println!("Cancelled; nothing was changed.");
                    return ExitCode::SUCCESS;
                }
            }
        }
        sancho_explain::Route::Do => {}
    }
    let profile = detect(sys.as_ref());
    let matcher = Matcher::new(&lib);
    let interactive = io::stdin().is_terminal();
    let mut answers = Answers::default();

    loop {
        let (question, labels, pick): (String, Vec<String>, Pick) =
            match interpret_with(&matcher, &profile, sys.as_ref(), &text, &answers) {
                Interpretation::Ready {
                    recipe,
                    values,
                    corrections,
                } => {
                    let mut plan =
                        match sancho_plan::build(&lib, &recipe, &values, &profile, sys.as_ref()) {
                            Ok(p) => p,
                            Err(e) => {
                                eprintln!("sancho: can't plan this: {e}");
                                return ExitCode::FAILURE;
                            }
                        };
                    for (typed, read) in corrections.iter().rev() {
                        plan.notes
                            .insert(0, format!("Read \"{typed}\" as \"{read}\"."));
                    }
                    print!("{}", sancho_plan::render::plan(&plan));
                    if dry_run {
                        return ExitCode::SUCCESS;
                    }
                    if !interactive && plan.confirmation() != sancho_core::plan::Confirmation::None
                    {
                        eprintln!("sancho: confirmation needs a terminal; nothing was changed.");
                        return ExitCode::from(NEEDS_ANSWER);
                    }
                    let mut stdin = io::stdin().lock();
                    return match ask::confirm(&mut stdin, &mut io::stdout(), plan.confirmation()) {
                        Ok(true) => run_plan(&plan, &text),
                        _ => {
                            println!("Cancelled; nothing was changed.");
                            ExitCode::SUCCESS
                        }
                    };
                }
                Interpretation::ChooseRecipe { options, suggested } => {
                    let question = if suggested {
                        "I'm not sure what you mean. Did you want to:"
                    } else {
                        "That could mean several things. Which one?"
                    };
                    let labels = options
                        .iter()
                        .map(|id| lib.recipes[id].summary.clone())
                        .collect();
                    (question.to_string(), labels, Pick::Recipe(options))
                }
                Interpretation::ChooseValue {
                    slot, candidates, ..
                } => {
                    let labels = candidates.iter().map(|c| c.value.display.clone()).collect();
                    let values = candidates.into_iter().map(|c| c.value).collect();
                    (format!("Which {slot}?"), labels, Pick::Value(slot, values))
                }
                Interpretation::NotFound { reason, .. } => {
                    eprintln!("sancho: {reason}");
                    return ExitCode::FAILURE;
                }
                Interpretation::Unsupported { reason } => {
                    eprintln!("sancho: {reason}");
                    return ExitCode::FAILURE;
                }
            };
        if !interactive {
            println!("{question}");
            for (i, l) in labels.iter().enumerate() {
                println!("  {}) {l}", i + 1);
            }
            eprintln!("sancho: this needs an answer; run it in a terminal or be more specific.");
            return ExitCode::from(NEEDS_ANSWER);
        }
        let mut stdin = io::stdin().lock();
        match ask::choose(&mut stdin, &mut io::stdout(), &question, &labels) {
            Ok(Some(i)) => pick.apply(i, &mut answers),
            _ => {
                println!("Cancelled; nothing was changed.");
                return ExitCode::SUCCESS;
            }
        }
    }
}

/// What the options in a question stand for.
enum Pick {
    Recipe(Vec<String>),
    Value(String, Vec<sancho_resolve::Value>),
}

impl Pick {
    fn apply(self, i: usize, answers: &mut Answers) {
        match self {
            Pick::Recipe(ids) => answers.recipe = ids.into_iter().nth(i),
            Pick::Value(slot, values) => {
                answers
                    .values
                    .insert(slot, values.into_iter().skip(i).take(1).collect());
            }
        }
    }
}

fn explain(command: &str) -> ExitCode {
    match Library::load(&data_dir()) {
        Ok(lib) => explain_with(&lib, &LiveSystem, command),
        Err(e) => {
            eprintln!(
                "sancho: the recipe library failed to load:\n{}",
                format_errors(&e)
            );
            ExitCode::FAILURE
        }
    }
}

/// Explains without running anything.
fn explain_with(lib: &Library, sys: &dyn System, command: &str) -> ExitCode {
    let e = sancho_explain::explain(lib, sys, command);
    print!("{}", sancho_explain::render::explanation(&e));
    ExitCode::SUCCESS
}

/// Runs a confirmed plan through the journal; offers rollback on failure.
fn run_plan(plan: &sancho_core::Plan, request: &str) -> ExitCode {
    use sancho_journal::{apply_undo, execute, plan_undo, RunOutcome};

    let journal = match history::journal() {
        Ok(j) => j,
        Err(e) => {
            eprintln!("sancho: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut ex = sancho_exec::SudoExecutor;
    let total = plan.steps.len();
    let mut progress = |i: usize| println!("[{}/{total}] {}", i + 1, plan.steps[i].explanation);
    match execute(&journal, &mut ex, plan, request, &mut progress) {
        Ok(RunOutcome::Completed { tx }) => {
            if plan.steps.iter().any(|s| !s.read_only) {
                println!("Done. `sancho undo` reverses this ({tx}).");
            }
            ExitCode::SUCCESS
        }
        Ok(RunOutcome::Failed { tx, step, exit }) => {
            println!(
                "Step {} failed (exit {exit}); later steps did not run.",
                step + 1
            );
            if step == 0 {
                return ExitCode::FAILURE;
            }
            let undo = match plan_undo(&journal, &mut ex, std::slice::from_ref(&tx), None) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("sancho: {e}");
                    return ExitCode::FAILURE;
                }
            };
            if undo.actions.iter().all(|a| a.restores.is_empty()) {
                return ExitCode::FAILURE;
            }
            print!("Roll back the {step} completed step(s)? [y/N] ");
            let _ = io::Write::flush(&mut io::stdout());
            let yes = ask::confirm(
                &mut io::stdin().lock(),
                &mut io::sink(),
                sancho_core::plan::Confirmation::YesNo,
            )
            .unwrap_or(false);
            if yes {
                match apply_undo(&journal, &mut ex, &undo, &format!("roll back {tx}"), false) {
                    Ok(RunOutcome::Completed { .. }) => println!("Rolled back."),
                    Ok(RunOutcome::Failed { step, .. }) => {
                        println!("Rollback step {} failed; see `sancho history`.", step + 1)
                    }
                    Err(e) => eprintln!("sancho: {e}"),
                }
            } else {
                println!("Left as is; `sancho undo` can roll it back later.");
            }
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("sancho: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Where recipes and primitives live: `$SANCHO_DATA`, else the system-wide
/// install, else (development builds) the repository's `data/`.
fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SANCHO_DATA") {
        return PathBuf::from(dir);
    }
    let installed = PathBuf::from("/usr/share/sancho/data");
    if installed.is_dir() || !cfg!(debug_assertions) {
        return installed;
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data"))
}

fn print_profile() {
    let profile = detect(&LiveSystem);
    for (key, value) in profile.iter() {
        println!("{:<14}{value}", key.name());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_refused() {
        assert_eq!(check_not_root(0), Err(ROOT_REFUSAL));
    }

    #[test]
    fn normal_users_are_allowed() {
        assert_eq!(check_not_root(1000), Ok(()));
        assert_eq!(check_not_root(1), Ok(()));
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
