use super::*;
use crate::macros::scripts::testdir;
use crate::macros::sync::merge_scripts_at;

fn store_in(dir: &Path, name: &str) -> Store {
    Store::open(&dir.join(name)).expect("open")
}

fn place(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write");
    path
}

fn sync_once(dir: &Path, theirs: &Store, ledger: &Path) -> crate::macros::sync::MacroReport {
    let held = tempfile::tempdir_in(dir).expect("tempdir");
    let scratch = store_in(held.path(), "scratch.db");
    merge_scripts_at(&scratch, theirs, ledger, false).expect("merge")
}

/// A script never synced is stamped with when it was written, and stamping again changes nothing.
#[test]
fn a_new_script_is_stamped_with_when_it_was_written() {
    let own = testdir::own();
    let held = tempfile::tempdir().expect("tempdir");
    let ledger = held.path().join("scripts.ledger");
    let path = place(own.dir.path(), "tool", "#!/bin/sh\n");

    let first = reconcile(&load(&ledger));
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].stamp.revision, super::super::modified(&path));
    assert!(!first[0].stamp.deleted);
    assert_eq!(
        reconcile(&load(&ledger)),
        first,
        "the same files, the same stamps"
    );
}

/// After a sync an unchanged script keeps its stamp, and an edit moves it on.
#[test]
fn an_edit_moves_the_revision_on() {
    let own = testdir::own();
    let held = tempfile::tempdir().expect("tempdir");
    let ledger = held.path().join("scripts.ledger");
    place(own.dir.path(), "tool", "#!/bin/sh\necho one\n");
    let theirs = store_in(held.path(), "theirs.db");
    sync_once(held.path(), &theirs, &ledger);

    let before = load(&ledger)["tool"].stamp;
    assert_eq!(reconcile(&load(&ledger))[0].stamp, before, "unchanged");
    place(own.dir.path(), "tool", "#!/bin/sh\necho two\n");
    let after = reconcile(&load(&ledger))[0].stamp;
    assert!(after.revision > before.revision);
    assert!(!after.deleted);
}

/// A file gone since the last sync is a tombstone one revision on.
#[test]
fn a_deleted_script_becomes_a_tombstone() {
    let own = testdir::own();
    let held = tempfile::tempdir().expect("tempdir");
    let ledger = held.path().join("scripts.ledger");
    let path = place(own.dir.path(), "tool", "#!/bin/sh\n");
    let theirs = store_in(held.path(), "theirs.db");
    sync_once(held.path(), &theirs, &ledger);
    let before = load(&ledger)["tool"].stamp;

    std::fs::remove_file(path).expect("rm");
    let now = reconcile(&load(&ledger));
    assert_eq!(now.len(), 1);
    assert!(now[0].stamp.deleted);
    assert_eq!(now[0].stamp.revision, before.revision + 1);
}

/// Their new script arrives, their deletion removes ours, our newer edit reaches them, and a
/// second sync has nothing left to do.
#[test]
fn a_merge_carries_scripts_both_ways() {
    let own = testdir::own();
    let held = tempfile::tempdir().expect("tempdir");
    let ledger = held.path().join("scripts.ledger");
    place(own.dir.path(), "kept", "#!/bin/sh\necho one\n");
    place(own.dir.path(), "doomed", "#!/bin/sh\n");
    let theirs = store_in(held.path(), "theirs.db");
    assert_eq!(sync_once(held.path(), &theirs, &ledger).added_right, 2);

    let mut doomed = stored(&theirs, Kind::Script, "doomed").expect("sent");
    doomed.stamp.bury();
    let key_of = |name: &str| key(Kind::Script, name);
    crate::store::set(&theirs, &key_of("doomed"), encode(&doomed).as_bytes()).expect("set");
    let fresh = Entry {
        stamp: Stamp {
            revision: 5,
            deleted: false,
            tie_breaker: [7; 16],
        },
        ..Entry::new(Kind::Script, "fresh", "#!/bin/sh\necho fresh\n")
    };
    crate::store::set(&theirs, &key_of("fresh"), encode(&fresh).as_bytes()).expect("set");
    place(own.dir.path(), "kept", "#!/bin/sh\necho two\n");

    sync_once(held.path(), &theirs, &ledger);
    assert!(!own.dir.path().join("doomed").exists(), "their deletion");
    assert_eq!(
        std::fs::read_to_string(own.dir.path().join("fresh")).expect("arrived"),
        "#!/bin/sh\necho fresh\n"
    );
    assert_eq!(
        stored(&theirs, Kind::Script, "kept").expect("there").body,
        "#!/bin/sh\necho two\n",
        "our edit"
    );
    assert!(sync_once(held.path(), &theirs, &ledger).quiet());
}

/// Stamping replaces every script record a store held, so a record with no file never travels.
#[test]
fn stamping_leaves_only_the_directory_in_the_store() {
    let own = testdir::own();
    let held = tempfile::tempdir().expect("tempdir");
    let ledger = held.path().join("scripts.ledger");
    let store = store_in(held.path(), "copy.db");
    let stale = Entry::new(Kind::Script, "stale", "#!/bin/sh\n");
    crate::store::set(
        &store,
        &key(Kind::Script, "stale"),
        encode(&stale).as_bytes(),
    )
    .expect("set");
    place(own.dir.path(), "real", "#!/bin/sh\n");

    stamp_into_with(&store, &ledger).expect("stamp");
    assert!(stored(&store, Kind::Script, "stale").is_none());
    assert!(stored(&store, Kind::Script, "real").is_some());
}

/// A dry run reads both sides and writes neither.
#[test]
fn a_dry_run_changes_nothing() {
    let own = testdir::own();
    let held = tempfile::tempdir().expect("tempdir");
    let ledger = held.path().join("scripts.ledger");
    let theirs = store_in(held.path(), "theirs.db");
    let arriving = Entry::new(Kind::Script, "arriving", "#!/bin/sh\n");
    crate::store::set(
        &theirs,
        &key(Kind::Script, "arriving"),
        encode(&arriving).as_bytes(),
    )
    .expect("set");

    let scratch = store_in(held.path(), "scratch.db");
    let report = merge_scripts_at(&scratch, &theirs, &ledger, true).expect("merge");
    assert_eq!(report.added_left, 1);
    assert!(!own.dir.path().join("arriving").exists());
    assert!(!ledger.exists());
}

/// A ledger line survives being written and read back.
#[test]
fn a_ledger_line_round_trips() {
    let held = tempfile::tempdir().expect("tempdir");
    let path = held.path().join("scripts.ledger");
    let line = Line {
        stamp: Stamp {
            revision: 1_754_870_400,
            deleted: true,
            tie_breaker: [0xab; 16],
        },
        fingerprint: "f".repeat(64),
    };
    let lines = BTreeMap::from([("git-rel".to_string(), line.clone())]);
    save(&path, &lines).expect("save");
    assert_eq!(load(&path), lines);
}
