//! Turns recipe + slots + profile into a Plan with risk and reversibility.
//!
//! Every argv element comes from a primitive's fixed command line, a typed
//! value resolved from the system, or a literal the recipe declared and the
//! loader checked. Nothing here invents a value.

pub mod render;

use std::collections::BTreeMap;
use std::fmt;

use sancho_core::effect::EffectKind;
use sancho_core::plan::{PlannedEffect, SkippedStep};
use sancho_core::recipe::{ProfileKey, PROFILE};
use sancho_core::template::{Part, Template};
use sancho_core::{Library, Plan, PlanStep, Reversibility, System, Tier, ValueType};
use sancho_profile::Profile;
use sancho_resolve::{validate, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    UnknownRecipe(String),
    /// A recipe slot has no value.
    MissingValue(String),
    /// A required command is not installed.
    NotInstalled {
        program: String,
        package: Option<String>,
    },
    /// A system object the recipe names does not exist here.
    Missing {
        what: ValueType,
        name: String,
    },
    /// A system query needed to check the plan failed.
    Unavailable(String),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::UnknownRecipe(id) => write!(f, "unknown recipe '{id}'"),
            PlanError::MissingValue(slot) => write!(f, "no value for '{slot}'"),
            PlanError::NotInstalled { program, package } => match package {
                Some(p) => write!(f, "`{program}` is not installed; it comes with {p}"),
                None => write!(f, "`{program}` is not installed"),
            },
            PlanError::Missing { what, name } => {
                write!(f, "{what} '{name}' does not exist on this system")
            }
            PlanError::Unavailable(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// Values a recipe's templates are filled from.
struct Context<'a> {
    values: &'a BTreeMap<String, Vec<Value>>,
    profile: &'a Profile,
}

impl Context<'_> {
    /// Values of one `{ref}`: slot keys or attributes, or a profile fact.
    fn lookup(&self, name: &str, attr: Option<&str>) -> Result<Vec<String>, PlanError> {
        if name == PROFILE {
            let key = attr
                .and_then(ProfileKey::from_name)
                .expect("checked at load time");
            return Ok(vec![self.profile.get(key).to_string()]);
        }
        let vals = self
            .values
            .get(name)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| PlanError::MissingValue(name.to_string()))?;
        Ok(vals
            .iter()
            .map(|v| match attr {
                Some(a) => v
                    .attrs
                    .get(a)
                    .cloned()
                    .expect("attributes checked at load time"),
                None => v.key.clone(),
            })
            .collect())
    }

    /// A template as argv values: a lone reference to a multi-value slot
    /// expands to one value per item; anything else is one string.
    fn values(&self, t: &Template) -> Result<Vec<String>, PlanError> {
        if let Some(r) = t.as_single_ref() {
            return self.lookup(&r.name, r.attr.as_deref());
        }
        Ok(vec![self.text(t, |v| v.join(" "))?])
    }

    /// A template as one string, joining multi-values with `join`.
    fn text(&self, t: &Template, join: impl Fn(&[String]) -> String) -> Result<String, PlanError> {
        let mut out = String::new();
        for part in &t.parts {
            match part {
                Part::Literal(s) => out.push_str(s),
                Part::Ref(r) => out.push_str(&join(&self.lookup(&r.name, r.attr.as_deref())?)),
            }
        }
        Ok(out)
    }
}

/// "a", "a and b", "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

pub fn build(
    lib: &Library,
    recipe_id: &str,
    values: &BTreeMap<String, Vec<Value>>,
    profile: &Profile,
    sys: &dyn System,
) -> Result<Plan, PlanError> {
    let recipe = lib
        .recipes
        .get(recipe_id)
        .ok_or_else(|| PlanError::UnknownRecipe(recipe_id.to_string()))?;
    let ctx = Context { values, profile };
    let mut steps: Vec<PlanStep> = Vec::new();
    let mut skipped = Vec::new();

    for step in &recipe.steps {
        let prim = &lib.primitives[&step.primitive];
        let unmet = profile.unmet(&step.requires);
        if !unmet.is_empty() {
            // A step for another variant of a fact the recipe already branches
            // on (e.g. the iwd step on a NetworkManager system) is not an
            // omission; anything else left out is listed with the reason.
            let alternative = unmet.iter().all(|u| recipe.requires.contains_key(&u.key));
            if !alternative {
                // The step's own text may name the very fact that is missing
                // ("upgrade with {profile.aur_helper}"), so describe it generically.
                let why: Vec<String> = unmet.iter().map(ToString::to_string).collect();
                skipped.push(SkippedStep {
                    explanation: prim.summary.clone(),
                    reason: why.join("; "),
                });
            }
            continue;
        }
        let explanation = ctx.text(&step.explain, and_list)?;

        let mut args: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut stdin = None;
        for (name, spec) in &prim.args {
            let tmpl = &step.args[name];
            let vals = ctx.values(tmpl)?;
            // Literal names of system objects must exist here.
            if spec.ty.is_resolved() && tmpl.refs().next().is_none() {
                for v in &vals {
                    let found = validate(sys, spec.ty, v)
                        .map_err(|e| PlanError::Unavailable(e.to_string()))?;
                    if found.is_none() {
                        return Err(PlanError::Missing {
                            what: spec.ty,
                            name: v.clone(),
                        });
                    }
                }
            }
            if spec.stdin {
                stdin = vals.into_iter().next();
            } else {
                args.insert(name, vals);
            }
        }

        let mut argv = Vec::new();
        for token in &prim.command {
            match token.as_single_ref() {
                Some(r) => argv.extend(args[r.name.as_str()].iter().cloned()),
                None => argv.push(
                    token
                        .as_literal()
                        .expect("command tokens checked at load time"),
                ),
            }
        }

        if !sys.has_program(&argv[0]) {
            let err = PlanError::NotInstalled {
                program: argv[0].clone(),
                package: prim.package.clone(),
            };
            if step.optional {
                skipped.push(SkippedStep {
                    explanation,
                    reason: err.to_string(),
                });
                continue;
            }
            return Err(err);
        }

        let effects: Vec<PlannedEffect> = prim
            .effects
            .iter()
            .map(|e| {
                let mut target = String::new();
                for part in &e.target.parts {
                    match part {
                        Part::Literal(s) => target.push_str(s),
                        Part::Ref(r) => target.push_str(&args[r.name.as_str()].join(", ")),
                    }
                }
                PlannedEffect {
                    kind: e.kind,
                    target,
                }
            })
            .collect();

        let mut reversibility = effects
            .iter()
            .map(|e| e.kind.reversibility())
            .max()
            .unwrap_or(Reversibility::NoUndoNeeded);
        let snapshot_taken = steps
            .iter()
            .any(|s| s.effects.iter().any(|e| e.kind == EffectKind::Snapshot));
        if reversibility == Reversibility::SnapshotOnly && !snapshot_taken {
            reversibility = Reversibility::Irreversible;
        }

        steps.push(PlanStep {
            primitive: prim.id.clone(),
            argv,
            stdin,
            needs_root: prim.needs_root,
            interactive: prim.interactive,
            read_only: prim.read_only,
            risk: prim.risk,
            explanation,
            effects,
            reversibility,
            learned: false,
        });
    }

    Ok(Plan {
        tier: Tier::VerifiedRecipe,
        recipe_id: Some(recipe.id.clone()),
        recipe_version: Some(recipe.version),
        summary: recipe.summary.clone(),
        risk: recipe.risk,
        steps,
        skipped,
        notes: recipe.notes.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_read_naturally() {
        let s = |v: &[&str]| and_list(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(s(&["vlc"]), "vlc");
        assert_eq!(s(&["vlc", "gimp"]), "vlc and gimp");
        assert_eq!(s(&["vlc", "gimp", "obs"]), "vlc, gimp and obs");
    }
}
