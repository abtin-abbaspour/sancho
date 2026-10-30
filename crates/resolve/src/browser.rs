//! `browser`: installed web browsers, from desktop entries in
//! `/usr/share/applications` with the `WebBrowser` category.

use sancho_core::System;

use crate::{safe_token, Value};

/// Desktop-file prefixes of Chromium-based browsers, which support WebHID.
/// Anything else (Firefox and forks, unknown browsers) is assumed not to.
const CHROMIUM_FAMILY: &[&str] = &[
    "chromium",
    "google-chrome",
    "brave",
    "vivaldi",
    "microsoft-edge",
    "opera",
    "thorium",
    "com.google.chrome",
    "com.brave.browser",
    "org.chromium.chromium",
    "com.vivaldi.vivaldi",
    "com.microsoft.edge",
];

const APPS: &str = "/usr/share/applications";

pub fn enumerate(sys: &dyn System) -> Vec<Value> {
    sys.read_dir(APPS)
        .into_iter()
        .filter(|f| f.ends_with(".desktop") && safe_token(f, "._-", 255))
        .filter_map(|file| {
            let entry = DesktopEntry::parse(&sys.read(&format!("{APPS}/{file}"))?);
            if entry.hidden || !entry.categories.iter().any(|c| c == "WebBrowser") {
                return None;
            }
            let name = entry
                .name
                .unwrap_or_else(|| file.trim_end_matches(".desktop").into());
            let lower = file.to_lowercase();
            let webhid = CHROMIUM_FAMILY.iter().any(|p| lower.starts_with(p));
            let mut v = Value::new(&file, &name).tag("webhid", webhid);
            v.attrs.insert("name", name);
            v.attrs.insert("desktop_id", file);
            Some(v)
        })
        .collect()
}

#[derive(Default)]
struct DesktopEntry {
    name: Option<String>,
    categories: Vec<String>,
    hidden: bool,
}

impl DesktopEntry {
    /// Reads keys of the `[Desktop Entry]` group only (actions have their own).
    fn parse(text: &str) -> DesktopEntry {
        let mut e = DesktopEntry::default();
        let mut in_main = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_main = line == "[Desktop Entry]";
                continue;
            }
            let Some((k, v)) = line.split_once('=').filter(|_| in_main) else {
                continue;
            };
            match k.trim() {
                "Name" => e.name = Some(v.trim().chars().filter(|c| !c.is_control()).collect()),
                "Categories" => e.categories = v.split(';').map(|s| s.trim().to_string()).collect(),
                "Hidden" | "NoDisplay" if v.trim() == "true" => e.hidden = true,
                _ => {}
            }
        }
        e
    }
}
