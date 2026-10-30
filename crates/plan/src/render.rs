//! Plain-text rendering of a plan for confirmation. Every line comes from a
//! template filled with real values; commands are shown exactly as they run.

use std::fmt::Write;

use sancho_core::effect::EffectKind;
use sancho_core::plan::Confirmation;
use sancho_core::{Plan, PlanStep, Reversibility, Risk, Tier};

pub fn plan(p: &Plan) -> String {
    let mut out = String::new();
    let tier = match p.tier {
        Tier::VerifiedRecipe => "verified recipe",
        Tier::CheckedCommand => "checked command",
        Tier::Unverified => "UNVERIFIED",
    };
    let source = match (&p.recipe_id, p.recipe_version) {
        (Some(id), Some(v)) => format!(" · {id} v{v}"),
        _ => String::new(),
    };
    let _ = writeln!(out, "[{tier}{source}] {}", p.summary);
    let _ = writeln!(out, "{}", risk_line(p));
    out.push('\n');

    let snapshot_step = p
        .steps
        .iter()
        .position(|s| s.effects.iter().any(|e| e.kind == EffectKind::Snapshot))
        .map(|i| i + 1);
    for (i, step) in p.steps.iter().enumerate() {
        step_text(&mut out, i + 1, step, snapshot_step);
    }

    if !p.skipped.is_empty() {
        out.push_str("Skipped:\n");
        for s in &p.skipped {
            let _ = writeln!(out, "  - {} ({})", s.explanation, s.reason);
        }
        out.push('\n');
    }
    if !p.learned.is_empty() {
        out.push_str("Learned from your earlier answers:\n");
        for (id, what) in &p.learned {
            let _ = writeln!(out, "  - {what} (forget it: sancho learned rm {id})");
        }
        out.push('\n');
    }
    if !p.notes.is_empty() {
        out.push_str("Notes:\n");
        for n in &p.notes {
            let _ = writeln!(out, "  - {n}");
        }
        out.push('\n');
    }
    let _ = writeln!(out, "Undo: {}", overall_undo(p));
    out
}

fn risk_line(p: &Plan) -> String {
    let risk = match p.risk {
        Risk::Low => "low",
        Risk::Medium => "medium",
        Risk::High => "HIGH",
    };
    let how = match p.confirmation() {
        Confirmation::None => "read-only, runs without asking",
        Confirmation::YesNo => "you will be asked to confirm",
        Confirmation::TypeYes => "you will be asked to type \"yes\" to confirm",
    };
    format!("Risk: {risk} — {how}")
}

fn step_text(out: &mut String, n: usize, step: &PlanStep, snapshot_step: Option<usize>) {
    let learned = if step.learned { " [learned]" } else { "" };
    let _ = writeln!(out, "  {n}. {}{learned}", step.explanation);
    let sudo = if step.needs_root { "sudo " } else { "" };
    let _ = writeln!(out, "     $ {sudo}{}", shell_words(&step.argv));
    if let Some(input) = &step.stdin {
        out.push_str("     with this content:\n");
        for line in input.lines() {
            let _ = writeln!(out, "     | {line}");
        }
    }
    let undo = if step.read_only {
        "read-only".to_string()
    } else {
        undo_text(step.reversibility, snapshot_step)
    };
    let _ = writeln!(out, "     {undo}");
    out.push('\n');
}

fn undo_text(r: Reversibility, snapshot_step: Option<usize>) -> String {
    match r {
        Reversibility::NoUndoNeeded => "nothing to undo".into(),
        Reversibility::Reversible => "can be undone".into(),
        Reversibility::ReversibleWhileCached => {
            "can be undone while the package files stay in the pacman cache".into()
        }
        Reversibility::SnapshotOnly => match snapshot_step {
            Some(n) => format!("can be undone only by rolling back the snapshot from step {n}"),
            None => "can be undone only by rolling back a snapshot".into(),
        },
        Reversibility::Irreversible => "CANNOT be undone".into(),
    }
}

fn overall_undo(p: &Plan) -> &'static str {
    match p.reversibility() {
        Reversibility::NoUndoNeeded => "nothing to undo",
        Reversibility::Reversible => "every step can be undone with `sancho undo`",
        Reversibility::ReversibleWhileCached => {
            "`sancho undo` can reverse this while the package files stay in the cache"
        }
        Reversibility::SnapshotOnly => "only by rolling back the snapshot taken first",
        Reversibility::Irreversible => "some steps CANNOT be undone (see above)",
    }
}

/// Displays argv the way a shell user would type it. For display only:
/// commands are executed as argument vectors, never through a shell.
pub fn shell_words(argv: &[String]) -> String {
    argv.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

fn quote(a: &str) -> String {
    let plain = !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+,%".contains(c));
    if plain {
        a.to_string()
    } else {
        format!("'{}'", a.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_is_shell_accurate() {
        let v = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            shell_words(&v(&["pacman", "-S", "--needed", "vlc"])),
            "pacman -S --needed vlc"
        );
        assert_eq!(
            shell_words(&v(&["snapper", "--description", "sancho: before changes"])),
            "snapper --description 'sancho: before changes'"
        );
        assert_eq!(shell_words(&v(&["echo", "it's", ""])), r"echo 'it'\''s' ''");
    }
}
