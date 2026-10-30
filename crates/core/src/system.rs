//! Read-only access to the system being inspected: files, environment, and a
//! closed set of query commands. The live implementation reads the real
//! machine; the fixture implementation reads a captured tree (`eval/fixtures`).
//!
//! Queries are an enum, so nothing but these read-only commands can be run
//! through this interface. Their output is data, never instructions.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
    /// All devices with their udev properties.
    UdevDb,
    /// Installed unit files and their enablement state.
    UnitFiles,
    /// Installed packages: `name version` per line.
    InstalledPackages,
    /// Packages in the sync databases: `repo name version [installed]`.
    SyncPackages,
}

impl Query {
    pub const ALL: [Query; 4] = [
        Query::UdevDb,
        Query::UnitFiles,
        Query::InstalledPackages,
        Query::SyncPackages,
    ];

    pub fn argv(self) -> &'static [&'static str] {
        match self {
            Query::UdevDb => &["udevadm", "info", "--export-db"],
            Query::UnitFiles => &["systemctl", "list-unit-files", "--no-legend", "--no-pager"],
            Query::InstalledPackages => &["pacman", "-Q"],
            Query::SyncPackages => &["pacman", "-Sl"],
        }
    }

    /// File name of the captured output in a fixture's `queries/` directory.
    pub fn fixture_name(self) -> &'static str {
        match self {
            Query::UdevDb => "udev-db.txt",
            Query::UnitFiles => "unit-files.txt",
            Query::InstalledPackages => "installed-packages.txt",
            Query::SyncPackages => "sync-packages.txt",
        }
    }
}

pub trait System {
    /// Contents of an absolute path, if readable.
    fn read(&self, path: &str) -> Option<String>;
    /// Sorted entry names of a directory; empty if unreadable.
    fn read_dir(&self, path: &str) -> Vec<String>;
    fn exists(&self, path: &str) -> bool;
    fn is_executable(&self, path: &str) -> bool;
    fn env(&self, key: &str) -> Option<String>;
    /// Output of a query, or `None` if it failed or is unavailable.
    fn query(&self, q: Query) -> Option<String>;

    /// Whether an executable `name` is on `PATH`.
    fn has_program(&self, name: &str) -> bool {
        let path = self
            .env("PATH")
            .unwrap_or_else(|| "/usr/local/bin:/usr/bin".into());
        path.split(':')
            .filter(|d| d.starts_with('/'))
            .any(|dir| self.is_executable(&format!("{}/{name}", dir.trim_end_matches('/'))))
    }
}

/// Files under a root directory; shared by the live and fixture systems.
fn read_under(root: &Path, path: &str) -> PathBuf {
    root.join(path.trim_start_matches('/'))
}

fn sorted_dir(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|e| {
            e.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The machine sancho runs on.
#[derive(Debug, Default)]
pub struct LiveSystem;

/// Queries that take longer than this are treated as failed.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

impl System for LiveSystem {
    fn read(&self, path: &str) -> Option<String> {
        fs::read_to_string(path).ok()
    }

    fn read_dir(&self, path: &str) -> Vec<String> {
        sorted_dir(Path::new(path))
    }

    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn is_executable(&self, path: &str) -> bool {
        executable(Path::new(path))
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }

    fn query(&self, q: Query) -> Option<String> {
        let argv = q.argv();
        let mut child = Command::new(argv[0])
            .args(&argv[1..])
            // Stable, untranslated output for parsing.
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut out = Vec::new();
            stdout.read_to_end(&mut out).map(|_| out)
        });
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().ok()? {
                break status;
            }
            if start.elapsed() > QUERY_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let out = reader.join().ok()?.ok()?;
        status
            .success()
            .then(|| String::from_utf8_lossy(&out).into_owned())
    }
}

/// A captured system: `<dir>/root/` (files), `<dir>/env` (`KEY=VALUE` lines)
/// and `<dir>/queries/<Query::fixture_name>`.
#[derive(Debug, Clone)]
pub struct FixtureSystem {
    pub dir: PathBuf,
    env: BTreeMap<String, String>,
}

impl FixtureSystem {
    pub fn new(dir: impl Into<PathBuf>) -> FixtureSystem {
        let dir = dir.into();
        let env = fs::read_to_string(dir.join("env"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        FixtureSystem { dir, env }
    }

    pub fn with_env(mut self, key: &str, value: &str) -> FixtureSystem {
        self.env.insert(key.to_string(), value.to_string());
        self
    }

    fn root(&self) -> PathBuf {
        self.dir.join("root")
    }
}

impl System for FixtureSystem {
    fn read(&self, path: &str) -> Option<String> {
        fs::read_to_string(read_under(&self.root(), path)).ok()
    }

    fn read_dir(&self, path: &str) -> Vec<String> {
        sorted_dir(&read_under(&self.root(), path))
    }

    fn exists(&self, path: &str) -> bool {
        read_under(&self.root(), path).exists()
    }

    fn is_executable(&self, path: &str) -> bool {
        executable(&read_under(&self.root(), path))
    }

    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).filter(|v| !v.is_empty()).cloned()
    }

    fn query(&self, q: Query) -> Option<String> {
        fs::read_to_string(self.dir.join("queries").join(q.fixture_name())).ok()
    }
}
