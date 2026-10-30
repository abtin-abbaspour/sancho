//! Matching eval: runs `eval/requests.toml` against fixture systems and
//! reports intent accuracy, slot accuracy, wrong-plan rate and clarification
//! rate. Exits non-zero if any request produced a wrong plan.
//!
//! Usage: sancho-eval [eval/requests.toml] [--data data] [--verbose]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;

use sancho_core::{format_errors, FixtureSystem, Library};
use sancho_match::{interpret, Interpretation, Matcher};
use sancho_profile::detect;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvalFile {
    /// Default fixture system (under `<requests dir>/fixtures/systems`).
    system: String,
    #[serde(rename = "case")]
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    request: String,
    /// A recipe id, `"clarify"` or `"unsupported"`.
    expect: String,
    #[serde(default)]
    slots: BTreeMap<String, OneOrMany>,
    #[serde(default)]
    system: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn keys(&self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s.clone()],
            OneOrMany::Many(v) => v.clone(),
        }
    }
}

enum Outcome {
    Plan {
        recipe: String,
        slots: BTreeMap<String, Vec<String>>,
    },
    /// Asked the user something. `recipe` is set when the recipe was known.
    Clarify {
        recipe: Option<String>,
        what: String,
    },
    Unsupported(String),
}

fn outcome(i: Interpretation) -> Outcome {
    match i {
        Interpretation::Ready { recipe, values, .. } => Outcome::Plan {
            recipe,
            slots: values
                .into_iter()
                .map(|(k, vs)| (k, vs.into_iter().map(|v| v.key).collect()))
                .collect(),
        },
        Interpretation::ChooseRecipe { options, suggested } => Outcome::Clarify {
            recipe: None,
            what: format!(
                "{} {}",
                if suggested { "suggest" } else { "choose" },
                options.join(" | ")
            ),
        },
        Interpretation::ChooseValue {
            recipe,
            slot,
            candidates,
        } => Outcome::Clarify {
            what: format!(
                "{recipe}: which {slot}? {}",
                candidates
                    .iter()
                    .map(|c| c.value.key.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            recipe: Some(recipe),
        },
        Interpretation::NotFound {
            recipe,
            slot,
            reason,
        } => Outcome::Clarify {
            what: format!("{recipe}: {slot}: {reason}"),
            recipe: Some(recipe),
        },
        Interpretation::Unsupported { reason } => Outcome::Unsupported(reason),
    }
}

fn main() -> ExitCode {
    let mut requests = PathBuf::from("eval/requests.toml");
    let mut data = PathBuf::from("data");
    let mut verbose = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--verbose" | "-v" => verbose = true,
            "--data" => data = args.next().map(PathBuf::from).unwrap_or(data),
            _ => requests = PathBuf::from(a),
        }
    }
    match run(&requests, &data, verbose) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("sancho-eval: {e}");
            ExitCode::from(2)
        }
    }
}

/// Returns whether the run had zero wrong plans.
fn run(requests: &Path, data: &Path, verbose: bool) -> Result<bool, String> {
    let lib =
        Library::load(data).map_err(|e| format!("data failed to load:\n{}", format_errors(&e)))?;
    let src =
        std::fs::read_to_string(requests).map_err(|e| format!("{}: {e}", requests.display()))?;
    let file: EvalFile =
        toml::from_str(&src).map_err(|e| format!("{}: {e}", requests.display()))?;
    let systems_dir = requests
        .parent()
        .unwrap_or(Path::new("."))
        .join("fixtures/systems");
    let matcher = Matcher::new(&lib);

    for c in &file.cases {
        if !["clarify", "unsupported"].contains(&c.expect.as_str())
            && !lib.recipes.contains_key(&c.expect)
        {
            return Err(format!(
                "case {:?}: unknown recipe '{}'",
                c.request, c.expect
            ));
        }
    }

    // Every recipe needs at least this many requests that should plan it.
    const MIN_CASES: usize = 3;
    let thin: Vec<String> = lib
        .recipes
        .keys()
        .map(|id| (id, file.cases.iter().filter(|c| &c.expect == id).count()))
        .filter(|(_, n)| *n < MIN_CASES)
        .map(|(id, n)| format!("{id} ({n})"))
        .collect();
    if !thin.is_empty() {
        return Err(format!(
            "recipes need at least {MIN_CASES} eval requests: {}",
            thin.join(", ")
        ));
    }

    let mut systems: BTreeMap<String, (FixtureSystem, sancho_profile::Profile)> = BTreeMap::new();
    let mut stats = Stats::default();
    for c in &file.cases {
        let name = c.system.as_deref().unwrap_or(&file.system);
        let (sys, profile) = systems.entry(name.to_string()).or_insert_with(|| {
            let sys = FixtureSystem::new(systems_dir.join(name));
            let profile = detect(&sys);
            (sys, profile)
        });
        let got = outcome(interpret(&matcher, profile, sys, &c.request));
        let verdict = stats.score(c, &got);
        if verbose || matches!(verdict, Verdict::WrongPlan | Verdict::Missed) {
            println!(
                "{:<10} {:?}\n           expected {}, got {}",
                verdict.label(),
                c.request,
                describe_expected(c),
                describe(&got)
            );
        }
    }
    stats.report();
    Ok(stats.wrong_plans == 0)
}

#[derive(Default)]
struct Stats {
    cases: usize,
    recipe_cases: usize,
    intent_correct: usize,
    slot_total: usize,
    slot_correct: usize,
    wrong_plans: usize,
    clarifications: usize,
    clarified_recipe_cases: usize,
    correct: usize,
}

enum Verdict {
    Correct,
    /// Asked when a plan was possible; acceptable but counted.
    Clarified,
    /// A plan that differs from what was expected. Must be zero.
    WrongPlan,
    /// Not understood when a recipe was expected.
    Missed,
}

impl Verdict {
    fn label(&self) -> &'static str {
        match self {
            Verdict::Correct => "ok",
            Verdict::Clarified => "clarified",
            Verdict::WrongPlan => "WRONG PLAN",
            Verdict::Missed => "missed",
        }
    }
}

impl Stats {
    fn score(&mut self, c: &Case, got: &Outcome) -> Verdict {
        self.cases += 1;
        if matches!(got, Outcome::Clarify { .. }) {
            self.clarifications += 1;
        }
        let verdict = match c.expect.as_str() {
            "clarify" | "unsupported" => match got {
                Outcome::Plan { .. } => Verdict::WrongPlan,
                _ => Verdict::Correct,
            },
            expected => {
                self.recipe_cases += 1;
                let expected_slots: Vec<(&String, Vec<String>)> =
                    c.slots.iter().map(|(k, v)| (k, v.keys())).collect();
                self.slot_total += expected_slots.len();
                match got {
                    Outcome::Plan { recipe, slots } if recipe == expected => {
                        self.intent_correct += 1;
                        let right = expected_slots
                            .iter()
                            .filter(|(k, v)| slots.get(*k) == Some(v))
                            .count();
                        self.slot_correct += right;
                        if right == expected_slots.len() {
                            Verdict::Correct
                        } else {
                            Verdict::WrongPlan
                        }
                    }
                    Outcome::Plan { .. } => Verdict::WrongPlan,
                    Outcome::Clarify { recipe, .. } => {
                        self.clarified_recipe_cases += 1;
                        if recipe.as_deref() == Some(expected) {
                            self.intent_correct += 1;
                        }
                        Verdict::Clarified
                    }
                    Outcome::Unsupported(_) => Verdict::Missed,
                }
            }
        };
        match verdict {
            Verdict::Correct => self.correct += 1,
            Verdict::WrongPlan => self.wrong_plans += 1,
            _ => {}
        }
        verdict
    }

    fn report(&self) {
        let pct = |n: usize, d: usize| {
            if d == 0 {
                0.0
            } else {
                100.0 * n as f64 / d as f64
            }
        };
        println!();
        println!("requests            {}", self.cases);
        println!(
            "correct             {} ({:.1}%)",
            self.correct,
            pct(self.correct, self.cases)
        );
        println!(
            "intent accuracy     {:.1}% ({}/{} recipe requests)",
            pct(self.intent_correct, self.recipe_cases),
            self.intent_correct,
            self.recipe_cases
        );
        println!(
            "slot accuracy       {:.1}% ({}/{})",
            pct(self.slot_correct, self.slot_total),
            self.slot_correct,
            self.slot_total
        );
        println!(
            "wrong-plan rate     {:.1}% ({})",
            pct(self.wrong_plans, self.cases),
            self.wrong_plans
        );
        println!(
            "clarification rate  {:.1}% of all requests; {:.1}% of requests that had a recipe",
            pct(self.clarifications, self.cases),
            pct(self.clarified_recipe_cases, self.recipe_cases)
        );
    }
}

fn describe_expected(c: &Case) -> String {
    let slots: Vec<String> = c
        .slots
        .iter()
        .map(|(k, v)| format!("{k}={}", v.keys().join("+")))
        .collect();
    if slots.is_empty() {
        c.expect.clone()
    } else {
        format!("{} [{}]", c.expect, slots.join(", "))
    }
}

fn describe(o: &Outcome) -> String {
    match o {
        Outcome::Plan { recipe, slots } => {
            let s: Vec<String> = slots
                .iter()
                .map(|(k, v)| format!("{k}={}", v.join("+")))
                .collect();
            format!("plan {recipe} [{}]", s.join(", "))
        }
        Outcome::Clarify { what, .. } => format!("clarify ({what})"),
        Outcome::Unsupported(r) => format!("unsupported ({r})"),
    }
}
