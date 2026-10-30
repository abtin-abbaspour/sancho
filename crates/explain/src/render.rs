//! Rendering of an explanation: `explanation` is plain text, `explanation_with`
//! styles it for a terminal.

use std::fmt::Write;

use sancho_core::style::{Paint, Plain, Tone};
use sancho_core::Risk;

use crate::{Explanation, ItemKind};

pub fn explanation(e: &Explanation) -> String {
    explanation_with(e, &Plain)
}

pub fn explanation_with(e: &Explanation, paint: &dyn Paint) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} {}\n",
        paint.paint(Tone::Muted, "$"),
        paint.paint(Tone::Command, e.source.trim())
    );
    for c in &e.commands {
        let mut tags = Vec::new();
        if c.as_root {
            tags.push("as root");
        }
        if c.nested {
            tags.push("runs first, inside $(...)");
        }
        let tags = if tags.is_empty() {
            String::new()
        } else {
            paint.paint(Tone::Warn, &format!(" ({})", tags.join(", ")))
        };
        let summary = match (&c.summary, c.known) {
            (Some(s), true) => s.clone(),
            (Some(s), false) => format!(
                "{s} {}",
                paint.paint(
                    Tone::Muted,
                    "[from its man page; sancho has no notes on its options]"
                )
            ),
            (None, _) => paint.paint(
                Tone::Warn,
                "?? sancho has no notes on this program and found no man page",
            ),
        };
        let _ = writeln!(
            out,
            "{}{tags}: {summary}",
            paint.paint(Tone::Title, &c.program)
        );
        let shown: Vec<_> = c
            .items
            .iter()
            .filter(|i| i.kind != ItemKind::Argument || i.risk.is_some())
            .collect();
        let width = shown
            .iter()
            .map(|i| i.text.chars().count())
            .max()
            .unwrap_or(0)
            .min(28);
        for i in shown {
            // Pad before painting: escape codes have no width on screen.
            let text = paint.paint(Tone::Command, &format!("{:<width$}", i.text));
            let help = match i.kind {
                ItemKind::Unknown => paint.paint(Tone::Warn, &format!("?? {}", i.help)),
                _ => i.help.clone(),
            };
            let risk = match i.risk {
                Some(Risk::High) => paint.paint(Tone::Bad, " [high risk]"),
                _ => String::new(),
            };
            let _ = writeln!(out, "  {text}  {help}{risk}");
        }
        out.push('\n');
    }
    if !e.notes.is_empty() {
        for n in &e.notes {
            let line = if n.risk == Risk::High {
                paint.paint(Tone::Bad, &format!("! {}", n.text))
            } else {
                format!("{} {}", paint.paint(Tone::Muted, "-"), n.text)
            };
            let _ = writeln!(out, "{line}");
        }
        out.push('\n');
    }
    if e.incomplete {
        let _ = writeln!(
            out,
            "{}",
            paint.paint(
                Tone::Warn,
                "Parts of this could not be parsed; the explanation may be incomplete."
            )
        );
    }
    let risk = match e.risk() {
        Some(Risk::Low) => paint.paint(Tone::Good, "low"),
        Some(Risk::Medium) => paint.paint(Tone::Warn, "medium"),
        Some(Risk::High) => paint.paint(Tone::Bad, "HIGH"),
        None => paint.paint(
            Tone::Warn,
            "unknown (something above is not in sancho's notes)",
        ),
    };
    let _ = writeln!(out, "Risk: {risk}");
    out
}
