use super::*;
use crate::macros::{Entry, Kind, get, put, remove};

fn store(at: &std::path::Path, name: &str) -> Store {
    Store::open(&at.join(format!("{name}.db"))).expect("open")
}

fn alias(store: &Store, name: &str, body: &str) {
    put(store, &Entry::new(Kind::Alias, name, body)).expect("put");
}

fn body_of(store: &Store, name: &str) -> Option<String> {
    get(store, Kind::Alias, name).map(|entry| entry.body)
}

/// The plain case: each machine has something the other has not, and both end up with both.
#[test]
fn both_ends_hold_the_union() {
    let dir = tempfile::tempdir().expect("dir");
    let here = store(dir.path(), "here");
    let there = store(dir.path(), "there");
    alias(&here, "gs", "git status");
    alias(&there, "ll", "ls -la");

    let report = merge(&here, &there, false).expect("merge");
    assert_eq!(report.added_left, 1, "{report:?}");
    assert_eq!(report.added_right, 1, "{report:?}");

    assert_eq!(body_of(&here, "ll").as_deref(), Some("ls -la"));
    assert_eq!(body_of(&there, "gs").as_deref(), Some("git status"));
}

/// **The question this whole design exists for.** Deleting on one machine must remove it on the
/// other, and must not come back on the sync after that.
#[test]
fn a_deletion_travels_and_stays_gone() {
    let dir = tempfile::tempdir().expect("dir");
    let here = store(dir.path(), "here");
    let there = store(dir.path(), "there");
    alias(&here, "gs", "git status");
    merge(&here, &there, false).expect("first");
    assert_eq!(body_of(&there, "gs").as_deref(), Some("git status"));

    assert!(remove(&here, Kind::Alias, "gs"), "removed here");
    let report = merge(&here, &there, false).expect("second");
    assert_eq!(report.deleted_right, 1, "{report:?}");
    assert_eq!(body_of(&there, "gs"), None, "still there");

    // And the machine that lost it does not hand it back.
    let again = merge(&here, &there, false).expect("third");
    assert!(again.quiet(), "{again:?}");
    assert_eq!(body_of(&here, "gs"), None);
    assert_eq!(body_of(&there, "gs"), None);
}

/// Syncing twice changes nothing the second time, so it is safe in a login file.
#[test]
fn merging_again_moves_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let here = store(dir.path(), "here");
    let there = store(dir.path(), "there");
    alias(&here, "gs", "git status");
    alias(&there, "ll", "ls -la");

    merge(&here, &there, false).expect("first");
    let again = merge(&here, &there, false).expect("second");
    assert!(again.quiet(), "{again:?}");
    assert_eq!(again.unchanged, 2, "{again:?}");
}

/// Both machines edited the same name. One wins, both agree which, and it does not matter which end
/// asked — that is what makes the sync order-independent.
#[test]
fn the_same_name_edited_on_both_ends_settles_the_same_way() {
    let dir = tempfile::tempdir().expect("dir");
    let here = store(dir.path(), "here");
    let there = store(dir.path(), "there");
    alias(&here, "gs", "git status --short");
    alias(&there, "gs", "git status --branch");

    merge(&here, &there, false).expect("merge");
    let winner = body_of(&here, "gs");
    assert_eq!(winner, body_of(&there, "gs"), "the two ends disagree");
    assert!(winner.is_some());

    // Asked from the other side, the same record survives.
    let again = merge(&there, &here, false).expect("again");
    assert!(again.quiet(), "{again:?}");
    assert_eq!(body_of(&here, "gs"), winner);
}

/// An edit after a sync beats the copy that has not heard about it, in either direction.
#[test]
fn the_later_edit_wins() {
    let dir = tempfile::tempdir().expect("dir");
    let here = store(dir.path(), "here");
    let there = store(dir.path(), "there");
    alias(&here, "gs", "git status");
    merge(&here, &there, false).expect("first");

    alias(&there, "gs", "git status --short");
    merge(&here, &there, false).expect("second");
    assert_eq!(body_of(&here, "gs").as_deref(), Some("git status --short"));
}

/// Writing a name somebody deleted brings it back, rather than being undone by the next sync.
#[test]
fn writing_over_a_tombstone_revives_it() {
    let dir = tempfile::tempdir().expect("dir");
    let here = store(dir.path(), "here");
    let there = store(dir.path(), "there");
    alias(&here, "gs", "git status");
    merge(&here, &there, false).expect("first");
    assert!(remove(&here, Kind::Alias, "gs"));
    merge(&here, &there, false).expect("second");
    assert_eq!(body_of(&there, "gs"), None);

    alias(&there, "gs", "git switch");
    merge(&here, &there, false).expect("third");
    assert_eq!(body_of(&here, "gs").as_deref(), Some("git switch"));
    assert_eq!(body_of(&there, "gs").as_deref(), Some("git switch"));
}

/// A dry run says what would happen and writes nothing to either side.
#[test]
fn a_dry_run_changes_neither_store() {
    let dir = tempfile::tempdir().expect("dir");
    let here = store(dir.path(), "here");
    let there = store(dir.path(), "there");
    alias(&here, "gs", "git status");

    let report = merge(&here, &there, true).expect("dry");
    assert_eq!(report.added_right, 1, "{report:?}");
    assert_eq!(body_of(&there, "gs"), None, "a dry run wrote something");
}

/// Every database kind syncs, not only the two a starting shell reads. Scripts are files and travel
/// through [`merge_scripts`] — see `scripts::ledger`'s tests.
#[test]
fn every_kind_travels() {
    let dir = tempfile::tempdir().expect("dir");
    let here = store(dir.path(), "here");
    let there = store(dir.path(), "there");
    let kinds: Vec<Kind> = Kind::every()
        .iter()
        .copied()
        .filter(|kind| *kind != Kind::Script)
        .collect();
    for kind in kinds.iter().copied() {
        put(&here, &Entry::new(kind, "thing", "the body")).expect("put");
    }

    merge(&here, &there, false).expect("merge");
    for kind in kinds.iter().copied() {
        assert!(
            get(&there, kind, "thing").is_some(),
            "{} did not travel",
            kind.word()
        );
    }
}
