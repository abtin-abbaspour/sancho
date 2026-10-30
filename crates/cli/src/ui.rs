//! How sancho talks: every message goes through here so the terminal output
//! is consistent. On a terminal, messages are marked (✓ done, ✗ problem,
//! · information, ! caution) and coloured; otherwise they are plain text,
//! with errors prefixed "sancho:" as scripts expect.

use std::io::Write;

use sancho_core::style::{stderr_painter, stdout_painter, Tone};

/// Something that worked.
pub fn done(msg: &str) {
    let p = stdout_painter();
    if p.styled() {
        println!("{} {msg}", p.paint(Tone::Good, "✓"));
    } else {
        println!("{msg}");
    }
}

/// Neutral information.
pub fn info(msg: &str) {
    let p = stdout_painter();
    if p.styled() {
        println!(
            "{} {}",
            p.paint(Tone::Muted, "·"),
            p.paint(Tone::Muted, msg)
        );
    } else {
        println!("{msg}");
    }
}

/// Something sancho couldn't do, or won't do without more input.
pub fn fail(msg: &str) {
    to_stderr(Tone::Bad, "✗", msg);
}

/// A caution that doesn't stop anything.
pub fn warn(msg: &str) {
    to_stderr(Tone::Warn, "!", msg);
}

fn to_stderr(tone: Tone, mark: &str, msg: &str) {
    let p = stderr_painter();
    let mut err = std::io::stderr();
    let _ = if p.styled() {
        writeln!(err, "{} {}", p.paint(tone, mark), p.paint(tone, msg))
    } else {
        writeln!(err, "sancho: {msg}")
    };
}
