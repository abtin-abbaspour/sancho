//! Notes on programs, for explaining command lines (`data/commands/*.toml`).
//! Only what is written here is explained; everything else is reported as
//! unknown, never guessed.

use serde::Deserialize;

use crate::validate::Problems;
use crate::value::{safe_token, Risk};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDoc {
    /// The program name as typed (`pacman`).
    pub command: String,
    pub summary: String,
    /// Risk of running it at all; flags and subcommands may raise it.
    pub risk: Risk,
    /// Runs another command given after its own flags (sudo, env, nice).
    #[serde(default)]
    pub wraps: bool,
    /// The wrapped command runs as root (sudo).
    #[serde(default)]
    pub as_root: bool,
    #[serde(default)]
    pub subcommands: Vec<SubcommandDoc>,
    #[serde(default)]
    pub flags: Vec<FlagDoc>,
    /// Arguments that make this command especially dangerous (`rm` with `/`).
    #[serde(default)]
    pub dangerous_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubcommandDoc {
    /// The words that select it, e.g. `["radio", "wifi"]`.
    pub words: Vec<String>,
    pub help: String,
    #[serde(default)]
    pub risk: Option<Risk>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlagDoc {
    /// Every spelling: `["-S", "--sync"]`.
    pub names: Vec<String>,
    pub help: String,
    /// Takes a value (`-k 2`, `-k2`, `--keep=2`).
    #[serde(default)]
    pub value: bool,
    #[serde(default)]
    pub risk: Option<Risk>,
}

impl CommandDoc {
    pub fn flag(&self, name: &str) -> Option<&FlagDoc> {
        self.flags
            .iter()
            .find(|f| f.names.iter().any(|n| n == name))
    }

    pub fn validate(&self, p: &mut Problems) {
        if !safe_token(&self.command, "._+-", 64) {
            p.push("command", "must be a program name");
        }
        if self.summary.trim().is_empty() {
            p.push("summary", "must not be empty");
        }
        if self.as_root && !self.wraps {
            p.push(
                "as_root",
                "only wrapper commands can run their command as root",
            );
        }
        let mut seen: Vec<&str> = Vec::new();
        for (i, f) in self.flags.iter().enumerate() {
            let field = format!("flags[{i}]");
            if f.names.is_empty() {
                p.push(&field, "needs at least one name");
            }
            for n in &f.names {
                let ok = n.starts_with('-')
                    && n.len() > 1
                    && !n.contains(|c: char| c.is_whitespace() || c == '=');
                if !ok {
                    p.push(
                        format!("{field}.names"),
                        format!("'{n}' is not a flag name"),
                    );
                }
                if seen.contains(&n.as_str()) {
                    p.push(
                        format!("{field}.names"),
                        format!("'{n}' is documented twice"),
                    );
                }
                seen.push(n);
            }
            if f.help.trim().is_empty() {
                p.push(format!("{field}.help"), "must not be empty");
            }
        }
        for (i, s) in self.subcommands.iter().enumerate() {
            let field = format!("subcommands[{i}]");
            if s.words.is_empty() || s.words.iter().any(|w| w.is_empty() || w.starts_with('-')) {
                p.push(
                    format!("{field}.words"),
                    "must be one or more words not starting with '-'",
                );
            }
            if s.help.trim().is_empty() {
                p.push(format!("{field}.help"), "must not be empty");
            }
        }
    }
}
