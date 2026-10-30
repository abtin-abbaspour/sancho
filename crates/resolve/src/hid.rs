//! `hid_device`: HID devices that expose a hidraw node, from the udev database.
//!
//! One physical device often has several HID interfaces (e.g. keyboard + media
//! keys); they are grouped by vendor:product, which is also what udev rules
//! match on. Tags come from the `ID_INPUT_*` properties of each interface's
//! input devices, so a receiver with keyboard and mouse interfaces is both.

use std::collections::BTreeMap;

use sancho_core::{Query, System};

use crate::{ResolveError, Value};

struct Record {
    path: String,
    props: BTreeMap<String, String>,
}

/// Parses `udevadm info --export-db`: blank-line separated records with
/// `P: <devpath>` and `E: KEY=VALUE` lines.
fn parse(db: &str) -> Vec<Record> {
    let mut records = Vec::new();
    let mut current: Option<Record> = None;
    for line in db.lines() {
        if let Some(path) = line.strip_prefix("P: ") {
            records.extend(current.take());
            current = Some(Record {
                path: path.to_string(),
                props: BTreeMap::new(),
            });
        } else if let (Some(rec), Some(kv)) = (current.as_mut(), line.strip_prefix("E: ")) {
            if let Some((k, v)) = kv.split_once('=') {
                rec.props.insert(k.to_string(), v.to_string());
            }
        } else if line.is_empty() {
            records.extend(current.take());
        }
    }
    records.extend(current);
    records
}

const CLASSES: &[(&str, &str)] = &[
    ("ID_INPUT_KEYBOARD", "keyboard"),
    ("ID_INPUT_MOUSE", "mouse"),
    ("ID_INPUT_TOUCHPAD", "touchpad"),
    ("ID_INPUT_JOYSTICK", "joystick"),
    ("ID_INPUT_TABLET", "tablet"),
];

pub fn enumerate(sys: &dyn System) -> Result<Vec<Value>, ResolveError> {
    let db = sys
        .query(Query::UdevDb)
        .ok_or(ResolveError::Unavailable(Query::UdevDb))?;
    let records = parse(&db);
    let subsystem = |r: &Record, s: &str| r.props.get("SUBSYSTEM").is_some_and(|v| v == s);
    let children = |parent: &str, s: &'static str| {
        let prefix = format!("{parent}/");
        records
            .iter()
            .filter(move |r| r.path.starts_with(&prefix) && subsystem(r, s))
    };

    let mut devices: BTreeMap<String, Value> = BTreeMap::new();
    for hid in records.iter().filter(|r| subsystem(r, "hid")) {
        if children(&hid.path, "hidraw").next().is_none() {
            continue;
        }
        let Some((bus, vid, pid)) = hid.props.get("HID_ID").and_then(|id| parse_hid_id(id)) else {
            continue;
        };
        let name = hid
            .props
            .get("HID_NAME")
            .map(|n| clean_name(n))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Unknown device {vid}:{pid}"));
        let key = format!("{vid}:{pid}");
        let entry = devices.entry(key.clone()).or_insert_with(|| {
            let mut v = Value::new(&key, format!("{name} ({key}, {bus})"));
            v.attrs.insert("name", name.clone());
            v.attrs.insert("vid", vid.clone());
            v.attrs.insert("pid", pid.clone());
            v
        });
        for input in children(&hid.path, "input") {
            for (prop, tag) in CLASSES {
                if input.props.get(*prop).is_some_and(|v| v == "1") {
                    entry.tags.insert(tag);
                }
            }
        }
    }
    Ok(devices.into_values().collect())
}

/// `HID_ID=0003:0000046D:0000C52B` → (bus name, "046d", "c52b").
fn parse_hid_id(id: &str) -> Option<(&'static str, String, String)> {
    let mut parts = id.split(':');
    let (bus, vid, pid) = (parts.next()?, parts.next()?, parts.next()?);
    let short = |s: &str| {
        let s = s.to_ascii_lowercase();
        let s = s.get(s.len().checked_sub(4)?..)?.to_string();
        s.chars().all(|c| c.is_ascii_hexdigit()).then_some(s)
    };
    let bus = match bus {
        "0003" => "USB",
        "0005" => "Bluetooth",
        "0018" => "I2C",
        _ => "other bus",
    };
    Some((bus, short(vid)?, short(pid)?))
}

/// Device names are free text from the device itself: keep printable
/// characters only and collapse whitespace.
fn clean_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_ids_are_validated_and_normalized() {
        assert_eq!(
            parse_hid_id("0003:0000046D:0000C52B"),
            Some(("USB", "046d".into(), "c52b".into()))
        );
        assert_eq!(
            parse_hid_id("0005:0000046D:0000B023").unwrap().0,
            "Bluetooth"
        );
        assert_eq!(parse_hid_id("0003:00000XYZ:0000C52B"), None);
        assert_eq!(parse_hid_id("0003:46"), None);
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(clean_name("  Evil\n\x1b[31mName\t "), "Evil [31mName");
    }
}
