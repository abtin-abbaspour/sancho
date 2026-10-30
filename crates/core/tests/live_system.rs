//! Read-only queries against the machine running the tests.

use sancho_core::{LiveSystem, Query, System};

#[test]
fn a_unit_pattern_matching_nothing_is_an_empty_list_not_a_failure() {
    if LiveSystem.query(Query::UnitFiles(None)).is_none() {
        eprintln!("skipping: systemctl is not available here");
        return;
    }
    let none = LiveSystem.query(Query::UnitFiles(Some("*sancho-no-such-unit*".into())));
    assert_eq!(none.as_deref(), Some(""));
}
