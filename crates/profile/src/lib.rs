//! System profile detection (distro, init, network stack, audio, ...).
//!
//! Detection only reads files under a root directory (`/` on a real system,
//! a fixture tree in tests) and the environment; it never runs commands.
//! It is cheap, so the profile is detected fresh on every run rather than
//! cached, which would go stale (e.g. display differs between SSH and desktop).

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use sancho_core::recipe::ProfileKey;

/// Where to look: a filesystem root and an environment.
#[derive(Debug, Clone)]
pub struct Host {
    pub root: PathBuf,
    pub env: BTreeMap<String, String>,
}

impl Host {
    pub fn current() -> Host {
        Host {
            root: PathBuf::from("/"),
            env: std::env::vars().collect(),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel.trim_start_matches('/'))
    }

    fn exists(&self, rel: &str) -> bool {
        self.path(rel).exists()
    }

    fn read(&self, rel: &str) -> Option<String> {
        fs::read_to_string(self.path(rel)).ok()
    }

    fn env(&self, key: &str) -> Option<&str> {
        self.env
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// Whether an executable `name` is on `PATH`.
    fn has_program(&self, name: &str) -> bool {
        let path = self.env("PATH").unwrap_or("/usr/local/bin:/usr/bin");
        path.split(':').filter(|d| d.starts_with('/')).any(|dir| {
            fs::metadata(self.path(dir).join(name))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    }

    /// Names (`/proc/<pid>/comm`) of running processes visible to us.
    fn processes(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.path("/proc")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .bytes()
                    .all(|b| b.is_ascii_digit())
            })
            .filter_map(|e| fs::read_to_string(e.path().join("comm")).ok())
            .map(|s| s.trim_end().to_string())
            .collect()
    }
}

/// Detected facts. Every key is present; every value is one of `key.values()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    facts: BTreeMap<ProfileKey, &'static str>,
}

/// A recipe requirement this system does not meet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmet {
    pub key: ProfileKey,
    pub actual: &'static str,
    pub accepted: Vec<String>,
}

impl std::fmt::Display for Unmet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "needs {} {}, this system has {}",
            self.key.name(),
            self.accepted.join(" or "),
            self.actual
        )
    }
}

impl Profile {
    pub fn get(&self, key: ProfileKey) -> &'static str {
        self.facts[&key]
    }

    pub fn iter(&self) -> impl Iterator<Item = (ProfileKey, &'static str)> + '_ {
        self.facts.iter().map(|(k, v)| (*k, *v))
    }

    /// Requirements (a recipe's `requires`) that this system does not meet.
    /// A recipe with any unmet requirement is excluded from matching.
    pub fn unmet(&self, requires: &BTreeMap<ProfileKey, Vec<String>>) -> Vec<Unmet> {
        requires
            .iter()
            .filter(|(k, accepted)| !accepted.iter().any(|v| v == self.get(**k)))
            .map(|(k, accepted)| Unmet {
                key: *k,
                actual: self.get(*k),
                accepted: accepted.clone(),
            })
            .collect()
    }
}

pub fn detect(host: &Host) -> Profile {
    let procs = host.processes();
    let running = |name: &str| procs.iter().any(|p| p == name);
    let facts = ProfileKey::ALL
        .into_iter()
        .map(|key| {
            let value = match key {
                ProfileKey::Distro => distro(host),
                ProfileKey::Init => init(host),
                ProfileKey::Network => network(&running),
                ProfileKey::Audio => audio(&running),
                ProfileKey::Display => display(host),
                ProfileKey::AurHelper => aur_helper(host),
                ProfileKey::RootFs => root_fs(host),
                ProfileKey::SnapshotTool => snapshot_tool(host),
                ProfileKey::Bootloader => bootloader(host),
            };
            debug_assert!(key.values().contains(&value), "{value} not in {key:?}");
            (key, value)
        })
        .collect();
    Profile { facts }
}

/// From os-release(5): `/etc/os-release`, falling back to `/usr/lib/os-release`.
fn distro(host: &Host) -> &'static str {
    let Some(text) = host
        .read("/etc/os-release")
        .or_else(|| host.read("/usr/lib/os-release"))
    else {
        return "unsupported";
    };
    let field = |name: &str| {
        text.lines()
            .filter_map(|l| l.split_once('='))
            .find(|(k, _)| k.trim() == name)
            .map(|(_, v)| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
            .unwrap_or_default()
    };
    let id = field("ID");
    let known = ProfileKey::Distro.values();
    if let Some(v) = known.iter().find(|v| **v == id && !v.contains('_')) {
        return v;
    }
    if field("ID_LIKE").split_whitespace().any(|l| l == "arch") {
        "arch_derivative"
    } else {
        "unsupported"
    }
}

/// Runtime markers each init system creates, then PID 1's name.
fn init(host: &Host) -> &'static str {
    // sd_booted(3): /run/systemd/system exists only when systemd is PID 1.
    let markers = [
        ("/run/systemd/system", "systemd"),
        ("/run/openrc", "openrc"),
        ("/run/runit", "runit"),
        ("/run/dinitctl", "dinit"),
        ("/run/s6", "s6"),
    ];
    if let Some((_, v)) = markers.iter().find(|(m, _)| host.exists(m)) {
        return v;
    }
    match host.read("/proc/1/comm").as_deref().map(str::trim_end) {
        Some("systemd") => "systemd",
        Some("openrc-init") => "openrc",
        Some("runit") => "runit",
        Some("dinit") => "dinit",
        Some("s6-svscan") => "s6",
        _ => "unknown",
    }
}

/// The daemon that manages connections. NetworkManager and ConnMan may use
/// iwd as their Wi-Fi backend, so they win over it; iwd beats networkd, which
/// it is commonly paired with for IP configuration.
fn network(running: &dyn Fn(&str) -> bool) -> &'static str {
    [
        ("NetworkManager", "networkmanager"),
        ("connmand", "connman"),
        ("iwd", "iwd"),
        // comm is truncated to 15 bytes.
        ("systemd-network", "systemd_networkd"),
    ]
    .into_iter()
    .find(|(p, _)| running(p))
    .map_or("none", |(_, v)| v)
}

fn audio(running: &dyn Fn(&str) -> bool) -> &'static str {
    if running("pipewire") {
        "pipewire"
    } else if running("pulseaudio") {
        "pulseaudio"
    } else {
        "none"
    }
}

fn display(host: &Host) -> &'static str {
    match host.env("XDG_SESSION_TYPE") {
        Some("wayland") => "wayland",
        Some("x11") => "x11",
        _ if host.env("WAYLAND_DISPLAY").is_some() => "wayland",
        _ if host.env("DISPLAY").is_some() => "x11",
        _ => "none",
    }
}

fn aur_helper(host: &Host) -> &'static str {
    ["paru", "yay"]
        .into_iter()
        .find(|h| host.has_program(h))
        .unwrap_or("none")
}

/// Filesystem type of the last mount on `/` in `/proc/self/mounts`.
fn root_fs(host: &Host) -> &'static str {
    let fstype = host
        .read("/proc/self/mounts")
        .and_then(|m| {
            m.lines().rev().find_map(|l| {
                let mut f = l.split_whitespace();
                let (_, mnt, ty) = (f.next()?, f.next()?, f.next()?);
                (mnt == "/").then(|| ty.to_string())
            })
        })
        .unwrap_or_default();
    let known = ProfileKey::RootFs.values();
    known
        .iter()
        .find(|v| **v == fstype)
        .copied()
        .unwrap_or("other")
}

/// A snapshot tool counts only when installed and configured for `/`.
fn snapshot_tool(host: &Host) -> &'static str {
    if host.has_program("snapper") && host.exists("/etc/snapper/configs/root") {
        "snapper"
    } else if host.has_program("timeshift") && host.exists("/etc/timeshift/timeshift.json") {
        "timeshift"
    } else {
        "none"
    }
}

/// Files on the ESP or /boot first (often unreadable without root), then the
/// configuration packages leave in /etc. GRUB is last because /etc/default/grub
/// can outlive a switch to another bootloader.
fn bootloader(host: &Host) -> &'static str {
    let evidence: [(&str, &[&str]); 4] = [
        (
            "systemd_boot",
            &[
                "/boot/loader/loader.conf",
                "/efi/loader/loader.conf",
                "/boot/efi/loader/loader.conf",
            ],
        ),
        (
            "limine",
            &[
                "/boot/limine.conf",
                "/boot/limine/limine.conf",
                "/boot/EFI/limine/limine.conf",
                "/efi/limine.conf",
                "/etc/default/limine",
            ],
        ),
        (
            "refind",
            &[
                "/boot/EFI/refind/refind.conf",
                "/efi/EFI/refind/refind.conf",
                "/boot/refind_linux.conf",
            ],
        ),
        ("grub", &["/boot/grub/grub.cfg", "/etc/default/grub"]),
    ];
    evidence
        .iter()
        .find(|(_, files)| files.iter().any(|f| host.exists(f)))
        .map_or("unknown", |(v, _)| v)
}
