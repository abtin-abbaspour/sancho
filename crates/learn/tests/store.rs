//! The learned store on disk, and export text.

use std::path::PathBuf;

use sancho_learn::scrub::{scrub, Secrets};
use sancho_learn::{export_text, Learned, Store};

fn path(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("learned")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("learned.json")
}

fn alias(phrase: &str, recipe: &str) -> Learned {
    Learned::Alias {
        phrase: phrase.into(),
        recipe: recipe.into(),
    }
}

fn keyboard_pref(key: &str, display: &str) -> Learned {
    Learned::Preference {
        recipe: "device.grant_hid_browser_access".into(),
        slot: "device".into(),
        mention: "keyboard".into(),
        key: key.into(),
        display: display.into(),
    }
}

#[test]
fn items_persist_and_can_be_removed() {
    let p = path("persist");
    let mut s = Store::open(&p).unwrap();
    let a = s
        .learn(alias("bring my system up to date", "system.update"))
        .unwrap();
    let k = s.learn(keyboard_pref("3434:0121", "Keychron Q2")).unwrap();
    s.used(a).unwrap();

    let s = Store::open(&p).unwrap();
    assert_eq!(s.items().len(), 2);
    assert_eq!(s.alias("bring my system up to date").unwrap().uses, 1);
    assert_eq!(
        s.preference("device.grant_hid_browser_access", "device", "keyboard")
            .unwrap()
            .id,
        k
    );
    assert!(s.alias("update my system").is_none());

    let mut s = s;
    assert!(s.remove(a).unwrap());
    assert!(!s.remove(a).unwrap(), "already gone");
    assert_eq!(Store::open(&p).unwrap().items().len(), 1);
}

#[test]
fn a_new_answer_replaces_the_old_one_for_the_same_question() {
    let mut s = Store::open(path("replace")).unwrap();
    s.learn(keyboard_pref("3434:0121", "Keychron Q2")).unwrap();
    let new = s
        .learn(keyboard_pref("046d:c33f", "Logitech G815"))
        .unwrap();
    assert_eq!(s.items().len(), 1);
    assert_eq!(s.items()[0].id, new);
    assert_eq!(
        s.items()[0].describe(),
        "for device.grant_hid_browser_access, device with \"keyboard\" means Logitech G815"
    );
}

#[test]
fn export_text_is_scrubbed_and_states_the_license() {
    let mut s = Store::open(path("export")).unwrap();
    let id = s
        .learn(alias(
            "let alice's chrome on alice-laptop see my keyboard",
            "device.grant_hid_browser_access",
        ))
        .unwrap();
    let text = export_text(s.get(id).unwrap(), "0.0.1");
    let secrets = Secrets {
        users: vec!["alice".into()],
        hostnames: vec!["alice-laptop".into()],
        ssids: vec![],
    };
    let out = scrub(&text, &secrets);
    assert!(out.text.contains("Contributed under MIT OR Apache-2.0"));
    assert!(
        out.text
            .contains("phrase = \"let <user>'s chrome on <hostname> see my keyboard\""),
        "{}",
        out.text
    );
    assert!(!out.text.contains("alice"));
}

#[test]
fn a_corrupt_store_is_an_error_not_silently_empty() {
    let p = path("corrupt");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "{ not json").unwrap();
    assert!(Store::open(&p).is_err());
}
