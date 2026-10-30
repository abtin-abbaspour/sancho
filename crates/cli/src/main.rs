//! The `sancho` binary.

mod ask;

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
    let profile = detect(sys.as_ref());
    let matcher = Matcher::new(&lib);
    let text = cli.request.join(" ");
    let interactive = io::stdin().is_terminal();
    let mut answers = Answers::default();

    loop {
        let (question, labels, pick): (String, Vec<String>, Pick) = match interpret_with(
            &matcher,
            &profile,
            sys.as_ref(),
            &text,
            &answers,
        ) {
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
                if !interactive && plan.confirmation() != sancho_core::plan::Confirmation::None {
                    eprintln!("sancho: confirmation needs a terminal; nothing was changed.");
                    return ExitCode::from(NEEDS_ANSWER);
                }
                let mut stdin = io::stdin().lock();
                return match ask::confirm(&mut stdin, &mut io::stdout(), plan.confirmation()) {
                    Ok(true) => {
                        println!("Running plans is not available yet in this version; nothing was changed.");
                        ExitCode::SUCCESS
                    }
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
