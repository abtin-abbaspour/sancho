//! A simulated machine for tests: records every invocation and applies the
//! commands sancho's primitives and state handlers use to an in-memory state.
//! Unknown commands exit 127 so tests notice anything unmodelled.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use crate::{Executor, Invocation, Output};

pub const PACKAGE_CACHE: &str = "/var/cache/pacman/pkg";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MockState {
    /// path -> (content, octal mode)
    pub files: BTreeMap<String, (String, String)>,
    /// unit -> (is-enabled state, active)
    pub units: BTreeMap<String, (String, bool)>,
    /// installed package -> version
    pub installed: BTreeMap<String, String>,
    /// repository package -> (version, dependencies)
    pub repo: BTreeMap<String, (String, Vec<String>)>,
    /// files in the package cache, e.g. "vlc-3.0.21-8-x86_64.pkg.tar.zst"
    pub cache: BTreeSet<String>,
    /// user -> supplementary groups
    pub groups: BTreeMap<String, BTreeSet<String>>,
    pub wifi: bool,
}

#[derive(Debug, Default)]
pub struct MockExecutor {
    pub state: MockState,
    pub log: Vec<Invocation>,
    /// Commands whose joined argv contains any of these fail with status 1.
    pub fail_on: Vec<String>,
}

impl MockExecutor {
    pub fn new(state: MockState) -> MockExecutor {
        MockExecutor {
            state,
            ..Default::default()
        }
    }

    /// Joined argv of every command run, in order.
    pub fn commands(&self) -> Vec<String> {
        self.log.iter().map(|i| i.argv.join(" ")).collect()
    }
}

/// A stand-in for sha256: stable and content-dependent, which is all the
/// handlers rely on.
pub fn mock_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn ok(stdout: impl Into<String>) -> Output {
    Output {
        status: 0,
        stdout: stdout.into(),
    }
}

fn fail(status: i32) -> Output {
    Output {
        status,
        stdout: String::new(),
    }
}

impl Executor for MockExecutor {
    fn run(&mut self, inv: &Invocation) -> io::Result<Output> {
        self.log.push(inv.clone());
        let joined = inv.argv.join(" ");
        if self.fail_on.iter().any(|f| joined.contains(f.as_str())) {
            return Ok(fail(1));
        }
        let a: Vec<&str> = inv.argv.iter().map(String::as_str).collect();
        let s = &mut self.state;
        Ok(match a.as_slice() {
            ["systemctl", "is-enabled", unit] => match s.units.get(*unit) {
                Some((e, _)) => Output {
                    status: i32::from(e != "enabled"),
                    stdout: format!("{e}\n"),
                },
                None => fail(1),
            },
            ["systemctl", "is-active", unit] => match s.units.get(*unit) {
                Some((_, active)) => Output {
                    status: i32::from(!active),
                    stdout: if *active { "active\n" } else { "inactive\n" }.into(),
                },
                None => Output {
                    status: 3,
                    stdout: "inactive\n".into(),
                },
            },
            ["systemctl", verb, unit] => match s.units.get_mut(*unit) {
                None => fail(5),
                Some(u) => {
                    match *verb {
                        "enable" if u.0 != "masked" => u.0 = "enabled".into(),
                        "disable" if u.0 != "masked" => u.0 = "disabled".into(),
                        "mask" => u.0 = "masked".into(),
                        "unmask" => u.0 = "disabled".into(),
                        "start" | "restart" => u.1 = true,
                        "stop" => u.1 = false,
                        _ => return Ok(fail(1)),
                    }
                    ok("")
                }
            },
            ["install", "-m", mode, "/dev/stdin", path] => {
                let content = inv.stdin.clone().unwrap_or_default();
                s.files
                    .insert(path.to_string(), (content, mode.to_string()));
                ok("")
            }
            ["sha256sum", "--", path] => match s.files.get(*path) {
                Some((c, _)) => ok(format!("{}  {path}\n", mock_hash(c))),
                None => fail(1),
            },
            ["stat", "-c", "%a", "--", path] => match s.files.get(*path) {
                Some((_, m)) => ok(format!("{}\n", m.trim_start_matches('0'))),
                None => fail(1),
            },
            ["install", "-d", "-m", "0700", _dir] => ok(""),
            ["cp", "-a", "--", from, to] => match s.files.get(*from).cloned() {
                Some(f) => {
                    s.files.insert(to.to_string(), f);
                    ok("")
                }
                None => fail(1),
            },
            ["rm", "-f", "--", path] => {
                s.files.remove(*path);
                ok("")
            }
            ["pacman", "-Q"] => ok(s
                .installed
                .iter()
                .map(|(n, v)| format!("{n} {v}\n"))
                .collect::<String>()),
            ["pacman", "-S", "--needed", pkgs @ ..] => {
                let mut todo: Vec<String> = pkgs.iter().map(|p| p.to_string()).collect();
                if todo.iter().any(|p| !s.repo.contains_key(p)) {
                    return Ok(fail(1));
                }
                while let Some(p) = todo.pop() {
                    if s.installed.contains_key(&p) {
                        continue;
                    }
                    let (ver, deps) = s.repo[&p].clone();
                    s.cache.insert(format!("{p}-{ver}-x86_64.pkg.tar.zst"));
                    s.installed.insert(p, ver);
                    todo.extend(deps);
                }
                ok("")
            }
            ["pacman", "-R", pkgs @ ..] => {
                if pkgs.iter().any(|p| !s.installed.contains_key(*p)) {
                    return Ok(fail(1));
                }
                for p in pkgs {
                    s.installed.remove(*p);
                }
                ok("")
            }
            ["pacman", "-U", files @ ..] => {
                for f in files.iter() {
                    let name = f.trim_start_matches(PACKAGE_CACHE).trim_start_matches('/');
                    if !s.cache.contains(name) {
                        return Ok(fail(1));
                    }
                    // "name-ver-rel-arch.pkg.tar.zst"
                    let stem = name.split(".pkg.").next().unwrap_or(name);
                    let parts: Vec<&str> = stem.rsplitn(4, '-').collect();
                    if let [_, rel, ver, pkg] = parts.as_slice() {
                        s.installed.insert(pkg.to_string(), format!("{ver}-{rel}"));
                    }
                }
                ok("")
            }
            ["ls", "-1", "--", dir] if *dir == PACKAGE_CACHE => {
                ok(s.cache.iter().map(|f| format!("{f}\n")).collect::<String>())
            }
            ["id", "-nG", "--", user] => match s.groups.get(*user) {
                Some(g) => ok(format!(
                    "{}\n",
                    std::iter::once(user.to_string())
                        .chain(g.iter().cloned())
                        .collect::<Vec<_>>()
                        .join(" ")
                )),
                None => fail(1),
            },
            ["usermod", "-a", "-G", group, user] => {
                s.groups
                    .entry(user.to_string())
                    .or_default()
                    .insert(group.to_string());
                ok("")
            }
            ["gpasswd", "-d", user, group] => {
                let removed = s.groups.get_mut(*user).is_some_and(|g| g.remove(*group));
                if removed {
                    ok("")
                } else {
                    fail(3)
                }
            }
            ["nmcli", "radio", "wifi"] => ok(if s.wifi { "enabled\n" } else { "disabled\n" }),
            ["nmcli", "radio", "wifi", state] => {
                s.wifi = *state == "on";
                ok("")
            }
            ["udevadm", ..]
            | ["snapper", ..]
            | ["timeshift", ..]
            | ["df", ..]
            | ["paccache", ..] => ok(""),
            _ => fail(127),
        })
    }
}
