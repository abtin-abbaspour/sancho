//! Answers read from a fixture system.

use std::path::Path;

use sancho_core::recipe::Answer;
use sancho_core::FixtureSystem;
use sancho_facts::{answer, Fact};

fn sys() -> FixtureSystem {
    FixtureSystem::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/fixtures/systems/eval_desktop"),
    )
}

fn rows(f: &Fact) -> Vec<(&str, &str)> {
    f.rows
        .iter()
        .map(|(l, v)| (l.as_str(), v.as_str()))
        .collect()
}

#[test]
fn network_devices_name_vendor_model_and_driver() {
    let f = answer(&sys(), Answer::NetworkDevices, None);
    assert_eq!(
        rows(&f),
        [
            ("enp3s0 (wired)", "Realtek Semiconductor Co., Ltd. RTL8125 2.5GbE Controller (driver r8169)"),
            ("wlan0 (Wi-Fi)", "Intel Corporation Wi-Fi 6E(802.11ax) AX210/AX1675* 2x2 [Typhoon Peak] (driver iwlwifi)"),
        ],
        "lo and docker0 are virtual"
    );
}

#[test]
fn hardware_and_system_facts() {
    let s = sys();
    assert_eq!(
        rows(&answer(&s, Answer::Cpu, None)),
        [
            ("Model", "AMD Ryzen 7 5800X 8-Core Processor"),
            ("Threads", "2")
        ]
    );
    assert_eq!(
        rows(&answer(&s, Answer::Gpu, None)),
        [("card0", "Advanced Micro Devices, Inc. [AMD/ATI] Navi 21 [Radeon RX 6800/6800 XT / 6900 XT] (driver amdgpu)")],
        "connectors like card0-HDMI-A-1 are not devices"
    );
    assert_eq!(
        rows(&answer(&s, Answer::Memory, None)),
        [
            ("Total", "31.2 GiB"),
            ("Available", "20.0 GiB"),
            ("Swap", "none")
        ]
    );
    assert_eq!(
        rows(&answer(&s, Answer::Kernel, None)),
        [("Release", "6.11.9-arch1-1")]
    );
    assert_eq!(
        rows(&answer(&s, Answer::Hostname, None)),
        [("Name", "workstation")]
    );
    assert_eq!(
        rows(&answer(&s, Answer::Battery, None)),
        [("BAT0", "87%, discharging")]
    );
    assert_eq!(
        rows(&answer(&s, Answer::Os, None))[0],
        ("Name", "Arch Linux")
    );
}

#[test]
fn addresses_skip_loopback_and_link_local() {
    assert_eq!(
        rows(&answer(&sys(), Answer::IpAddresses, None)),
        [("wlan0", "192.168.1.20/24")]
    );
}

#[test]
fn groups_include_the_primary_group_and_memberships() {
    let f = answer(&sys(), Answer::MyGroups, None);
    assert_eq!(
        rows(&f),
        [("User", "alice"), ("Groups", "wheel video alice")]
    );
}

#[test]
fn service_status_reads_systemd() {
    let unit =
        sancho_resolve::validate(&sys(), sancho_core::ValueType::SystemdUnit, "sshd.service")
            .unwrap()
            .unwrap();
    assert_eq!(
        rows(&answer(&sys(), Answer::ServiceStatus, Some(&unit))),
        [
            ("Unit", "sshd.service"),
            ("Description", "OpenSSH Daemon"),
            ("Running", "no (inactive)"),
            ("At boot", "does not start at boot"),
        ]
    );
}

#[test]
fn missing_information_is_said_not_guessed() {
    let empty = FixtureSystem::new(std::env::temp_dir().join("sancho-no-such-fixture"));
    for kind in [
        Answer::Cpu,
        Answer::Memory,
        Answer::Kernel,
        Answer::IpAddresses,
    ] {
        let f = answer(&empty, kind, None);
        assert!(
            f.rows.is_empty()
                && f.note
                    .as_deref()
                    .is_some_and(|n| n.contains("couldn't read")),
            "{f:?}"
        );
    }
    let f = answer(&empty, Answer::Battery, None);
    assert!(f.note.unwrap().contains("No battery found"));
}
