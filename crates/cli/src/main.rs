//! The `sancho` binary.

mod ask;
mod history;
mod learned;
mod session;
mod ui;

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use sancho_core::{format_errors, FixtureSystem, Library, LiveSystem, System};
use sancho_learn::Learned;
use sancho_match::{interpret_with, Answers, Interpretation, Matcher};
use sancho_profile::detect;

/// Turn plain-language requests into verified, reversible Arch Linux tasks.
///
/// Run `sancho` on its own to talk to it, or give a request directly:
/// sancho update my system
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
    /// List, forget or export what sancho learned from your answers.
    Learned {
        #[command(subcommand)]
        action: Option<LearnedAction>,
    },
    /// Undo the last undo.
    Redo {
        /// Show what would be redone and exit.
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
enum LearnedAction {
    /// List learned answers (the default).
    List,
    /// Forget a learned answer.
    Rm { id: u32 },
    /// Write a learned answer to a file for sharing, with personal details removed.
    Export {
        id: u32,
        /// Where to write it (default: sancho-learned-<id>.toml).
        #[arg(long, short)]
        output: Option<PathBuf>,
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
            print_profile(system(cli.fixture.as_deref()).as_ref());
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
                        ui::fail(&e.to_string());
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
        Some(Command::Learned { action }) => match action.unwrap_or(LearnedAction::List) {
            LearnedAction::List => learned::list(),
            LearnedAction::Rm { id } => learned::remove(id),
            LearnedAction::Export { id, output } => learned::export(id, output),
        },
        None if cli.request.is_empty() => match load_library() {
            Ok(lib) => session::run(&lib, cli.fixture.as_deref(), cli.dry_run),
            Err(code) => code,
        },
        None => match load_library() {
            Ok(lib) => run_request(
                &lib,
                cli.fixture.as_deref(),
                cli.dry_run,
                &cli.request.join(" "),
            ),
            Err(code) => code,
        },
    }
}

/// Exit code when sancho needs an answer but stdin is not a terminal.
const NEEDS_ANSWER: u8 = 2;

/// The recipe library, or the reason it couldn't be loaded.
fn load_library() -> Result<Library, ExitCode> {
    Library::load(&data_dir()).map_err(|e| {
        ui::fail(&format!(
            "the recipe library failed to load:\n{}",
            format_errors(&e)
        ));
        ExitCode::FAILURE
    })
}

/// The system to plan against: a fixture (tests), or this machine with each
/// query cached for the length of one request only.
fn system(fixture: Option<&Path>) -> Box<dyn System> {
    match fixture {
        Some(dir) => Box::new(FixtureSystem::new(dir)),
        None => Box::new(sancho_core::Cached::new(LiveSystem)),
    }
}

/// Handles one plain-language request: explain, ask, plan, confirm, run.
fn run_request(lib: &Library, fixture: Option<&Path>, dry_run: bool, request: &str) -> ExitCode {
    let sys = system(fixture);
    let dry_run = dry_run || fixture.is_some();
    let mut text = request.to_string();
    let is_program = |w: &str| {
        lib.commands.contains_key(w) || sancho_explain::man_summary(sys.as_ref(), w).is_some()
    };
    match sancho_explain::route(&text, is_program) {
        sancho_explain::Route::Explain(what) => return explain_with(lib, sys.as_ref(), &what),
        sancho_explain::Route::HelpWith(what) => {
            let options = vec![
                format!("Explain: what is \"{what}\"?"),
                format!("Do: \"{what}\""),
                format!("Fix a problem with \"{what}\""),
            ];
            if !io::stdin().is_terminal() {
                let _ = ask::write_options(
                    &mut io::stdout(),
                    sancho_core::style::stdout_painter().as_ref(),
                    "What kind of help?",
                    &options,
                );
                ui::fail("this needs an answer; run it in a terminal or be more specific.");
                return ExitCode::from(NEEDS_ANSWER);
            }
            match ask::choose(
                &mut io::stdin().lock(),
                &mut io::stdout(),
                sancho_core::style::stdout_painter().as_ref(),
                "What kind of help?",
                &options,
            ) {
                Ok(Some(0)) => return explain_with(lib, sys.as_ref(), &what),
                Ok(Some(1)) => text = what,
                Ok(Some(_)) => {
                    ui::info("Fixing problems isn't available yet. Try describing what you want done, e.g. \"restart wifi\".");
                    return ExitCode::SUCCESS;
                }
                _ => {
                    ui::info("Cancelled; nothing was changed.");
                    return ExitCode::SUCCESS;
                }
            }
        }
        sancho_explain::Route::Do => {}
    }
    let profile = detect(sys.as_ref());
    let matcher = Matcher::new(lib);
    let interactive = io::stdin().is_terminal();
    let mut answers = Answers::default();
    // Answers to earlier questions; without a store, sancho just asks.
    let mut store = learned::open()
        .map_err(|e| ui::warn(&format!("{e}; answers won't be remembered")))
        .ok();
    let phrase = matcher.phrase(&text);
    let mut used_learned: Vec<(u32, String)> = Vec::new();

    loop {
        let (question, labels, pick): (String, Vec<String>, Pick) =
            match interpret_with(&matcher, &profile, sys.as_ref(), &text, &answers) {
                Interpretation::Ready {
                    recipe,
                    values,
                    corrections,
                } => {
                    // A question about the system: read and answer, nothing runs.
                    if let Some(kind) = lib.recipes[&recipe].answer {
                        let value = kind
                            .slot()
                            .and_then(|(name, _)| values.get(name))
                            .and_then(|v| v.first());
                        let fact = sancho_facts::answer(sys.as_ref(), kind, value);
                        print!(
                            "{}",
                            sancho_facts::render(
                                &fact,
                                sancho_core::style::stdout_painter().as_ref()
                            )
                        );
                        for (typed, read) in &corrections {
                            ui::info(&format!("Read \"{typed}\" as \"{read}\"."));
                        }
                        for (id, what) in &used_learned {
                            ui::info(&format!(
                                "Learned: {what} (forget it: sancho learned rm {id})"
                            ));
                        }
                        return ExitCode::SUCCESS;
                    }
                    let mut plan =
                        match sancho_plan::build(lib, &recipe, &values, &profile, sys.as_ref()) {
                            Ok(p) => p,
                            Err(e) => {
                                ui::fail(&format!("can't plan this: {e}"));
                                return ExitCode::FAILURE;
                            }
                        };
                    plan.learned = used_learned.clone();
                    for (typed, read) in corrections.iter().rev() {
                        plan.notes
                            .insert(0, format!("Read \"{typed}\" as \"{read}\"."));
                    }
                    print!(
                        "{}",
                        sancho_plan::render::plan_with(
                            &plan,
                            sancho_core::style::stdout_painter().as_ref()
                        )
                    );
                    if dry_run {
                        ui::info("Dry run: nothing was changed.");
                        return ExitCode::SUCCESS;
                    }
                    if !interactive && plan.confirmation() != sancho_core::plan::Confirmation::None
                    {
                        ui::fail("confirmation needs a terminal; nothing was changed.");
                        return ExitCode::from(NEEDS_ANSWER);
                    }
                    let mut stdin = io::stdin().lock();
                    let paint = sancho_core::style::stdout_painter();
                    return match ask::confirm(
                        &mut stdin,
                        &mut io::stdout(),
                        paint.as_ref(),
                        plan.confirmation(),
                    ) {
                        Ok(true) => run_plan(&plan, &text),
                        _ => {
                            ui::info("Cancelled; nothing was changed.");
                            ExitCode::SUCCESS
                        }
                    };
                }
                Interpretation::ChooseRecipe { options, suggested } => {
                    let remembered = store.as_ref().and_then(|s| s.alias(&phrase)).and_then(|i| {
                        match &i.learned {
                            Learned::Alias { recipe, .. } if options.contains(recipe) => {
                                Some((i.id, i.describe(), recipe.clone()))
                            }
                            _ => None,
                        }
                    });
                    if let Some((id, what, recipe)) = remembered {
                        answers.recipe = Some(recipe);
                        used_learned.push((id, what));
                        if let Some(s) = store.as_mut() {
                            let _ = s.used(id);
                        }
                        continue;
                    }
                    let question = if suggested {
                        "I'm not sure what you mean. Closest things I can do:"
                    } else {
                        "That could mean several things. Which one?"
                    };
                    let mut opts: Vec<Opt> = options.into_iter().map(Opt::Recipe).collect();
                    if suggested {
                        // Also point at programs on this system that look related.
                        opts.extend(
                            sancho_facts::related::related_programs(sys.as_ref(), &text, 3)
                                .into_iter()
                                .map(|r| Opt::Explain(r.program, r.description)),
                        );
                    }
                    let labels = opts.iter().map(|o| o.label(lib)).collect();
                    (question.to_string(), labels, Pick::Options(opts))
                }
                Interpretation::ChooseValue {
                    recipe,
                    slot,
                    candidates,
                } => {
                    let mention = matcher
                        .mentions_for(&text, &recipe)
                        .0
                        .get(&slot)
                        .map(|m| matcher.phrase(m))
                        .unwrap_or_default();
                    // Only a value that is still among the live candidates counts.
                    let remembered = store
                        .as_ref()
                        .and_then(|s| s.preference(&recipe, &slot, &mention))
                        .and_then(|i| match &i.learned {
                            Learned::Preference { key, .. } => candidates
                                .iter()
                                .find(|c| &c.value.key == key)
                                .map(|c| (i.id, i.describe(), c.value.clone())),
                            _ => None,
                        });
                    if let Some((id, what, value)) = remembered {
                        answers.values.insert(slot, vec![value]);
                        used_learned.push((id, what));
                        if let Some(s) = store.as_mut() {
                            let _ = s.used(id);
                        }
                        continue;
                    }
                    let labels = candidates.iter().map(|c| c.value.display.clone()).collect();
                    let values = candidates.into_iter().map(|c| c.value).collect();
                    let pick = Pick::Value {
                        recipe,
                        slot: slot.clone(),
                        mention,
                        values,
                    };
                    (format!("Which {slot}?"), labels, pick)
                }
                Interpretation::NotFound { reason, .. } => {
                    ui::fail(&reason);
                    return ExitCode::FAILURE;
                }
                Interpretation::Unsupported { reason } => {
                    let related = sancho_facts::related::related_programs(sys.as_ref(), &text, 5);
                    if related.is_empty() {
                        ui::fail(&reason);
                        return ExitCode::FAILURE;
                    }
                    ui::info(&reason);
                    let opts: Vec<Opt> = related
                        .into_iter()
                        .map(|r| Opt::Explain(r.program, r.description))
                        .collect();
                    let labels = opts.iter().map(|o| o.label(lib)).collect();
                    (
                        "These programs on your system look related. Explain one?".to_string(),
                        labels,
                        Pick::Options(opts),
                    )
                }
            };
        if !interactive {
            let _ = ask::write_options(
                &mut io::stdout(),
                sancho_core::style::stdout_painter().as_ref(),
                &question,
                &labels,
            );
            ui::fail("this needs an answer; run it in a terminal or be more specific.");
            return ExitCode::from(NEEDS_ANSWER);
        }
        let mut stdin = io::stdin().lock();
        match ask::choose(
            &mut stdin,
            &mut io::stdout(),
            sancho_core::style::stdout_painter().as_ref(),
            &question,
            &labels,
        ) {
            Ok(Some(i)) => {
                if let Some(program) = pick.explains(i) {
                    return explain_with(lib, sys.as_ref(), &program);
                }
                if let (Some(s), Some(learned)) = (store.as_mut(), pick.learning(i, &phrase)) {
                    let what = sancho_learn::Item {
                        id: 0,
                        learned: learned.clone(),
                        uses: 0,
                    }
                    .describe();
                    match s.learn(learned) {
                        Ok(id) => ui::info(&format!(
                            "Remembered: {what} (forget it: sancho learned rm {id})"
                        )),
                        Err(e) => ui::warn(&format!("couldn't remember that answer: {e}")),
                    }
                }
                pick.apply(i, &mut answers)
            }
            _ => {
                ui::info("Cancelled; nothing was changed.");
                return ExitCode::SUCCESS;
            }
        }
    }
}

/// What the options in a question stand for.
enum Pick {
    Options(Vec<Opt>),
    Value {
        recipe: String,
        slot: String,
        /// The user's words for the slot, normalized.
        mention: String,
        values: Vec<sancho_resolve::Value>,
    },
}

/// One option offered when sancho isn't sure.
enum Opt {
    Recipe(String),
    /// A related program (name, man page description) to explain.
    Explain(String, String),
}

impl Opt {
    fn label(&self, lib: &Library) -> String {
        match self {
            Opt::Recipe(id) => lib.recipes[id].summary.clone(),
            Opt::Explain(program, description) => format!("Explain `{program}`: {description}"),
        }
    }
}

impl Pick {
    /// The program to explain, if option `i` is one.
    fn explains(&self, i: usize) -> Option<String> {
        match self {
            Pick::Options(opts) => match opts.get(i)? {
                Opt::Explain(program, _) => Some(program.clone()),
                Opt::Recipe(_) => None,
            },
            Pick::Value { .. } => None,
        }
    }

    /// What to remember about picking option `i`.
    fn learning(&self, i: usize, phrase: &str) -> Option<Learned> {
        match self {
            Pick::Options(opts) => match opts.get(i)? {
                Opt::Recipe(id) => Some(Learned::Alias {
                    phrase: phrase.to_string(),
                    recipe: id.clone(),
                }),
                Opt::Explain(..) => None,
            },
            Pick::Value {
                recipe,
                slot,
                mention,
                values,
            } => {
                let v = values.get(i)?;
                Some(Learned::Preference {
                    recipe: recipe.clone(),
                    slot: slot.clone(),
                    mention: mention.clone(),
                    key: v.key.clone(),
                    display: v.display.clone(),
                })
            }
        }
    }

    fn apply(self, i: usize, answers: &mut Answers) {
        match self {
            Pick::Options(opts) => {
                if let Some(Opt::Recipe(id)) = opts.into_iter().nth(i) {
                    answers.recipe = Some(id);
                }
            }
            Pick::Value { slot, values, .. } => {
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
    print!(
        "{}",
        sancho_explain::render::explanation_with(&e, sancho_core::style::stdout_painter().as_ref())
    );
    ExitCode::SUCCESS
}

/// Runs a confirmed plan through the journal; offers rollback on failure.
fn run_plan(plan: &sancho_core::Plan, request: &str) -> ExitCode {
    use sancho_journal::{apply_undo, execute, plan_undo, RunOutcome};

    let journal = match history::journal() {
        Ok(j) => j,
        Err(e) => {
            ui::fail(&e.to_string());
            return ExitCode::FAILURE;
        }
    };
    let mut ex = sancho_exec::SudoExecutor;
    let total = plan.steps.len();
    let paint = sancho_core::style::stdout_painter();
    let mut progress = |i: usize| {
        let counter = format!("[{}/{total}]", i + 1);
        println!(
            "{} {}",
            paint.paint(sancho_core::style::Tone::Accent, &counter),
            paint.paint(sancho_core::style::Tone::Title, &plan.steps[i].explanation)
        )
    };
    match execute(&journal, &mut ex, plan, request, &mut progress) {
        Ok(RunOutcome::Completed { tx }) => {
            if plan.steps.iter().any(|s| !s.read_only) {
                ui::done(&format!("Done. `sancho undo` reverses this ({tx})."));
            }
            ExitCode::SUCCESS
        }
        Ok(RunOutcome::Failed { tx, step, exit }) => {
            ui::fail(&format!(
                "Step {} failed (exit {exit}); later steps did not run.",
                step + 1
            ));
            if step == 0 {
                return ExitCode::FAILURE;
            }
            let undo = match plan_undo(&journal, &mut ex, std::slice::from_ref(&tx), None) {
                Ok(u) => u,
                Err(e) => {
                    ui::fail(&e.to_string());
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
                sancho_core::style::stdout_painter().as_ref(),
                sancho_core::plan::Confirmation::YesNo,
            )
            .unwrap_or(false);
            if yes {
                match apply_undo(&journal, &mut ex, &undo, &format!("roll back {tx}"), false) {
                    Ok(RunOutcome::Completed { .. }) => ui::done("Rolled back."),
                    Ok(RunOutcome::Failed { step, .. }) => ui::fail(&format!(
                        "Rollback step {} failed; see `sancho history`.",
                        step + 1
                    )),
                    Err(e) => ui::fail(&e.to_string()),
                }
            } else {
                ui::info("Left as is; `sancho undo` can roll it back later.");
            }
            ExitCode::FAILURE
        }
        Err(e) => {
            ui::fail(&e.to_string());
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

fn print_profile(sys: &dyn System) {
    let profile = detect(sys);
    for (key, value) in profile.iter() {
        let paint = sancho_core::style::stdout_painter();
        println!(
            "{}{}",
            paint.paint(
                sancho_core::style::Tone::Muted,
                &format!("{:<14}", key.name())
            ),
            paint.paint(sancho_core::style::Tone::Title, value)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

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
