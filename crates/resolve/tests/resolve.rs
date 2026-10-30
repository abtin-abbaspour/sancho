//! Resolvers against fixture systems.

use std::collections::BTreeMap;
use std::path::Path;

use sancho_core::recipe::{FilterValue, SlotSpec};
use sancho_core::{FixtureSystem, Query, ValueType};
use sancho_resolve::{enumerate, resolve, validate, Resolution, ResolveError, Value};

fn system(name: &str) -> FixtureSystem {
    FixtureSystem::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../eval/fixtures/systems")
            .join(name),
    )
}

fn slot(ty: ValueType, filter: &[(&str, FilterValue)]) -> SlotSpec {
    SlotSpec {
        ty,
        values: vec![],
        filter: filter
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
        multiple: false,
    }
}

fn run(sys: &str, ty: ValueType, mention: Option<&str>) -> Resolution {
    resolve(&system(sys), &slot(ty, &[]), mention).unwrap()
}

#[track_caller]
fn selected(r: Resolution) -> Value {
    match r {
        Resolution::Selected(v) => v,
        other => panic!("expected a selection, got {other:?}"),
    }
}

#[track_caller]
fn asked(r: Resolution) -> Vec<(String, String)> {
    match r {
        Resolution::Ask(c) => c.into_iter().map(|c| (c.id, c.value.display)).collect(),
        other => panic!("expected a question, got {other:?}"),
    }
}

#[track_caller]
fn no_match(r: Resolution) -> String {
    match r {
        Resolution::NoMatch(reason) => reason,
        other => panic!("expected no match, got {other:?}"),
    }
}

// ---- acceptance: "my keyboard" ----

#[test]
fn one_keyboard_is_auto_selected() {
    let v = selected(run(
        "hid_one_keyboard",
        ValueType::HidDevice,
        Some("my keyboard"),
    ));
    assert_eq!(v.key, "3434:0121");
    assert_eq!(v.attrs["name"], "Keychron Q2");
    assert_eq!(v.attrs["vid"], "3434");
    assert_eq!(v.attrs["pid"], "0121");
}

#[test]
fn two_keyboards_ask_with_both_names() {
    assert_eq!(
        asked(run(
            "hid_two_keyboards",
            ValueType::HidDevice,
            Some("my keyboard")
        )),
        [
            ("c1".into(), "Keychron Q2 (3434:0121, USB)".into()),
            ("c2".into(), "Logitech G815 RGB (046d:c33f, USB)".into()),
        ]
    );
}

#[test]
fn keyboard_and_mouse_excludes_the_mouse() {
    let v = selected(run(
        "hid_keyboard_and_mouse",
        ValueType::HidDevice,
        Some("my keyboard"),
    ));
    assert_eq!(v.attrs["name"], "Keychron Q2");
    let v = selected(run(
        "hid_keyboard_and_mouse",
        ValueType::HidDevice,
        Some("the mouse"),
    ));
    assert_eq!(v.attrs["name"], "Logitech G502 HERO");
}

// ---- hid_device details ----

#[test]
fn hid_interfaces_group_into_one_device_and_serio_is_ignored() {
    let all = enumerate(&system("hid_one_keyboard"), ValueType::HidDevice).unwrap();
    assert_eq!(all.len(), 1, "{all:?}");
}

#[test]
fn hid_names_and_recipe_filters() {
    let sys = system("hid_keyboard_and_mouse");
    let v = selected(run(
        "hid_keyboard_and_mouse",
        ValueType::HidDevice,
        Some("g502"),
    ));
    assert_eq!(v.key, "046d:c08b");
    let mice = slot(
        ValueType::HidDevice,
        &[("input_class", FilterValue::String("mouse".into()))],
    );
    let v = selected(resolve(&sys, &mice, None).unwrap());
    assert_eq!(v.attrs["name"], "Logitech G502 HERO");
    assert!(no_match(resolve(&sys, &mice, Some("keychron")).unwrap()).contains("keychron"));
}

#[test]
fn devices_reporting_both_classes_cause_a_question_not_a_guess() {
    // Captured hardware: keyboards with a mouse interface and a headset
    // receiver with a keyboard interface all count as keyboards.
    let names: Vec<String> = asked(run(
        "captured_desktop",
        ValueType::HidDevice,
        Some("my keyboard"),
    ))
    .into_iter()
    .map(|(_, d)| d)
    .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names
        .iter()
        .any(|n| n.starts_with("Wireless Headset Receiver")));
    assert!(names.iter().any(|n| n.starts_with("Keyboard A")));
}

// ---- other types ----

#[test]
fn units_follow_systemctl_naming() {
    let v = selected(run("workstation", ValueType::SystemdUnit, Some("sshd")));
    assert_eq!(v.key, "sshd.service");
    // An explicit suffix is exact.
    let v = selected(run(
        "workstation",
        ValueType::SystemdUnit,
        Some("cups.socket"),
    ));
    assert_eq!(v.key, "cups.socket");
    // Without a suffix systemctl means .service, so the socket is not offered.
    let v = selected(run("workstation", ValueType::SystemdUnit, Some("cups")));
    assert_eq!(v.key, "cups.service");
    let all = enumerate(&system("workstation"), ValueType::SystemdUnit).unwrap();
    let keys: Vec<&str> = all.iter().map(|v| v.key.as_str()).collect();
    assert!(!keys.contains(&"getty@.service"), "templates are skipped");
    assert!(
        !keys.contains(&"-evil.service"),
        "option-like names are dropped"
    );
}

#[test]
fn enabled_filter_selects_only_enabled_units() {
    let sys = system("workstation");
    let enabled = slot(
        ValueType::SystemdUnit,
        &[("enabled", FilterValue::Bool(true))],
    );
    assert!(no_match(resolve(&sys, &enabled, Some("sshd")).unwrap()).contains("sshd"));
    let v = selected(resolve(&sys, &enabled, Some("bluetooth")).unwrap());
    assert_eq!(v.key, "bluetooth.service");
}

#[test]
fn packages_resolve_exactly_and_by_installed_state() {
    let sys = system("workstation");
    let v = selected(run("workstation", ValueType::Package, Some("vlc")));
    assert!(v.tags.contains("installed"));
    // Exact name wins over the many packages containing "firefox".
    assert_eq!(
        selected(run("workstation", ValueType::Package, Some("firefox"))).key,
        "firefox"
    );
    let available = slot(
        ValueType::Package,
        &[("installed", FilterValue::Bool(false))],
    );
    let v = selected(resolve(&sys, &available, Some("chromium")).unwrap());
    assert_eq!(v.display, "chromium 131.0.6778.85-1 (extra)");
    assert!(no_match(resolve(&sys, &available, Some("git")).unwrap()).contains("git"));
    let v = selected(run("workstation", ValueType::Package, Some("paru-bin")));
    assert_eq!(
        v.display,
        "paru-bin 2.0.4-1 (installed, not from a repository)"
    );
    assert!(validate(&sys, ValueType::Package, "Bad_Name")
        .unwrap()
        .is_none());
}

#[test]
fn partial_package_names_select_a_unique_match_or_ask() {
    let sys = system("workstation");
    let v = selected(run("workstation", ValueType::Package, Some("firefox i18n")));
    assert_eq!(v.key, "firefox-i18n-de");
    // The installed `firefox` is filtered out, so its exact name no longer wins.
    let available = slot(
        ValueType::Package,
        &[("installed", FilterValue::Bool(false))],
    );
    assert_eq!(
        asked(resolve(&sys, &available, Some("firefox")).unwrap()),
        [
            (
                "c1".into(),
                "firefox-developer-edition 134.0b9-1 (extra)".into()
            ),
            ("c2".into(), "firefox-i18n-de 133.0-1 (extra)".into()),
        ]
    );
}

#[test]
fn network_interfaces_are_physical_and_classified() {
    let all = enumerate(&system("workstation"), ValueType::NetIface).unwrap();
    let keys: Vec<&str> = all.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(keys, ["enp3s0", "wlan0"], "lo and docker0 are virtual");
    assert_eq!(
        selected(run("workstation", ValueType::NetIface, Some("wifi"))).key,
        "wlan0"
    );
    assert_eq!(
        selected(run("workstation", ValueType::NetIface, Some("ethernet"))).key,
        "enp3s0"
    );
    assert_eq!(
        asked(run("workstation", ValueType::NetIface, None)).len(),
        2
    );
}

#[test]
fn browsers_come_from_desktop_entries() {
    let sys = system("workstation");
    let all = enumerate(&sys, ValueType::Browser).unwrap();
    let names: Vec<&str> = all.iter().map(|v| v.display.as_str()).collect();
    assert_eq!(
        names,
        ["Chromium", "Firefox"],
        "hidden and non-browser entries skipped"
    );
    let webhid = slot(ValueType::Browser, &[("webhid", FilterValue::Bool(true))]);
    let v = selected(resolve(&sys, &webhid, None).unwrap());
    assert_eq!(v.attrs["desktop_id"], "chromium.desktop");
    assert!(no_match(resolve(&sys, &webhid, Some("firefox")).unwrap()).contains("firefox"));
}

#[test]
fn users_default_to_the_current_user() {
    let v = selected(run("workstation", ValueType::User, None));
    assert_eq!(v.key, "alice");
    assert_eq!(
        selected(run("workstation", ValueType::User, Some("me"))).key,
        "alice"
    );
    assert_eq!(
        selected(run("workstation", ValueType::User, Some("bob"))).key,
        "bob"
    );
    // System and no-login accounts are not users you can name.
    assert!(matches!(
        run("workstation", ValueType::User, Some("http")),
        Resolution::NoMatch(_)
    ));
    assert!(matches!(
        run("workstation", ValueType::User, Some("backup")),
        Resolution::NoMatch(_)
    ));
}

#[test]
fn groups_resolve_by_name() {
    assert_eq!(
        selected(run("workstation", ValueType::UserGroup, Some("video"))).key,
        "video"
    );
    assert!(matches!(
        run("workstation", ValueType::UserGroup, Some("plugdev")),
        Resolution::NoMatch(_)
    ));
}

// ---- errors and limits ----

#[test]
fn missing_query_output_is_an_error_not_an_empty_list() {
    let err = resolve(
        &system("workstation"),
        &slot(ValueType::HidDevice, &[]),
        None,
    )
    .unwrap_err();
    assert_eq!(err, ResolveError::Unavailable(Query::UdevDb));
    assert_eq!(err.to_string(), "could not run `udevadm info --export-db`");
}

#[test]
fn literal_types_are_not_resolved() {
    let err = resolve(&system("workstation"), &slot(ValueType::Path, &[]), None).unwrap_err();
    assert_eq!(err, ResolveError::NotResolvable(ValueType::Path));
}

#[test]
fn too_many_candidates_ask_for_detail() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("many_units");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("queries")).unwrap();
    let units: String = (1..=12)
        .map(|i| {
            format!(
                "svc{i}.service disabled disabled
"
            )
        })
        .collect();
    std::fs::write(dir.join("queries/unit-files.txt"), units).unwrap();
    let r = resolve(
        &FixtureSystem::new(&dir),
        &slot(ValueType::SystemdUnit, &[]),
        Some("svc"),
    )
    .unwrap();
    assert!(no_match(r).contains("be more specific"));
}
