//! Profile detection against fixture filesystems, and recipe exclusion.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sancho_core::parse_recipe;
use sancho_core::recipe::ProfileKey;
use sancho_core::FixtureSystem;
use sancho_profile::{detect, Profile};

fn fixture(name: &str) -> FixtureSystem {
    FixtureSystem::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../eval/fixtures/systems")
            .join(name),
    )
}

#[track_caller]
fn assert_profile(profile: &Profile, expected: &[(ProfileKey, &str)]) {
    let actual: Vec<_> = profile.iter().collect();
    let expected: Vec<_> = expected.iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(actual, expected);
}

use ProfileKey::*;

#[test]
fn vanilla_arch_with_networkmanager() {
    assert_profile(
        &detect(&fixture("arch_networkmanager")),
        &[
            (Distro, "arch"),
            (Init, "systemd"),
            // iwd is running too, as NetworkManager's Wi-Fi backend.
            (Network, "networkmanager"),
            (Audio, "pipewire"),
            (Display, "wayland"),
            (AurHelper, "paru"),
            (RootFs, "btrfs"),
            (SnapshotTool, "snapper"),
            (Bootloader, "systemd_boot"),
        ],
    );
}

#[test]
fn arch_with_iwd() {
    assert_profile(
        &detect(&fixture("arch_iwd")),
        &[
            // os-release only in /usr/lib, with a quoted ID.
            (Distro, "arch"),
            (Init, "systemd"),
            (Network, "iwd"),
            (Audio, "pulseaudio"),
            (Display, "x11"),
            // yay exists but is not executable.
            (AurHelper, "none"),
            (RootFs, "ext4"),
            (SnapshotTool, "none"),
            (Bootloader, "grub"),
        ],
    );
}

#[test]
fn artix_with_openrc() {
    assert_profile(
        &detect(&fixture("artix_openrc")),
        &[
            (Distro, "artix"),
            (Init, "openrc"),
            (Network, "connman"),
            (Audio, "none"),
            (Display, "none"),
            (AurHelper, "yay"),
            (RootFs, "ext4"),
            (SnapshotTool, "none"),
            (Bootloader, "grub"),
        ],
    );
}

const SYSTEMD_RECIPE: &str = r#"id = "service.enable"
version = 1
summary = "Enable a service"
patterns = ["enable {unit}"]
risk = "medium"
requires = { init = ["systemd"] }
tests = { verified_by = "container" }

[slots]
unit = { type = "systemd_unit" }

[[steps]]
primitive = "systemctl.enable"
args = { unit = "{unit}" }
explain = "Enable {unit}"
"#;

#[test]
fn systemd_recipe_is_excluded_on_artix_openrc() {
    let recipe = parse_recipe(Path::new("t.toml"), SYSTEMD_RECIPE).unwrap();
    for name in ["arch_networkmanager", "arch_iwd"] {
        assert!(
            detect(&fixture(name)).unmet(&recipe.requires).is_empty(),
            "{name}"
        );
    }
    let unmet = detect(&fixture("artix_openrc")).unmet(&recipe.requires);
    assert_eq!(unmet.len(), 1);
    assert_eq!(unmet[0].key, Init);
    assert_eq!(
        unmet[0].to_string(),
        "needs init systemd, this system has openrc"
    );
}

#[test]
fn any_listed_value_satisfies_a_requirement() {
    let p = detect(&fixture("artix_openrc"));
    let req =
        |vals: &[&str]| BTreeMap::from([(AurHelper, vals.iter().map(|v| v.to_string()).collect())]);
    assert!(p.unmet(&req(&["paru", "yay"])).is_empty());
    assert_eq!(p.unmet(&req(&["paru"])).len(), 1);
    assert!(p.unmet(&BTreeMap::new()).is_empty());
}

// ---- edge cases on scratch trees ----

/// A throwaway fixture: files under `<dir>/root`, environment passed in.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("profile")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("root")).unwrap();
        Scratch(dir)
    }

    fn file(&self, rel: &str, content: &str) -> &Self {
        let path = self.0.join("root").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        self
    }

    fn host(&self, env: &[(&str, &str)]) -> FixtureSystem {
        env.iter()
            .fold(FixtureSystem::new(&self.0), |s, (k, v)| s.with_env(k, v))
    }
}

#[test]
fn empty_system_reports_safe_unknowns() {
    let s = Scratch::new("empty");
    assert_profile(
        &detect(&s.host(&[])),
        &[
            (Distro, "unsupported"),
            (Init, "unknown"),
            (Network, "none"),
            (Audio, "none"),
            (Display, "none"),
            (AurHelper, "none"),
            (RootFs, "other"),
            (SnapshotTool, "none"),
            (Bootloader, "unknown"),
        ],
    );
}

#[test]
fn distro_derivatives_and_non_arch() {
    let cases = [
        ("ID=cachyos\nID_LIKE=arch\n", "cachyos"),
        ("ID=omarchy\nID_LIKE=\"arch\"\n", "arch_derivative"),
        ("ID=arch_derivative\n", "unsupported"),
        ("ID=debian\n", "unsupported"),
        ("ID=ubuntu\nID_LIKE=debian\n", "unsupported"),
    ];
    for (i, (os_release, expected)) in cases.iter().enumerate() {
        let s = Scratch::new(&format!("distro{i}"));
        s.file("etc/os-release", os_release);
        assert_eq!(detect(&s.host(&[])).get(Distro), *expected, "{os_release}");
    }
}

#[test]
fn init_falls_back_to_pid1_name() {
    let s = Scratch::new("init");
    s.file("proc/1/comm", "runit\n");
    assert_eq!(detect(&s.host(&[])).get(Init), "runit");
}

#[test]
fn display_prefers_session_type() {
    let s = Scratch::new("display");
    let h = |env: &[(&str, &str)]| detect(&s.host(env)).get(Display);
    assert_eq!(
        h(&[("XDG_SESSION_TYPE", "x11"), ("WAYLAND_DISPLAY", "w")]),
        "x11"
    );
    assert_eq!(
        h(&[("XDG_SESSION_TYPE", "tty"), ("WAYLAND_DISPLAY", "w")]),
        "wayland"
    );
    assert_eq!(h(&[("DISPLAY", "")]), "none");
}

#[test]
fn snapshot_tool_needs_root_config() {
    let s = Scratch::new("snapper");
    s.file("usr/bin/snapper", "");
    fs::set_permissions(
        s.0.join("root/usr/bin/snapper"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let env = [("PATH", "/usr/bin")];
    assert_eq!(detect(&s.host(&env)).get(SnapshotTool), "none");
    s.file("etc/snapper/configs/root", "");
    assert_eq!(detect(&s.host(&env)).get(SnapshotTool), "snapper");
}

#[test]
fn root_fs_uses_the_last_mount_on_root() {
    let s = Scratch::new("overmount");
    s.file(
        "proc/self/mounts",
        "/dev/sda1 / ext4 rw 0 0\noverlay / overlay rw 0 0\n/dev/sdb1 / xfs rw 0 0\n",
    );
    assert_eq!(detect(&s.host(&[])).get(RootFs), "xfs");
}

#[test]
fn detection_only_reports_known_values() {
    for name in [
        "arch_networkmanager",
        "arch_iwd",
        "artix_openrc",
        "captured_desktop",
    ] {
        for (k, v) in detect(&fixture(name)).iter() {
            assert!(k.values().contains(&v), "{k:?}={v}");
        }
    }
}

#[test]
fn captured_cachyos_desktop() {
    assert_profile(
        &detect(&fixture("captured_desktop")),
        &[
            (Distro, "cachyos"),
            (Init, "systemd"),
            (Network, "networkmanager"),
            (Audio, "pipewire"),
            (Display, "wayland"),
            (AurHelper, "none"),
            (RootFs, "btrfs"),
            (SnapshotTool, "snapper"),
            // /boot is unreadable without root; /etc/default/limine gives it away.
            (Bootloader, "limine"),
        ],
    );
}
