//! Deterministic command explanation from command notes (`data/commands`).
//!
//! A command line is parsed with tree-sitter-bash. Each simple command is
//! explained word by word from the notes: programs, subcommands, flags and
//! their values. Anything not in the notes is marked unknown, never guessed.
//! Shell features that change what runs (pipes into interpreters, redirects,
//! command substitution) are reported as notes with their risk.

pub mod render;

use sancho_core::command_doc::CommandDoc;
use sancho_core::{Library, Query, Risk, System};
use tree_sitter::{Node, Parser};

/// One simple command, explained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explained {
    pub program: String,
    /// From the notes, or the man page summary for programs without notes.
    pub summary: Option<String>,
    /// sancho has notes on this program.
    pub known: bool,
    pub as_root: bool,
    /// Runs inside `$(...)`, before the command that uses its output.
    pub nested: bool,
    pub items: Vec<Item>,
    /// From the notes; `None` when the program is unknown.
    pub risk: Option<Risk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub text: String,
    pub kind: ItemKind,
    pub help: String,
    pub risk: Option<Risk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Subcommand,
    Flag,
    Argument,
    /// A flag sancho has no notes on.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub risk: Risk,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Explanation {
    pub source: String,
    pub commands: Vec<Explained>,
    pub notes: Vec<Note>,
    /// The parser couldn't read all of it.
    pub incomplete: bool,
}

impl Explanation {
    /// Highest known risk, or `None` if any program or flag is unknown.
    pub fn risk(&self) -> Option<Risk> {
        if self.incomplete
            || self
                .commands
                .iter()
                .any(|c| !c.known || c.items.iter().any(|i| i.kind == ItemKind::Unknown))
        {
            return None;
        }
        let commands = self.commands.iter().filter_map(|c| c.risk);
        let items = self
            .commands
            .iter()
            .flat_map(|c| c.items.iter().filter_map(|i| i.risk));
        let notes = self.notes.iter().map(|n| n.risk);
        commands.chain(items).chain(notes).max().or(Some(Risk::Low))
    }

    pub fn unknown_flags(&self) -> Vec<&str> {
        self.commands
            .iter()
            .flat_map(|c| &c.items)
            .filter(|i| i.kind == ItemKind::Unknown)
            .map(|i| i.text.as_str())
            .collect()
    }
}

/// Programs that run text they are given as code.
const INTERPRETERS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "fish", "python", "python3", "perl", "ruby", "node",
];

/// Explains a whole command line as a shell would read it.
pub fn explain(lib: &Library, sys: &dyn System, source: &str) -> Explanation {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .expect("the bundled grammar matches the tree-sitter version");
    let mut out = Explanation {
        source: source.to_string(),
        ..Default::default()
    };
    let Some(tree) = parser.parse(source, None) else {
        out.incomplete = true;
        return out;
    };
    out.incomplete = tree.root_node().has_error();
    let mut w = Walker {
        lib,
        sys,
        src: source,
        out: &mut out,
    };
    w.node(tree.root_node(), false);
    out
}

struct Walker<'a> {
    lib: &'a Library,
    sys: &'a dyn System,
    src: &'a str,
    out: &'a mut Explanation,
}

impl Walker<'_> {
    fn text(&self, n: Node) -> &str {
        &self.src[n.byte_range()]
    }

    fn note(&mut self, text: String, risk: Risk) {
        self.out.notes.push(Note { text, risk });
    }

    fn node(&mut self, n: Node, nested: bool) {
        match n.kind() {
            "command" => self.command(n, nested),
            "pipeline" => self.pipeline(n, nested),
            "redirected_statement" => {
                for child in named(n) {
                    if child.kind().ends_with("redirect") {
                        self.redirect(child);
                    } else {
                        self.node(child, nested);
                    }
                }
            }
            "if_statement"
            | "while_statement"
            | "for_statement"
            | "c_style_for_statement"
            | "case_statement"
            | "function_definition" => {
                self.note(
                    "Contains shell control flow: each command is explained, but not when or how often it runs".into(),
                    Risk::Medium,
                );
                self.children(n, nested);
            }
            _ => self.children(n, nested),
        }
    }

    fn children(&mut self, n: Node, nested: bool) {
        for child in named(n) {
            self.node(child, nested);
        }
    }

    fn pipeline(&mut self, n: Node, nested: bool) {
        let mut prev: Option<String> = None;
        for child in named(n) {
            let name = command_name(child, self.src);
            if let (Some(p), Some(name)) = (&prev, &name) {
                // An interpreter reading a pipe runs whatever arrives.
                let base = basename(name);
                let reads_pipe = INTERPRETERS.contains(&base)
                    && command_args(child, self.src)
                        .iter()
                        .all(|a| a.starts_with('-'));
                if reads_pipe {
                    self.note(
                        format!("Pipes the output of `{p}` into `{base}`, which runs it as code without showing it first"),
                        Risk::High,
                    );
                }
            }
            prev = name.map(|n| basename(&n).to_string());
            self.node(child, nested);
        }
    }

    fn redirect(&mut self, n: Node) {
        let text = self.text(n).to_string();
        let dest = n
            .child_by_field_name("destination")
            .map(|d| unquote(self.text(d)))
            .unwrap_or_default();
        let op = n
            .children(&mut n.walk())
            .find(|c| !c.is_named())
            .map(|c| self.text(c).to_string())
            .unwrap_or_default();
        let (msg, risk) = match op.as_str() {
            _ if dest == "/dev/null" => (format!("`{text}` throws that output away"), Risk::Low),
            ">" | ">|" | "&>" => (
                format!("`{text}` replaces the contents of {dest}"),
                file_risk(&dest),
            ),
            ">>" | "&>>" => (format!("`{text}` appends to {dest}"), file_risk(&dest)),
            "<" => (format!("`{text}` reads input from {dest}"), Risk::Low),
            _ => (format!("`{text}` redirects input or output"), Risk::Medium),
        };
        self.note(msg, risk);
    }

    fn command(&mut self, n: Node, nested: bool) {
        let mut words = Vec::new();
        for child in named(n) {
            match child.kind() {
                "variable_assignment" => {
                    let t = self.text(child).to_string();
                    self.note(
                        format!("`{t}` sets an environment variable for this command"),
                        Risk::Low,
                    );
                }
                "command_name" => words.push(unquote(self.text(child))),
                "command_substitution" => {
                    let t = self.text(child).to_string();
                    self.note(
                        format!("`{t}` runs first; its output becomes arguments here"),
                        Risk::Medium,
                    );
                    self.children(child, true);
                    words.push(t);
                }
                "simple_expansion" | "expansion" => {
                    words.push(self.text(child).to_string());
                }
                k if k.ends_with("redirect") => self.redirect(child),
                _ => words.push(unquote(self.text(child))),
            }
        }
        if !words.is_empty() {
            let explained = explain_words(self.lib, self.sys, &words, false, nested);
            for e in explained {
                for arg in e.items.iter().filter(|i| i.kind == ItemKind::Argument) {
                    if arg.risk == Some(Risk::High) {
                        self.out.notes.push(Note {
                            text: format!(
                                "`{} {}` {}",
                                e.program,
                                arg.text,
                                arg.help.to_lowercase()
                            ),
                            risk: Risk::High,
                        });
                    }
                }
                self.out.commands.push(e);
            }
        }
    }
}

fn named(n: Node) -> Vec<Node> {
    n.named_children(&mut n.walk()).collect()
}

fn file_risk(path: &str) -> Risk {
    if path.starts_with("/etc/")
        || path.starts_with("/dev/")
        || path.starts_with("/boot/")
        || path.starts_with("/usr/")
    {
        Risk::High
    } else {
        Risk::Medium
    }
}

fn command_name(n: Node, src: &str) -> Option<String> {
    let cmd = if n.kind() == "redirected_statement" {
        n.child_by_field_name("body")?
    } else {
        n
    };
    (cmd.kind() == "command")
        .then(|| cmd.child_by_field_name("name"))
        .flatten()
        .map(|c| unquote(&src[c.byte_range()]))
}

fn command_args(n: Node, src: &str) -> Vec<String> {
    let mut cursor = n.walk();
    n.children_by_field_name("argument", &mut cursor)
        .map(|a| unquote(&src[a.byte_range()]))
        .collect()
}

/// `'a b'` / `"a b"` → `a b` when the quotes hold plain text.
fn unquote(s: &str) -> String {
    let inner = s
        .strip_prefix('\'')
        .and_then(|r| r.strip_suffix('\''))
        .or_else(|| {
            s.strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .filter(|r| !r.contains(['$', '`', '\\']))
        });
    inner.unwrap_or(s).to_string()
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Explains one simple command given as words (`argv`). Wrappers such as
/// `sudo` produce one entry for themselves and one for what they run.
pub fn explain_words(
    lib: &Library,
    sys: &dyn System,
    words: &[String],
    as_root: bool,
    nested: bool,
) -> Vec<Explained> {
    let Some((first, args)) = words.split_first() else {
        return vec![];
    };
    let program = basename(first).to_string();
    let Some(doc) = lib.commands.get(&program) else {
        return vec![unknown_program(sys, &program, args, as_root, nested)];
    };
    let mut items = Vec::new();
    let mut i = 0;
    let mut positional_seen = false;
    let mut flags_done = false;
    while i < args.len() {
        let a = &args[i];
        if !flags_done && a == "--" {
            flags_done = true;
            i += 1;
            continue;
        }
        if !flags_done && a.starts_with('-') && a.len() > 1 {
            i += flag_items(doc, args, i, &mut items);
            continue;
        }
        if doc.wraps {
            let mut out = vec![Explained {
                program,
                summary: Some(doc.summary.clone()),
                known: true,
                as_root,
                nested,
                items,
                risk: Some(doc.risk),
            }];
            out.extend(explain_words(
                lib,
                sys,
                &args[i..],
                as_root || doc.as_root,
                nested,
            ));
            return out;
        }
        if !positional_seen {
            if let Some(sub) = longest_subcommand(doc, &args[i..]) {
                items.push(Item {
                    text: sub.words.join(" "),
                    kind: ItemKind::Subcommand,
                    help: sub.help.clone(),
                    risk: sub.risk,
                });
                i += sub.words.len();
                positional_seen = true;
                continue;
            }
        }
        positional_seen = true;
        let dangerous = doc.dangerous_args.iter().any(|d| d == a);
        items.push(Item {
            text: a.clone(),
            kind: ItemKind::Argument,
            help: if dangerous {
                "Targets everything there: this is how whole systems get deleted".into()
            } else {
                "argument".into()
            },
            risk: dangerous.then_some(Risk::High),
        });
        i += 1;
    }
    let item_risk = items.iter().filter_map(|i| i.risk).max();
    let subcommand_sets_risk = items
        .iter()
        .any(|i| i.kind == ItemKind::Subcommand && i.risk.is_some());
    let base = if subcommand_sets_risk {
        // A read-only subcommand ("systemctl status") lowers the program's default.
        item_risk.unwrap_or(doc.risk)
    } else {
        doc.risk.max(item_risk.unwrap_or(Risk::Low))
    };
    vec![Explained {
        program,
        summary: Some(doc.summary.clone()),
        known: true,
        as_root,
        nested,
        items,
        risk: Some(base),
    }]
}

/// Explains the flag at `args[i]` (and its value); returns how many words it used.
fn flag_items(doc: &CommandDoc, args: &[String], i: usize, items: &mut Vec<Item>) -> usize {
    let a = &args[i];
    let flag = |text: String, f: &sancho_core::command_doc::FlagDoc| Item {
        text,
        kind: ItemKind::Flag,
        help: f.help.clone(),
        risk: f.risk,
    };
    // --name=value
    if let Some((name, value)) = a.split_once('=') {
        if let Some(f) = doc.flag(name).filter(|f| f.value) {
            items.push(flag(format!("{name}={value}"), f));
            return 1;
        }
    }
    if let Some(f) = doc.flag(a) {
        if f.value {
            if let Some(v) = args.get(i + 1) {
                items.push(flag(format!("{a} {v}"), f));
                return 2;
            }
        }
        items.push(flag(a.clone(), f));
        return 1;
    }
    // A cluster of short flags (-Syu, -rk2): only if every letter is known.
    if !a.starts_with("--") {
        let chars: Vec<char> = a[1..].chars().collect();
        let mut parts = Vec::new();
        let mut used = 1;
        let mut j = 0;
        while j < chars.len() {
            let name = format!("-{}", chars[j]);
            let Some(f) = doc.flag(&name) else {
                parts.clear();
                break;
            };
            if f.value {
                let rest: String = chars[j + 1..].iter().collect();
                let value = if !rest.is_empty() {
                    Some(rest)
                } else {
                    used = 2;
                    args.get(i + 1).cloned()
                };
                parts.push(flag(
                    format!("{name} {}", value.unwrap_or_default())
                        .trim_end()
                        .to_string(),
                    f,
                ));
                j = chars.len();
            } else {
                parts.push(flag(name, f));
                j += 1;
            }
        }
        if !parts.is_empty() {
            items.extend(parts);
            return used;
        }
    }
    items.push(Item {
        text: a.clone(),
        kind: ItemKind::Unknown,
        help: format!(
            "not in sancho's notes on {} — see `man {}`",
            doc.command, doc.command
        ),
        risk: None,
    });
    1
}

fn longest_subcommand<'d>(
    doc: &'d CommandDoc,
    rest: &[String],
) -> Option<&'d sancho_core::command_doc::SubcommandDoc> {
    doc.subcommands
        .iter()
        .filter(|s| s.words.len() <= rest.len() && s.words.iter().zip(rest).all(|(w, a)| w == a))
        .max_by_key(|s| s.words.len())
}

fn unknown_program(
    sys: &dyn System,
    program: &str,
    args: &[String],
    as_root: bool,
    nested: bool,
) -> Explained {
    let items = args
        .iter()
        .map(|a| {
            if a.starts_with('-') && a.len() > 1 {
                Item {
                    text: a.clone(),
                    kind: ItemKind::Unknown,
                    help: format!("sancho has no notes on {program}'s options"),
                    risk: None,
                }
            } else {
                Item {
                    text: a.clone(),
                    kind: ItemKind::Argument,
                    help: "argument".into(),
                    risk: None,
                }
            }
        })
        .collect();
    Explained {
        program: program.to_string(),
        summary: man_summary(sys, program),
        known: false,
        as_root,
        nested,
        items,
        risk: None,
    }
}

/// First `man -f` line's description: `pacman (8) - package manager utility`.
pub fn man_summary(sys: &dyn System, program: &str) -> Option<String> {
    let out = sys.query(Query::ManSummary(program.to_string()))?;
    let line = out.lines().next()?;
    let desc = line.split_once(" - ")?.1.trim();
    (!desc.is_empty()).then(|| desc.chars().filter(|c| !c.is_control()).collect())
}

/// What a request asks for, before recipe matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// "explain X", "what does X do", "what is X".
    Explain(String),
    /// "help me with X": explain, fix or do?
    HelpWith(String),
    /// Anything else: a request to do something.
    Do,
}

/// `is_program` says whether a word names a program (notes or a man page);
/// "what is the weather" is not a command to explain.
pub fn route(request: &str, is_program: impl Fn(&str) -> bool) -> Route {
    let t = request.trim();
    let explain = |what: String| {
        let first = what.split_whitespace().next().unwrap_or("");
        let first = if first == "sudo" {
            what.split_whitespace().nth(1).unwrap_or("")
        } else {
            first
        };
        if is_program(basename(first)) {
            Route::Explain(what)
        } else {
            Route::Do
        }
    };
    let lower = t.to_lowercase();
    let rest = |prefix: &str| {
        t[prefix.len()..]
            .trim()
            .trim_end_matches('?')
            .trim()
            .to_string()
    };
    for p in ["explain ", "what's ", "what is ", "whats "] {
        if lower.starts_with(p) && lower.len() > p.len() {
            return explain(rest(p));
        }
    }
    if lower.starts_with("what does ") {
        let body = rest("what does ");
        let body = body.strip_suffix(" do").unwrap_or(&body).trim().to_string();
        if !body.is_empty() {
            return explain(body);
        }
    }
    for p in ["help me with ", "help with "] {
        if lower.starts_with(p) && lower.len() > p.len() {
            return Route::HelpWith(rest(p));
        }
    }
    Route::Do
}
