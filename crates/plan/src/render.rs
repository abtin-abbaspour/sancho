//! Rendering of a plan for confirmation. Every line comes from a template
//! filled with real values; commands are shown exactly as they run.
//! `plan` is plain text (tests, pipes); `plan_with` styles it for a terminal.

use std::fmt::Write;

use sancho_core::effect::EffectKind;
use sancho_core::plan::Confirmation;
use sancho_core::style::{Paint, Plain, Tone};
use sancho_core::{Plan, PlanStep, Reversibility, Risk, Tier};

pub fn plan(p: &Plan) -> String {
    plan_with(p, &Plain)
}

pub fn plan_with(p: &Plan, paint: &dyn Paint) -> String {
    let mut out = String::new();
    let tier = match p.tier {
        Tier::VerifiedRecipe => "verified recipe",
        Tier::CheckedCommand => "checked command",
        Tier::Unverified => "UNVERIFIED",
    };
    let source = match (&p.recipe_id, p.recipe_version) {
        (Some(id), Some(v)) => format!("{tier} · {id} v{v}"),
        _ => tier.to_string(),
    };
    let _ = writeln!(out, "{}", paint.paint(Tone::Title, &p.summary));
    let _ = writeln!(out, "{}", paint.paint(Tone::Muted, &source));
    let _ = writeln!(out, "{}", risk_line(p, paint));
    out.push('\n');

    let snapshot_step = p
        .steps
        .iter()
        .position(|s| s.effects.iter().any(|e| e.kind == EffectKind::Snapshot))
        .map(|i| i + 1);
    for (i, step) in p.steps.iter().enumerate() {
        step_text(&mut out, i + 1, step, snapshot_step, paint);
    }

    let section = |out: &mut String, title: &str, tone: Tone, lines: Vec<String>| {
        if lines.is_empty() {
            return;
        }
        let _ = writeln!(out, "{}", paint.paint(tone, title));
        for l in lines {
            let _ = writeln!(out, "  - {l}");
        }
        out.push('\n');
    };
    section(
        &mut out,
        "Skipped",
        Tone::Muted,
        p.skipped
            .iter()
            .map(|s| {
                format!(
                    "{} {}",
                    s.explanation,
                    paint.paint(Tone::Muted, &format!("({})", s.reason))
                )
            })
            .collect(),
    );
    section(
        &mut out,
        "Learned from your earlier answers",
        Tone::Accent,
        p.learned
            .iter()
            .map(|(id, what)| {
                format!(
                    "{what} {}",
                    paint.paint(Tone::Muted, &format!("(forget it: sancho learned rm {id})"))
                )
            })
            .collect(),
    );
    section(&mut out, "Notes", Tone::Warn, p.notes.clone());
    let (tone, undo) = overall_undo(p);
    let _ = writeln!(out, "Undo: {}", paint.paint(tone, undo));
    out
}

fn risk_line(p: &Plan, paint: &dyn Paint) -> String {
    let (tone, risk) = match p.risk {
        Risk::Low => (Tone::Good, "low"),
        Risk::Medium => (Tone::Warn, "medium"),
        Risk::High => (Tone::Bad, "HIGH"),
    };
    let how = match p.confirmation() {
        Confirmation::None => "read-only, runs without asking",
        Confirmation::YesNo => "you will be asked to confirm",
        Confirmation::TypeYes => "you will be asked to type \"yes\" to confirm",
    };
    format!("Risk: {} — {how}", paint.paint(tone, risk))
}

fn step_text(
    out: &mut String,
    n: usize,
    step: &PlanStep,
    snapshot_step: Option<usize>,
    paint: &dyn Paint,
) {
    let learned = if step.learned {
        paint.paint(Tone::Accent, " [learned]")
    } else {
        String::new()
    };
    let _ = writeln!(out, "  {}. {}{learned}", n, step.explanation);
    let sudo = if step.needs_root { "sudo " } else { "" };
    let _ = writeln!(
        out,
        "     {} {}",
        paint.paint(Tone::Muted, "$"),
        paint.paint(Tone::Command, &format!("{sudo}{}", shell_words(&step.argv)))
    );
    if let Some(input) = &step.stdin {
        let _ = writeln!(
            out,
            "     {}",
            paint.paint(Tone::Muted, "with this content:")
        );
        for line in input.lines() {
            let _ = writeln!(
                out,
                "     {} {}",
                paint.paint(Tone::Muted, "|"),
                paint.paint(Tone::Command, line)
            );
        }
    }
    let (tone, undo) = if step.read_only {
        (Tone::Muted, "· read-only".to_string())
    } else {
        undo_text(step.reversibility, snapshot_step)
    };
    let _ = writeln!(out, "     {}", paint.paint(tone, &undo));
    out.push('\n');
}

fn undo_text(r: Reversibility, snapshot_step: Option<usize>) -> (Tone, String) {
    match r {
        Reversibility::NoUndoNeeded => (Tone::Muted, "· nothing to undo".into()),
        Reversibility::Reversible => (Tone::Good, "↺ can be undone".into()),
        Reversibility::ReversibleWhileCached => (
            Tone::Good,
            "↺ can be undone while the package files stay in the pacman cache".into(),
        ),
        Reversibility::SnapshotOnly => (
            Tone::Warn,
            match snapshot_step {
                Some(n) => {
                    format!("⚠ can be undone only by rolling back the snapshot from step {n}")
                }
                None => "⚠ can be undone only by rolling back a snapshot".into(),
            },
        ),
        Reversibility::Irreversible => (Tone::Bad, "✗ CANNOT be undone".into()),
    }
}

fn overall_undo(p: &Plan) -> (Tone, &'static str) {
    match p.reversibility() {
        Reversibility::NoUndoNeeded => (Tone::Muted, "nothing to undo"),
        Reversibility::Reversible => (Tone::Good, "every step can be undone with `sancho undo`"),
        Reversibility::ReversibleWhileCached => (
            Tone::Good,
            "`sancho undo` can reverse this while the package files stay in the cache",
        ),
        Reversibility::SnapshotOnly => {
            (Tone::Warn, "only by rolling back the snapshot taken first")
        }
        Reversibility::Irreversible => (Tone::Bad, "some steps CANNOT be undone (see above)"),
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
