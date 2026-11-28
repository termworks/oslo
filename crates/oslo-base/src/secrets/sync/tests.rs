use super::*;
use crate::secrets::{Crypto, KeySource};

/// A store on disk with a key of its own, so a test can seal and open for real.
fn store(at: &Path, name: &str) -> Store {
    let directory = at.join(name);
    std::fs::create_dir_all(&directory).expect("directory");
    let key = at.join(format!("{name}.key"));
    crate::secrets::key::generate(&key).expect("key");
    Store {
        name: name.to_string(),
        directory,
        keys: vec![KeySource::File(key)],
        recipients: Vec::new(),
        crypto: Crypto::Native,
        timeout: crate::secrets::child::DEFAULT_TIMEOUT,
    }
}

/// The same store on two machines: one key, two directories, so what one seals the other opens.
fn pair(at: &Path) -> (Store, Store) {
    let here = store(at, "here");
    let there = Store {
        name: "there".to_string(),
        directory: at.join("there"),
        keys: here.keys.clone(),
        recipients: here.recipients.clone(),
        crypto: Crypto::Native,
        timeout: here.timeout,
    };
    std::fs::create_dir_all(&there.directory).expect("directory");
    (here, there)
}

#[test]
fn a_header_survives_being_written_down() {
    let kept = Kept {
        stamp: Stamp {
            revision: 7,
            deleted: true,
            tie_breaker: [3; 16],
        },
        sealed: vec![9, 8, 7],
    };
    let read = unwrap(&wrap(&kept)).expect("a header");
    assert_eq!(read, kept);
}

/// A file written before there were stamps opens, and counts as never having been synced.
#[test]
fn a_body_with_no_header_is_revision_one() {
    let read = unwrap(b"OSLO2 and then some sealed bytes").expect("no header");
    assert_eq!(read.stamp.revision, 1);
    assert!(!read.stamp.deleted);
    assert_eq!(read.sealed, b"OSLO2 and then some sealed bytes");
}

/// **The point of the header being outside the ciphertext**: reading a stamp needs no key at all.
#[test]
fn a_stamp_is_readable_without_the_key() {
    let dir = tempfile::tempdir().expect("dir");
    let (here, _) = pair(dir.path());
    here.set("token", b"hunter2").expect("set");

    let raw = std::fs::read(here.path("token").expect("path")).expect("read");
    assert!(raw.starts_with(b"OSLOSEC1 "), "no header");
    let kept = unwrap(&raw).expect("a header");
    assert_eq!(kept.stamp.revision, 1);
    // And the body is genuinely sealed — the value is not sitting in the file.
    assert!(
        !raw.windows(7).any(|window| window == b"hunter2"),
        "the value is in the clear"
    );
}

#[test]
fn both_ends_hold_the_union_and_the_values_open() {
    let dir = tempfile::tempdir().expect("dir");
    let (here, there) = pair(dir.path());
    here.set("deploy", b"one").expect("set");
    there.set("api", b"two").expect("set");

    let report = merge(&here, &there, false).expect("merge");
    assert_eq!(report.added_left, 1, "{report:?}");
    assert_eq!(report.added_right, 1, "{report:?}");

    // Carried as ciphertext, and still openable on the machine that received it.
    assert_eq!(there.get("deploy").expect("get"), b"one");
    assert_eq!(here.get("api").expect("get"), b"two");
}

/// The question the whole design exists for.
#[test]
fn a_deletion_travels_and_stays_gone() {
    let dir = tempfile::tempdir().expect("dir");
    let (here, there) = pair(dir.path());
    here.set("deploy", b"one").expect("set");
    merge(&here, &there, false).expect("first");
    assert_eq!(there.get("deploy").expect("get"), b"one");

    here.forget("deploy").expect("forget");
    let report = merge(&here, &there, false).expect("second");
    assert_eq!(report.deleted_right, 1, "{report:?}");
    assert!(there.get("deploy").is_err(), "still readable");
    assert!(!there.names().iter().any(|name| name == "deploy"));

    let again = merge(&here, &there, false).expect("third");
    assert!(again.quiet(), "{again:?}");
    assert!(here.get("deploy").is_err());
    assert!(there.get("deploy").is_err());
}

#[test]
fn merging_again_moves_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let (here, there) = pair(dir.path());
    here.set("deploy", b"one").expect("set");
    merge(&here, &there, false).expect("first");
    let again = merge(&here, &there, false).expect("second");
    assert!(again.quiet(), "{again:?}");
}

/// A later write beats the copy that has not heard about it.
#[test]
fn the_later_write_wins() {
    let dir = tempfile::tempdir().expect("dir");
    let (here, there) = pair(dir.path());
    here.set("deploy", b"one").expect("set");
    merge(&here, &there, false).expect("first");

    there.set("deploy", b"two").expect("set");
    merge(&here, &there, false).expect("second");
    assert_eq!(here.get("deploy").expect("get"), b"two");
}

/// Writing a name somebody deleted brings it back rather than being undone by the next sync.
#[test]
fn writing_over_a_tombstone_revives_it() {
    let dir = tempfile::tempdir().expect("dir");
    let (here, there) = pair(dir.path());
    here.set("deploy", b"one").expect("set");
    merge(&here, &there, false).expect("first");
    here.forget("deploy").expect("forget");
    merge(&here, &there, false).expect("second");

    there.set("deploy", b"three").expect("set");
    merge(&here, &there, false).expect("third");
    assert_eq!(here.get("deploy").expect("get"), b"three");
}

#[test]
fn a_dry_run_changes_neither_store() {
    let dir = tempfile::tempdir().expect("dir");
    let (here, there) = pair(dir.path());
    here.set("deploy", b"one").expect("set");

    let report = merge(&here, &there, true).expect("dry");
    assert_eq!(report.added_right, 1, "{report:?}");
    assert!(there.get("deploy").is_err(), "a dry run wrote something");
}

/// Forgetting something that is not there says so, rather than writing a tombstone for a name
/// nobody ever used.
#[test]
fn forgetting_nothing_is_an_error() {
    let dir = tempfile::tempdir().expect("dir");
    let (here, _) = pair(dir.path());
    assert!(here.forget("never-existed").is_err());
    here.set("deploy", b"one").expect("set");
    here.forget("deploy").expect("forget");
    assert!(here.forget("deploy").is_err(), "forgot it twice");
}
