//! Loading the shipped data, and precise errors for invalid input.

use std::path::{Path, PathBuf};

use sancho_core::validate::Problems;
use sancho_core::{format_errors, parse_primitive, parse_recipe, Library, LoadError};

fn repo_data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn shipped() -> Library {
    Library::load(&repo_data())
        .unwrap_or_else(|e| panic!("data/ failed to load:\n{}", format_errors(&e)))
}

#[derive(Debug, Clone, Copy)]
enum At {
    Field(&'static str),
    Line(usize),
}

/// Asserts that one error is at this field (or line) with a reason containing `reason`.
#[track_caller]
fn assert_has(errors: &[LoadError], at: At, reason: &str) {
    let found = errors.iter().any(|e| {
        let place = match at {
            At::Field(f) => e.field.as_deref() == Some(f),
            At::Line(l) => e.position.map(|(line, _)| line) == Some(l),
        };
        place && e.reason.contains(reason) && e.file == Path::new("t.toml")
    });
    assert!(
        found,
        "no error at {at:?} containing {reason:?}; got:\n{}",
        format_errors(errors)
    );
}

/// Applies each `(from, to)` edit to `base` and checks the expected error.
#[track_caller]
fn check_cases(
    base: &str,
    cases: &[(&str, &str, At, &str)],
    errors: impl Fn(&str) -> Vec<LoadError>,
) {
    for (from, to, at, reason) in cases {
        assert!(base.contains(from), "bad test case, not in base: {from}");
        assert_has(&errors(&base.replacen(from, to, 1)), *at, reason);
    }
}

#[test]
fn shipped_data_loads() {
    let lib = shipped();
    let expected = [
        "aur_helper.upgrade",
        "file.write_root",
        "ip.link",
        "lsusb.list",
        "nmcli.radio_wifi",
        "paccache.keep",
        "pacman.install",
        "pacman.remove",
        "pacman.sync_upgrade",
        "systemctl.disable",
        "systemctl.enable",
        "systemctl.restart",
        "systemctl.start",
        "systemctl.status",
        "systemctl.stop",
        "udevadm.reload",
        "udevadm.trigger",
        "usermod.add_group",
    ];
    assert_eq!(lib.primitives.keys().collect::<Vec<_>>(), expected);
}

#[test]
fn valid_recipe_fixtures_validate_against_shipped_primitives() {
    let lib = shipped();
    for name in ["device.grant_hid_browser_access", "system.update_aur"] {
        let file = fixtures().join(format!("recipes/{name}.toml"));
        let src = std::fs::read_to_string(&file).unwrap();
        let recipe = parse_recipe(&file, &src).unwrap_or_else(|e| panic!("{}", format_errors(&e)));
        let mut p = Problems::default();
        recipe.validate(&mut p, &lib.primitives);
        assert!(p.is_empty(), "{name}: {:?}", p.0);
    }
}

// ---- primitives ----

const GOOD_PRIMITIVE: &str = r#"id = "systemctl.enable"
summary = "Enable a unit"
command = ["systemctl", "enable", "{unit}"]
needs_root = true
risk = "medium"
read_only = false
effects = [{ kind = "service_enablement", target = "{unit}" }]

[args]
unit = { type = "systemd_unit", help = "Unit" }
"#;

fn prim_errors(src: &str) -> Vec<LoadError> {
    match parse_primitive(Path::new("t.toml"), src) {
        Ok(_) => panic!("expected errors for:\n{src}"),
        Err(e) => e,
    }
}

#[test]
fn good_primitive_parses() {
    parse_primitive(Path::new("t.toml"), GOOD_PRIMITIVE).unwrap();
}

#[test]
fn syntax_and_type_errors_have_line_and_reason() {
    use At::Line;
    check_cases(
        GOOD_PRIMITIVE,
        &[
            (
                r#"risk = "medium""#,
                r#"risk = "extreme""#,
                Line(5),
                "unknown variant `extreme`",
            ),
            (
                "needs_root = true",
                r#"needs_root = "yes""#,
                Line(4),
                "invalid type",
            ),
            (
                r#"type = "systemd_unit""#,
                r#"type = "unit""#,
                Line(10),
                "unknown variant `unit`",
            ),
            (
                r#"help = "Unit" }"#,
                r#"help = "Unit", hint = "x" }"#,
                Line(10),
                "unknown field `hint`",
            ),
            (
                r#"risk = "medium""#,
                "risk = \"medium\"\nrisk = \"high\"",
                Line(6),
                "duplicate key",
            ),
            (
                r#"target = "{unit}""#,
                r#"target = "{Unit}""#,
                Line(7),
                "invalid placeholder",
            ),
            (
                "summary = \"Enable a unit\"\n",
                "",
                Line(1),
                "missing field `summary`",
            ),
            ("needs_root = true", "needs_root = ", Line(4), ""),
        ],
        prim_errors,
    );
}

#[test]
fn error_display_has_file_position_field_and_reason() {
    let e = prim_errors(&GOOD_PRIMITIVE.replace(r#""medium""#, r#""extreme""#));
    assert!(
        e[0].to_string()
            .starts_with("t.toml:5:8: unknown variant `extreme`"),
        "{}",
        e[0]
    );
    let e = prim_errors(&GOOD_PRIMITIVE.replace(r#"id = "systemctl.enable""#, r#"id = "enable""#));
    assert_eq!(
        e[0].to_string(),
        "t.toml: id: 'enable' is not a valid id (expected e.g. 'systemctl.enable')"
    );
}

#[test]
fn semantic_errors_name_the_field() {
    use At::Field;
    check_cases(
        GOOD_PRIMITIVE,
        &[
            (
                r#"target = "{unit}""#,
                r#"target = "{units}""#,
                Field("effects[0].target"),
                "{units} is not an argument",
            ),
            (
                r#""{unit}"]"#,
                r#""x{unit}"]"#,
                Field("command[2]"),
                "whole token",
            ),
            (
                r#"["systemctl", "enable", "{unit}"]"#,
                r#"["{unit}", "enable"]"#,
                Field("command[0]"),
                "program name",
            ),
            (
                r#"["systemctl", "enable", "{unit}"]"#,
                "[]",
                Field("command"),
                "must not be empty",
            ),
            (
                r#""{unit}"]"#,
                r#""{unit}", "{unit}"]"#,
                Field("args.unit"),
                "more than once",
            ),
            (
                r#", "{unit}"]"#,
                "]",
                Field("args.unit"),
                "not used in command",
            ),
            (
                r#""{unit}"]"#,
                r#""{unt}"]"#,
                Field("command[2]"),
                "{unt} is not an argument",
            ),
            (
                "read_only = false",
                "read_only = true",
                Field("effects"),
                "must not declare effects",
            ),
            (
                r#"help = "Unit" }"#,
                r#"help = "Unit", values = ["a"] }"#,
                Field("args.unit.values"),
                "only allowed for type = \"choice\"",
            ),
            (
                r#"type = "systemd_unit""#,
                r#"type = "text""#,
                Field("args.unit"),
                "text args must use stdin",
            ),
            (
                r#"type = "systemd_unit""#,
                r#"type = "choice""#,
                Field("args.unit.values"),
                "at least one value",
            ),
            (
                r#"type = "systemd_unit""#,
                r#"type = "choice", values = ["on", "--force"]"#,
                Field("args.unit.values[1]"),
                "not start with '-'",
            ),
            (
                r#"needs_root = true"#,
                "needs_root = true\ndry_run = [\"systemctl\", \"--dry\"]",
                Field("dry_run"),
                "same args as command",
            ),
            (
                "unit = {",
                "Unit = {",
                Field("args.Unit"),
                "lowercase identifiers",
            ),
        ],
        prim_errors,
    );
}

#[test]
fn read_only_primitive_needs_no_effects_and_mutating_one_does() {
    let effects = "effects = [{ kind = \"service_enablement\", target = \"{unit}\" }]\n";
    let ro = GOOD_PRIMITIVE
        .replace("read_only = false", "read_only = true")
        .replace(effects, "");
    parse_primitive(Path::new("t.toml"), &ro).unwrap();
    assert_has(
        &prim_errors(&GOOD_PRIMITIVE.replace(effects, "")),
        At::Field("effects"),
        "at least one effect",
    );
}

// ---- recipes ----

const GOOD_RECIPE: &str = r#"id = "service.enable"
version = 1
summary = "Enable a service and start it now"
patterns = ["enable {unit}"]
risk = "medium"
tests = { verified_by = "container" }

[slots]
unit = { type = "systemd_unit" }

[[steps]]
primitive = "systemctl.enable"
args = { unit = "{unit}" }
explain = "Start {unit} at boot"

[[steps]]
primitive = "systemctl.start"
args = { unit = "{unit}" }
explain = "Start {unit} now"
"#;

fn recipe_errors(src: &str) -> Vec<LoadError> {
    let recipe = match parse_recipe(Path::new("t.toml"), src) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let mut p = Problems::default();
    recipe.validate(&mut p, &shipped().primitives);
    assert!(!p.is_empty(), "expected errors for:\n{src}");
    p.0.into_iter()
        .map(|(field, reason)| LoadError {
            file: "t.toml".into(),
            position: None,
            field: Some(field),
            reason,
        })
        .collect()
}

#[test]
fn good_recipe_validates() {
    let r = parse_recipe(Path::new("t.toml"), GOOD_RECIPE).unwrap();
    let mut p = Problems::default();
    r.validate(&mut p, &shipped().primitives);
    assert!(p.is_empty(), "{:?}", p.0);
}

#[test]
fn recipe_errors_name_the_field() {
    use At::{Field, Line};
    check_cases(
        GOOD_RECIPE,
        &[
            (
                r#""systemctl.enable""#,
                r#""systemctl.enabel""#,
                Field("steps[0].primitive"),
                "unknown primitive 'systemctl.enabel'",
            ),
            (
                r#"args = { unit = "{unit}" }"#,
                "",
                Field("steps[0].args"),
                "missing arg 'unit'",
            ),
            (
                r#"{ unit = "{unit}" }"#,
                r#"{ unit = "{unit}", mode = "x" }"#,
                Field("steps[0].args.mode"),
                "has no arg 'mode'",
            ),
            (
                r#"{ unit = "{unit}" }"#,
                r#"{ unit = "sshd.service" }"#,
                Field("steps[0].args.unit"),
                "must come from a slot",
            ),
            (
                r#"{ unit = "{unit}" }"#,
                r#"{ unit = "{unit}.service" }"#,
                Field("steps[0].args.unit"),
                "exactly one slot or literal",
            ),
            (
                r#"unit = { type = "systemd_unit" }"#,
                r#"unit = { type = "package" }"#,
                Field("steps[0].args.unit"),
                "slot 'unit' is package but the arg expects systemd_unit",
            ),
            (
                r#""Start {unit} now""#,
                r#""Start {svc} now""#,
                Field("steps[1].explain"),
                "{svc} is not a slot",
            ),
            (
                r#"["enable {unit}"]"#,
                r#"["enable {service}"]"#,
                Field("patterns[0]"),
                "{service} is not a slot",
            ),
            (
                r#"["enable {unit}"]"#,
                "[]",
                Field("patterns"),
                "at least one pattern",
            ),
            (
                r#"risk = "medium""#,
                r#"risk = "low""#,
                Field("risk"),
                "lower than step primitive systemctl.enable",
            ),
            (
                r#"{ verified_by = "container" }"#,
                r#"{ verified_by = "mock" }"#,
                Field("tests.reason"),
                "reason is required",
            ),
            (
                r#""service.enable""#,
                r#""Service.Enable""#,
                Field("id"),
                "not a valid id",
            ),
            ("version = 1", "version = 0", Field("version"), "at least 1"),
            (
                r#"{ type = "systemd_unit" }"#,
                r#"{ type = "systemd_unit", filter = { webhid = true } }"#,
                Field("slots.unit.filter.webhid"),
                "no 'webhid' filter",
            ),
            (
                r#"{ type = "systemd_unit" }"#,
                "{ type = \"systemd_unit\" }\nextra = { type = \"package\" }",
                Field("slots.extra"),
                "never used",
            ),
            (
                r#""Start {unit} now""#,
                r#""Start {profile.init} now""#,
                Field("steps[1].explain"),
                "requires does not constrain it",
            ),
            (
                r#""Start {unit} now""#,
                r#""Start {profile.kernel} now""#,
                Field("steps[1].explain"),
                "not a profile fact",
            ),
            (
                "tests = ",
                "requires = { kernel = [\"linux\"] }\ntests = ",
                Line(6),
                "unknown variant `kernel`",
            ),
            (
                "tests = ",
                "requires = { init = \"systemd\" }\ntests = ",
                Line(6),
                "invalid type",
            ),
            (
                "tests = ",
                "requires = { init = [] }\ntests = ",
                Field("requires.init"),
                "at least one",
            ),
        ],
        recipe_errors,
    );
}

#[test]
fn compound_slots_need_attributes_and_paths_stay_absolute() {
    use At::Field;
    let base =
        std::fs::read_to_string(fixtures().join("recipes/device.grant_hid_browser_access.toml"))
            .unwrap();
    check_cases(
        &base,
        &[
            (
                "from {browser.name}",
                "from {browser}",
                Field("steps[0].explain"),
                "needs an attribute",
            ),
            (
                "{device.name} now",
                "{device.serial} now",
                Field("steps[2].explain"),
                "no attribute 'serial'",
            ),
            (
                r#"path = "/etc"#,
                r#"path = "etc"#,
                Field("steps[0].args.path"),
                "must be absolute",
            ),
            (
                "/rules.d/70-sancho-",
                "/../70-sancho-",
                Field("steps[0].args.path"),
                "'..'",
            ),
            (
                r#"mode = "0644""#,
                r#"mode = "0999""#,
                Field("steps[0].args.mode"),
                "octal",
            ),
            (
                r#"subsystem = "hidraw""#,
                r#"subsystem = "block""#,
                Field("steps[2].args.subsystem"),
                "not one of",
            ),
        ],
        recipe_errors,
    );
}

// ---- library ----

#[test]
fn bad_library_reports_every_problem_with_its_file() {
    let errors = Library::load(&fixtures().join("bad_library")).expect_err("must not load");
    let text = format_errors(&errors);
    let expect = [
        ("primitives/extra.yml", "only *.toml files belong here"),
        ("primitives/ip.link.toml", "expected 'ip.links.toml'"),
        (
            "recipes/usb.list.toml",
            "steps[0].primitive: unknown primitive 'lsusb.all'",
        ),
    ];
    for (file, reason) in expect {
        assert!(
            errors
                .iter()
                .any(|e| e.file.ends_with(file) && e.to_string().contains(reason)),
            "missing {file}: {reason}; got:\n{text}"
        );
    }
    assert_eq!(errors.len(), expect.len(), "{text}");
}

#[test]
fn missing_primitives_dir_is_an_error() {
    let errors = Library::load(Path::new("/nonexistent/sancho-data")).unwrap_err();
    assert!(errors[0].reason.contains("cannot read directory"));
}
