//! Answers to questions about the system ("is my wifi card intel or
//! realtek?", "how much memory do I have?"), read directly from /sys, /proc,
//! /etc, hwdata's ID lists and a few read-only queries. Nothing runs that
//! changes anything, and everything read is treated as data.

mod ids;
pub mod related;

use sancho_core::recipe::Answer;
use sancho_core::{Query, System};
use sancho_resolve::Value;

/// A short answer: a title and labeled rows, plus an optional note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub title: String,
    pub rows: Vec<(String, String)>,
    pub note: Option<String>,
}

impl Fact {
    fn new(title: &str) -> Fact {
        Fact {
            title: title.to_string(),
            rows: Vec::new(),
            note: None,
        }
    }

    fn row(mut self, label: impl Into<String>, value: impl Into<String>) -> Fact {
        self.rows.push((label.into(), value.into()));
        self
    }

    fn note(mut self, note: impl Into<String>) -> Fact {
        self.note = Some(note.into());
        self
    }
}

/// Answers `kind`; `value` is the resolved slot for answers that take one.
pub fn answer(sys: &dyn System, kind: Answer, value: Option<&Value>) -> Fact {
    match kind {
        Answer::NetworkDevices => network_devices(sys),
        Answer::Cpu => cpu(sys),
        Answer::Gpu => gpu(sys),
        Answer::Memory => memory(sys),
        Answer::Kernel => one_line(sys, "Kernel", "Release", "/proc/sys/kernel/osrelease"),
        Answer::Os => os(sys),
        Answer::Hostname => one_line(sys, "Hostname", "Name", "/etc/hostname"),
        Answer::Battery => battery(sys),
        Answer::IpAddresses => ip_addresses(sys),
        Answer::MyGroups => my_groups(sys),
        Answer::PackageVersion => package_version(value),
        Answer::ServiceStatus => service_status(sys, value),
    }
}

fn unknown(title: &str, what: &str) -> Fact {
    Fact::new(title).note(format!("sancho couldn't read {what} on this system."))
}

fn one_line(sys: &dyn System, title: &str, label: &str, path: &str) -> Fact {
    match sys
        .read(path)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(v) => Fact::new(title).row(label, v),
        None => unknown(title, path),
    }
}

/// `KEY=value` lines (uevent files, os-release).
fn field(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
}

/// Vendor and model of the device behind `sysfs_device` (a directory with a
/// `uevent` file), and its driver.
fn device_name(sys: &dyn System, sysfs_device: &str) -> (String, Option<String>) {
    let uevent = sys
        .read(&format!("{sysfs_device}/uevent"))
        .unwrap_or_default();
    let driver = field(&uevent, "DRIVER");
    if let Some(id) = field(&uevent, "PCI_ID") {
        if let Some((v, d)) = id.split_once(':') {
            let text = sys.read("/usr/share/hwdata/pci.ids").unwrap_or_default();
            return (ids::describe(&text, v, d), driver);
        }
    }
    // USB: PRODUCT=bda/8156/3121 (hex, not zero-padded).
    if let Some(product) = field(&uevent, "PRODUCT") {
        let mut parts = product.split('/');
        if let (Some(v), Some(d)) = (parts.next(), parts.next()) {
            let text = sys.read("/usr/share/hwdata/usb.ids").unwrap_or_default();
            return (
                ids::describe(&text, &format!("{v:0>4}"), &format!("{d:0>4}")),
                driver,
            );
        }
    }
    ("unknown device".into(), driver)
}

fn with_driver(name: String, driver: Option<String>) -> String {
    match driver {
        Some(d) => format!("{name} (driver {d})"),
        None => name,
    }
}

fn network_devices(sys: &dyn System) -> Fact {
    let mut fact = Fact::new("Network devices");
    for iface in sys.read_dir("/sys/class/net") {
        let base = format!("/sys/class/net/{iface}");
        if !sys.exists(&format!("{base}/device")) {
            continue; // virtual: lo, bridges, containers
        }
        let wireless =
            sys.exists(&format!("{base}/wireless")) || sys.exists(&format!("{base}/phy80211"));
        let kind = if wireless { "Wi-Fi" } else { "wired" };
        let (name, driver) = device_name(sys, &format!("{base}/device"));
        fact = fact.row(format!("{iface} ({kind})"), with_driver(name, driver));
    }
    if fact.rows.is_empty() {
        return fact.note("No physical network devices found.");
    }
    fact
}

fn cpu(sys: &dyn System) -> Fact {
    let Some(info) = sys.read("/proc/cpuinfo") else {
        return unknown("CPU", "/proc/cpuinfo");
    };
    let model = info
        .lines()
        .find_map(|l| {
            l.strip_prefix("model name")
                .and_then(|r| r.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".into());
    let threads = info.lines().filter(|l| l.starts_with("processor")).count();
    Fact::new("CPU")
        .row("Model", model)
        .row("Threads", threads.to_string())
}

fn gpu(sys: &dyn System) -> Fact {
    let mut fact = Fact::new("Graphics");
    for card in sys.read_dir("/sys/class/drm") {
        // card0, card1 (not connectors such as card0-HDMI-A-1)
        if !card.starts_with("card") || card.contains('-') {
            continue;
        }
        let (name, driver) = device_name(sys, &format!("/sys/class/drm/{card}/device"));
        fact = fact.row(card, with_driver(name, driver));
    }
    if fact.rows.is_empty() {
        return fact.note("No graphics devices found.");
    }
    fact
}

fn memory(sys: &dyn System) -> Fact {
    let Some(info) = sys.read("/proc/meminfo") else {
        return unknown("Memory", "/proc/meminfo");
    };
    let kib = |key: &str| {
        info.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
    };
    let gib = |k: u64| format!("{:.1} GiB", k as f64 / 1_048_576.0);
    let mut fact = Fact::new("Memory");
    if let Some(t) = kib("MemTotal") {
        fact = fact.row("Total", gib(t));
    }
    if let Some(a) = kib("MemAvailable") {
        fact = fact.row("Available", gib(a));
    }
    match (kib("SwapTotal"), kib("SwapFree")) {
        (Some(0), _) => fact.row("Swap", "none"),
        (Some(t), Some(f)) => fact.row("Swap", format!("{} of {} free", gib(f), gib(t))),
        _ => fact,
    }
}

fn os(sys: &dyn System) -> Fact {
    let Some(text) = sys
        .read("/etc/os-release")
        .or_else(|| sys.read("/usr/lib/os-release"))
    else {
        return unknown("Operating system", "os-release");
    };
    let mut fact = Fact::new("Operating system");
    if let Some(name) = field(&text, "PRETTY_NAME").or_else(|| field(&text, "NAME")) {
        fact = fact.row("Name", name);
    }
    if let Some(kernel) = sys.read("/proc/sys/kernel/osrelease") {
        fact = fact.row("Kernel", kernel.trim());
    }
    fact
}

fn battery(sys: &dyn System) -> Fact {
    let mut fact = Fact::new("Battery");
    for supply in sys.read_dir("/sys/class/power_supply") {
        let base = format!("/sys/class/power_supply/{supply}");
        if sys
            .read(&format!("{base}/type"))
            .is_none_or(|t| t.trim() != "Battery")
        {
            continue;
        }
        let capacity = sys
            .read(&format!("{base}/capacity"))
            .map(|c| format!("{}%", c.trim()));
        let status = sys
            .read(&format!("{base}/status"))
            .map(|s| s.trim().to_lowercase());
        let value = match (capacity, status) {
            (Some(c), Some(s)) => format!("{c}, {s}"),
            (Some(c), None) => c,
            (None, Some(s)) => s,
            (None, None) => "present".into(),
        };
        fact = fact.row(supply, value);
    }
    if fact.rows.is_empty() {
        return fact.note("No battery found; this is probably a desktop.");
    }
    fact
}

fn ip_addresses(sys: &dyn System) -> Fact {
    let Some(out) = sys.query(Query::IpAddresses) else {
        return unknown("IP addresses", "the address list (ip -o addr show)");
    };
    // "2: wlan0    inet 192.168.1.5/24 brd ... scope global dynamic wlan0"
    let mut rows: Vec<(String, Vec<String>)> = Vec::new();
    for line in out.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let (Some(iface), Some(family), Some(addr)) = (words.get(1), words.get(2), words.get(3))
        else {
            continue;
        };
        let global = words.windows(2).any(|w| w == ["scope", "global"]);
        if *iface == "lo" || !matches!(*family, "inet" | "inet6") || !global {
            continue;
        }
        match rows.iter_mut().find(|(i, _)| i == iface) {
            Some((_, list)) => list.push(addr.to_string()),
            None => rows.push((iface.to_string(), vec![addr.to_string()])),
        }
    }
    let mut fact = Fact::new("IP addresses");
    for (iface, addrs) in rows {
        fact = fact.row(iface, addrs.join(", "));
    }
    if fact.rows.is_empty() {
        return fact.note("No interface has a global address; this machine looks offline.");
    }
    fact
}

fn my_groups(sys: &dyn System) -> Fact {
    let Some(user) = sys.env("USER").or_else(|| sys.env("LOGNAME")) else {
        return unknown("Your groups", "your user name ($USER)");
    };
    let passwd = sys.read("/etc/passwd").unwrap_or_default();
    let primary_gid = passwd
        .lines()
        .map(|l| l.split(':').collect::<Vec<_>>())
        .find(|f| f.first() == Some(&user.as_str()))
        .and_then(|f| f.get(3).map(|g| g.to_string()));
    let groups: Vec<String> = sys
        .read("/etc/group")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            let member = f.get(3).is_some_and(|m| m.split(',').any(|u| u == user));
            let primary = primary_gid.as_deref().is_some_and(|g| f.get(2) == Some(&g));
            (member || primary).then(|| f[0].to_string())
        })
        .collect();
    Fact::new("Your groups")
        .row("User", user)
        .row(
            "Groups",
            if groups.is_empty() {
                "none".into()
            } else {
                groups.join(" ")
            },
        )
        .note("Changes to group membership apply after you log out and back in.")
}

fn package_version(value: Option<&Value>) -> Fact {
    match value {
        Some(v) => Fact::new("Package").row(
            v.key.clone(),
            v.display.trim_start_matches(&v.key).trim().to_string(),
        ),
        None => Fact::new("Package").note("No package was named."),
    }
}

fn service_status(sys: &dyn System, value: Option<&Value>) -> Fact {
    let Some(unit) = value.map(|v| v.key.clone()) else {
        return Fact::new("Service").note("No service was named.");
    };
    let Some(out) = sys.query(Query::UnitStatus(unit.clone())) else {
        return unknown("Service", &format!("the status of {unit}"));
    };
    let get = |k: &str| field(&out, k).filter(|v| !v.is_empty());
    let mut fact = Fact::new("Service").row("Unit", &unit);
    if let Some(d) = get("Description") {
        fact = fact.row("Description", d);
    }
    let running = match (get("ActiveState").as_deref(), get("SubState")) {
        (Some("active"), Some(sub)) => format!("yes ({sub})"),
        (Some("failed"), _) => "no, it failed (see: systemctl status)".into(),
        (Some(state), _) => format!("no ({state})"),
        (None, _) => "unknown".into(),
    };
    fact = fact.row("Running", running);
    if let Some(boot) = get("UnitFileState") {
        let at_boot = match boot.as_str() {
            "enabled" => "starts at boot".to_string(),
            "disabled" => "does not start at boot".to_string(),
            other => other.to_string(),
        };
        fact = fact.row("At boot", at_boot);
    }
    fact
}

/// An answer for the terminal: a bold title, aligned rows, a dimmed note.
pub fn render(fact: &Fact, paint: &dyn sancho_core::style::Paint) -> String {
    use sancho_core::style::Tone;
    let mut out = format!("{}\n", paint.paint(Tone::Title, &fact.title));
    let width = fact
        .rows
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    for (label, value) in &fact.rows {
        out.push_str(&format!(
            "  {}  {value}\n",
            paint.paint(Tone::Muted, &format!("{label:<width$}"))
        ));
    }
    if let Some(note) = &fact.note {
        out.push_str(&format!("  {}\n", paint.paint(Tone::Muted, note)));
    }
    out
}
