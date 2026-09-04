use super::*;

/// Every machine has a loopback, whatever else it has.
#[test]
fn the_loopback_is_an_interface() {
    let found = interfaces();
    assert!(
        found.iter().any(|one| one.value == "lo"),
        "no loopback among {found:?} rows",
        found = found.len()
    );
}

/// The name is the value and the port is the note — that way round, because a person types `ht⇥`
/// precisely because they cannot remember 443.
#[test]
fn a_port_is_offered_by_name_with_its_number_beside_it() {
    let found = ports();
    if found.is_empty() {
        return; // A machine with no `/etc/services` has nothing to check.
    }
    let http = found.iter().find(|one| one.value == "http");
    assert_eq!(http.map(|one| one.note.as_str()), Some("80/tcp"));
}

/// A name listed for both TCP and UDP is one offer, not two identical words.
#[test]
fn a_name_is_offered_once() {
    let found = ports();
    let mut names: Vec<&str> = found.iter().map(|one| one.value.as_str()).collect();
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), before, "a port name was offered twice");
}
