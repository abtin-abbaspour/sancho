//! Plain-text rendering of an explanation.

use std::fmt::Write;

use sancho_core::Risk;

use crate::{Explanation, ItemKind};

pub fn explanation(e: &Explanation) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "$ {}\n", e.source.trim());
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
            format!(" ({})", tags.join(", "))
        };
        let summary = match (&c.summary, c.known) {
            (Some(s), true) => s.clone(),
            (Some(s), false) => {
                format!("{s} [from its man page; sancho has no notes on its options]")
            }
            (None, _) => "?? sancho has no notes on this program and found no man page".into(),
        };
        let _ = writeln!(out, "{}{tags}: {summary}", c.program);
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
            let mark = match i.kind {
                ItemKind::Unknown => "?? ",
                _ => "",
            };
            let risk = match i.risk {
                Some(Risk::High) => " [high risk]",
                _ => "",
            };
            let _ = writeln!(out, "  {:<width$}  {mark}{}{risk}", i.text, i.help);
        }
        out.push('\n');
    }
    if !e.notes.is_empty() {
        for n in &e.notes {
            let mark = if n.risk == Risk::High { "! " } else { "- " };
            let _ = writeln!(out, "{mark}{}", n.text);
        }
        out.push('\n');
    }
    if e.incomplete {
        out.push_str("Parts of this could not be parsed; the explanation may be incomplete.\n");
    }
    let risk = match e.risk() {
        Some(Risk::Low) => "low".to_string(),
        Some(Risk::Medium) => "medium".to_string(),
        Some(Risk::High) => "HIGH".to_string(),
        None => "unknown (something above is not in sancho's notes)".to_string(),
    };
    let _ = writeln!(out, "Risk: {risk}");
    out
}
