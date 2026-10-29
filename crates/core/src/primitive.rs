//! Command primitives: one fixed command line each (`data/primitives/*.toml`).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::effect::EffectSpec;
use crate::template::{is_ident, Template};
use crate::validate::{check_id, Problems};
use crate::value::{Risk, ValueType};

/// A command schema. `command` is the exact argv; each arg appears in it once
/// as a whole-token `{arg}` placeholder, except a `stdin` arg.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Primitive {
    pub id: String,
    pub summary: String,
    pub command: Vec<Template>,
    /// Arch package that provides the command, when fixed.
    #[serde(default)]
    pub package: Option<String>,
    pub needs_root: bool,
    /// Needs a PTY so the command can ask the user questions.
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub args: BTreeMap<String, ArgSpec>,
    pub risk: Risk,
    /// What the command changes. Empty exactly when `read_only`.
    #[serde(default)]
    pub effects: Vec<EffectSpec>,
    pub read_only: bool,
    /// Same command in show-only mode, if the tool has one.
    #[serde(default)]
    pub dry_run: Option<Vec<Template>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgSpec {
    #[serde(rename = "type")]
    pub ty: ValueType,
    pub help: String,
    /// Allowed values for `type = "choice"`.
    #[serde(default)]
    pub values: Vec<String>,
    /// Expands to one argv element per value.
    #[serde(default)]
    pub variadic: bool,
    /// Passed on stdin instead of argv (required for `text`).
    #[serde(default)]
    pub stdin: bool,
}

impl Primitive {
    /// Checks everything that can be checked without other files.
    pub fn validate(&self, p: &mut Problems) {
        check_id(p, "id", &self.id);
        if self.summary.trim().is_empty() {
            p.push("summary", "must not be empty");
        }

        for (name, arg) in &self.args {
            let field = format!("args.{name}");
            if !is_ident(name) {
                p.push(&field, "arg names must be lowercase identifiers");
            }
            if arg.help.trim().is_empty() {
                p.push(format!("{field}.help"), "must not be empty");
            }
            check_values(p, &field, arg.ty, &arg.values);
            if (arg.ty == ValueType::Text) != arg.stdin {
                p.push(&field, "text args must use stdin, and only text args can");
            }
            if arg.stdin && arg.variadic {
                p.push(&field, "a stdin arg cannot be variadic");
            }
            let uses = self
                .command_refs(&self.command)
                .filter(|n| n == name)
                .count();
            match (arg.stdin, uses) {
                (true, 0) | (false, 1) => {}
                (true, _) => p.push(&field, "a stdin arg must not appear in command"),
                (false, 0) => p.push(&field, "arg is not used in command"),
                (false, _) => p.push(&field, "arg appears in command more than once"),
            }
        }
        if self.args.values().filter(|a| a.stdin).count() > 1 {
            p.push("args", "at most one arg may use stdin");
        }
        if self.args.values().filter(|a| a.variadic).count() > 1 {
            p.push("args", "at most one arg may be variadic");
        }

        self.validate_command(p, "command", &self.command);
        if let Some(dry) = &self.dry_run {
            self.validate_command(p, "dry_run", dry);
            let mut a: Vec<_> = self.command_refs(&self.command).collect();
            let mut b: Vec<_> = self.command_refs(dry).collect();
            a.sort();
            b.sort();
            if a != b {
                p.push("dry_run", "must use the same args as command");
            }
        }

        for (i, effect) in self.effects.iter().enumerate() {
            for r in effect.target.refs() {
                if r.attr.is_some() || !self.args.contains_key(&r.name) {
                    p.push(
                        format!("effects[{i}].target"),
                        format!("{r} is not an argument of this primitive"),
                    );
                }
            }
        }
        match (self.read_only, self.effects.is_empty()) {
            (true, false) => p.push("effects", "read_only primitives must not declare effects"),
            (false, true) => p.push(
                "effects",
                "primitives that change the system must declare at least one effect",
            ),
            _ => {}
        }
    }

    fn validate_command(&self, p: &mut Problems, field: &str, command: &[Template]) {
        if command.is_empty() {
            p.push(field, "must not be empty");
        }
        for (i, token) in command.iter().enumerate() {
            let field = format!("{field}[{i}]");
            if let Some(lit) = token.as_literal() {
                if lit.is_empty() {
                    p.push(field, "must not be empty");
                }
                continue;
            }
            let Some(r) = token.as_single_ref() else {
                p.push(
                    field,
                    "a placeholder must be the whole token (no text around it)",
                );
                continue;
            };
            match self.args.get(&r.name) {
                Some(arg) if r.attr.is_none() => {
                    if i == 0 && (arg.ty != ValueType::Choice || arg.variadic) {
                        p.push(field, "the program name may only come from a choice arg");
                    }
                }
                _ => p.push(field, format!("{r} is not an argument of this primitive")),
            }
        }
    }

    fn command_refs<'a>(&self, command: &'a [Template]) -> impl Iterator<Item = &'a str> {
        command
            .iter()
            .flat_map(|t| t.refs())
            .map(|r| r.name.as_str())
    }
}

pub(crate) fn check_values(p: &mut Problems, field: &str, ty: ValueType, values: &[String]) {
    if ty == ValueType::Choice {
        if values.is_empty() {
            p.push(
                format!("{field}.values"),
                "a choice needs at least one value",
            );
        }
        for (i, v) in values.iter().enumerate() {
            if v.is_empty() || v.starts_with('-') || v.contains(char::is_whitespace) {
                p.push(
                    format!("{field}.values[{i}]"),
                    "values must be non-empty, contain no spaces and not start with '-'",
                );
            }
            if values[..i].contains(v) {
                p.push(
                    format!("{field}.values[{i}]"),
                    format!("duplicate value '{v}'"),
                );
            }
        }
    } else if !values.is_empty() {
        p.push(
            format!("{field}.values"),
            "values are only allowed for type = \"choice\"",
        );
    }
}
