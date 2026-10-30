//! Capture/restore handlers: one per effect kind. Undo restores what was
//! captured; it never runs a guessed inverse.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use sancho_core::EffectKind;
use sancho_exec::{Executor, Invocation};

pub const PACKAGE_CACHE: &str = "/var/cache/pacman/pkg";

/// The state of one resource at one moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    File {
        exists: bool,
        hash: Option<String>,
        mode: Option<String>,
        /// Root-owned copy taken before the change (never in the user's home).
        backup: Option<String>,
    },
    Enablement {
        value: String,
    },
    Active {
        active: bool,
    },
    Packages {
        installed: BTreeMap<String, String>,
    },
    Groups {
        groups: BTreeSet<String>,
    },
    Radio {
        on: bool,
    },
}

impl State {
    /// Equal as observed on the system (backups don't count).
    pub fn same_as(&self, other: &State) -> bool {
        match (self, other) {
            (
                State::File {
                    exists: a,
                    hash: ha,
                    mode: ma,
                    ..
                },
                State::File {
                    exists: b,
                    hash: hb,
                    mode: mb,
                    ..
                },
            ) => a == b && ha == hb && ma == mb,
            _ => self == other,
        }
    }

    /// A short description for previews ("enabled", "absent", ...).
    pub fn describe(&self) -> String {
        match self {
            State::File { exists: false, .. } => "absent".into(),
            State::File { hash, .. } => format!(
                "present ({})",
                hash.as_deref()
                    .map_or("unknown content", |h| &h[..h.len().min(12)])
            ),
            State::Enablement { value } => value.clone(),
            State::Active { active } => if *active { "running" } else { "stopped" }.into(),
            State::Packages { installed } => format!("{} packages", installed.len()),
            State::Groups { groups } => groups.iter().cloned().collect::<Vec<_>>().join(" "),
            State::Radio { on } => if *on { "on" } else { "off" }.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// A command failed to run at all.
    Exec(String),
    /// The command ran but its output wasn't understood.
    Unexpected(String),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Exec(e) | StateError::Unexpected(e) => f.write_str(e),
        }
    }
}

/// Whether this kind of effect has a capture/restore handler.
pub fn has_handler(kind: EffectKind) -> bool {
    use EffectKind::*;
    matches!(
        kind,
        FileWrite
            | FileEdit
            | ServiceEnablement
            | ServiceRuntime
            | PackageInstall
            | PackageRemove
            | GroupMembership
            | RadioState
    )
}

fn run(ex: &mut dyn Executor, argv: &[&str], root: bool) -> Result<(i32, String), StateError> {
    let out = ex
        .run(&Invocation::query(argv, root))
        .map_err(|e| StateError::Exec(format!("`{}`: {e}", argv.join(" "))))?;
    Ok((out.status, out.stdout))
}

/// Reads the current state of `target` for an effect kind; `None` when the
/// kind has nothing to restore (runtime, snapshot, irreversible kinds).
/// With `backup = Some(path)`, a file's current content is also copied there.
pub fn read(
    ex: &mut dyn Executor,
    kind: EffectKind,
    target: &str,
    backup: Option<&str>,
) -> Result<Option<State>, StateError> {
    use EffectKind::*;
    let state = match kind {
        FileWrite | FileEdit => {
            let (status, mode) = run(ex, &["stat", "-c", "%a", "--", target], true)?;
            if status != 0 {
                State::File {
                    exists: false,
                    hash: None,
                    mode: None,
                    backup: None,
                }
            } else {
                let (status, sum) = run(ex, &["sha256sum", "--", target], true)?;
                let hash = sum.split_whitespace().next().filter(|_| status == 0);
                let hash =
                    hash.ok_or_else(|| StateError::Unexpected(format!("could not read {target}")))?;
                let saved = match backup {
                    Some(dest) => {
                        let dir = dest.rsplit_once('/').map_or("/", |(d, _)| d);
                        run(ex, &["install", "-d", "-m", "0700", dir], true)?;
                        let (status, _) = run(ex, &["cp", "-a", "--", target, dest], true)?;
                        if status != 0 {
                            return Err(StateError::Unexpected(format!(
                                "could not back up {target}"
                            )));
                        }
                        Some(dest.to_string())
                    }
                    None => None,
                };
                State::File {
                    exists: true,
                    hash: Some(hash.to_string()),
                    mode: Some(mode.trim().to_string()),
                    backup: saved,
                }
            }
        }
        ServiceEnablement => {
            let (_, out) = run(ex, &["systemctl", "is-enabled", target], false)?;
            let value = out.lines().next().unwrap_or("").trim();
            if value.is_empty() {
                return Err(StateError::Unexpected(format!("unknown unit {target}")));
            }
            State::Enablement {
                value: value.into(),
            }
        }
        ServiceRuntime => {
            let (_, out) = run(ex, &["systemctl", "is-active", target], false)?;
            State::Active {
                active: out.trim() == "active",
            }
        }
        PackageInstall | PackageRemove => {
            let (status, out) = run(ex, &["pacman", "-Q"], false)?;
            if status != 0 {
                return Err(StateError::Unexpected(
                    "could not list installed packages".into(),
                ));
            }
            State::Packages {
                installed: out
                    .lines()
                    .filter_map(|l| l.split_once(' '))
                    .map(|(n, v)| (n.to_string(), v.trim().to_string()))
                    .collect(),
            }
        }
        GroupMembership => {
            let (status, out) = run(ex, &["id", "-nG", "--", target], false)?;
            if status != 0 {
                return Err(StateError::Unexpected(format!("unknown user {target}")));
            }
            State::Groups {
                groups: out.split_whitespace().map(String::from).collect(),
            }
        }
        RadioState => {
            let (_, out) = run(ex, &["nmcli", "radio", "wifi"], false)?;
            State::Radio {
                on: out.trim() == "enabled",
            }
        }
        Runtime | Snapshot | PackageUpgrade | PackageCachePrune | Destructive => return Ok(None),
    };
    Ok(Some(state))
}

/// A command that puts part of the state back, with a plain explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restore {
    pub explanation: String,
    pub invocation: Invocation,
}

fn inv(argv: &[&str], root: bool, interactive: bool) -> Invocation {
    Invocation {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        stdin: None,
        root,
        interactive,
        quiet: false,
    }
}

/// Commands that bring `target` from `now` back to `before`. Empty when they
/// already match. `Err` explains why it can't be restored.
pub fn restore(
    ex: &mut dyn Executor,
    target: &str,
    before: &State,
    now: &State,
) -> Result<Vec<Restore>, String> {
    if before.same_as(now) {
        return Ok(vec![]);
    }
    let r = |explanation: String, invocation: Invocation| Restore {
        explanation,
        invocation,
    };
    Ok(match (before, now) {
        (
            State::File {
                exists: true,
                backup,
                ..
            },
            _,
        ) => {
            let Some(backup) = backup else {
                return Err(format!("no backup of {target} was taken"));
            };
            vec![r(
                format!("Put back the previous {target}"),
                inv(&["cp", "-a", "--", backup, target], true, false),
            )]
        }
        (State::File { exists: false, .. }, _) => vec![r(
            format!("Delete {target}, which did not exist before"),
            inv(&["rm", "-f", "--", target], true, false),
        )],
        (State::Enablement { value }, _) => {
            let verb = match value.as_str() {
                "enabled" => "enable",
                "disabled" => "disable",
                "masked" => "mask",
                // static, indirect, generated: not something systemctl sets.
                other => return Err(format!("{target} was {other}, which can't be set back")),
            };
            let mut out = Vec::new();
            if matches!(now, State::Enablement { value } if value == "masked") && verb != "mask" {
                out.push(r(
                    format!("Unmask {target}"),
                    inv(&["systemctl", "unmask", target], true, false),
                ));
            }
            out.push(r(
                format!("Set {target} back to {value} at boot"),
                inv(&["systemctl", verb, target], true, false),
            ));
            out
        }
        (State::Active { active }, _) => {
            let (verb, what) = if *active {
                ("start", "running")
            } else {
                ("stop", "stopped")
            };
            vec![r(
                format!("Set {target} back to {what}"),
                inv(&["systemctl", verb, target], true, false),
            )]
        }
        (State::Packages { installed: before }, State::Packages { installed: after }) => {
            let mut out = Vec::new();
            let added: Vec<&str> = after
                .keys()
                .filter(|n| !before.contains_key(*n))
                .map(String::as_str)
                .collect();
            if !added.is_empty() {
                let mut argv = vec!["pacman", "-R"];
                argv.extend(&added);
                out.push(r(
                    format!(
                        "Remove {}, installed by this change (including dependencies)",
                        added.join(", ")
                    ),
                    inv(&argv, true, true),
                ));
            }
            let missing: Vec<(&String, &String)> = before
                .iter()
                .filter(|(n, v)| after.get(*n) != Some(v))
                .collect();
            if !missing.is_empty() {
                let (_, listing) = run(ex, &["ls", "-1", "--", PACKAGE_CACHE], false)
                    .map_err(|e| e.to_string())?;
                let cached: Vec<&str> = listing.lines().collect();
                let mut files = Vec::new();
                for (name, ver) in &missing {
                    let prefix = format!("{name}-{ver}-");
                    match cached.iter().find(|f| {
                        f.starts_with(&prefix) && f.contains(".pkg.tar") && !f.ends_with(".sig")
                    }) {
                        Some(f) => files.push(format!("{PACKAGE_CACHE}/{f}")),
                        None => {
                            return Err(format!("{name} {ver} is no longer in the package cache"))
                        }
                    }
                }
                let mut argv = vec!["pacman", "-U"];
                argv.extend(files.iter().map(String::as_str));
                let names: Vec<String> = missing.iter().map(|(n, v)| format!("{n} {v}")).collect();
                out.push(r(
                    format!("Reinstall {} from the package cache", names.join(", ")),
                    inv(&argv, true, true),
                ));
            }
            out
        }
        (State::Groups { groups: before }, State::Groups { groups: after }) => {
            let mut out = Vec::new();
            for g in after.difference(before) {
                out.push(r(
                    format!("Remove {target} from the {g} group"),
                    inv(&["gpasswd", "-d", target, g], true, false),
                ));
            }
            for g in before.difference(after) {
                out.push(r(
                    format!("Add {target} back to the {g} group"),
                    inv(&["usermod", "-a", "-G", g, target], true, false),
                ));
            }
            out
        }
        (State::Radio { on }, _) => {
            let s = if *on { "on" } else { "off" };
            vec![r(
                format!("Turn Wi-Fi back {s}"),
                inv(&["nmcli", "radio", "wifi", s], false, false),
            )]
        }
        _ => return Err(format!("state of {target} changed kind; can't restore")),
    })
}

/// Resources a change touched, for linking dependent transactions:
/// e.g. `unit:sshd.service`, `file:/etc/x`, `package:vlc`, `group:alice:video`.
pub fn resources(
    kind: EffectKind,
    target: &str,
    before: Option<&State>,
    after: Option<&State>,
) -> Vec<String> {
    use EffectKind::*;
    match (kind, before, after) {
        (FileWrite | FileEdit, ..) => vec![format!("file:{target}")],
        (ServiceEnablement | ServiceRuntime, ..) => vec![format!("unit:{target}")],
        (RadioState, ..) => vec!["radio:wifi".into()],
        (
            PackageInstall | PackageRemove,
            Some(State::Packages { installed: a }),
            Some(State::Packages { installed: b }),
        ) => {
            let names: BTreeSet<&String> = a
                .keys()
                .chain(b.keys())
                .filter(|n| a.get(*n) != b.get(*n))
                .collect();
            names.into_iter().map(|n| format!("package:{n}")).collect()
        }
        (GroupMembership, Some(State::Groups { groups: a }), Some(State::Groups { groups: b })) => {
            a.symmetric_difference(b)
                .map(|g| format!("group:{target}:{g}"))
                .collect()
        }
        _ => vec![],
    }
}
