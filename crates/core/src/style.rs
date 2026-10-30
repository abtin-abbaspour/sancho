//! Terminal styling. Renderers describe text by tone; the caller picks plain
//! text (tests, pipes, `NO_COLOR`) or ANSI colours (a terminal).

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// What something is: plan titles, program names.
    Title,
    /// Secondary details: recipe IDs, notes, undo classes.
    Muted,
    /// Commands exactly as they run.
    Command,
    Good,
    Warn,
    Bad,
    /// The prompt and question markers.
    Accent,
}

pub trait Paint {
    fn paint(&self, tone: Tone, text: &str) -> String;

    /// Whether output is styled; decorations such as ✓ and › are only added
    /// then, so plain output (pipes, scripts, tests) stays unchanged.
    fn styled(&self) -> bool {
        false
    }
}

/// No styling.
pub struct Plain;

impl Paint for Plain {
    fn paint(&self, _: Tone, text: &str) -> String {
        text.to_string()
    }
}

/// ANSI colours for a terminal.
pub struct Ansi;

impl Paint for Ansi {
    fn paint(&self, tone: Tone, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let code = match tone {
            Tone::Title => "1",
            Tone::Muted => "2",
            Tone::Command => "36",
            Tone::Good => "32",
            Tone::Warn => "33",
            Tone::Bad => "1;31",
            Tone::Accent => "1;35",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    fn styled(&self) -> bool {
        true
    }
}

/// Colours when writing to a terminal, unless `NO_COLOR` is set
/// (no-color.org); `CLICOLOR_FORCE` forces them on.
pub fn painter(terminal: bool) -> Box<dyn Paint> {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty() && v != "0");
    let dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");
    if set("CLICOLOR_FORCE") || (terminal && !set("NO_COLOR") && !dumb) {
        Box::new(Ansi)
    } else {
        Box::new(Plain)
    }
}

pub fn stdout_painter() -> Box<dyn Paint> {
    painter(std::io::stdout().is_terminal())
}

pub fn stderr_painter() -> Box<dyn Paint> {
    painter(std::io::stderr().is_terminal())
}
