use super::*;
use crate::scratch::{dir, scratch};

/// **The heart of it**: holding the lock is what "alive" means, and letting go is what "gone" means.
#[test]
fn a_held_lock_is_what_alive_means() {
    let (_dir, _lock) = scratch();
    dir::open_checked().expect("scratch directory");
    assert!(!alive("alpha"), "nothing has been created yet");

    let held = hold("alpha").expect("lock").expect("nobody else has it");
    assert!(alive("alpha"), "while it is held");

    // A second keeper for the same name is refused, which is how a double-start is prevented.
    assert!(hold("alpha").expect("lock").is_none());

    drop(held);
    assert!(!alive("alpha"), "the moment it is dropped");
}

/// A keeper killed with -9 cannot tidy up, so whoever looks next does it.
#[test]
fn a_dead_tab_is_swept_by_whoever_lists() {
    let (_scratch, _lock) = scratch();
    dir::open_checked().expect("scratch directory");
    // Leftovers with no keeper behind them: exactly what a `kill -9` leaves.
    for extension in ["lock", "sock", "meta", "log"] {
        std::fs::write(dir::path().join(format!("beta.{extension}")), b"").expect("write");
    }
    assert!(!alive("beta"));

    let scratches = list().expect("list");
    assert!(
        scratches.is_empty(),
        "a dead scratch is not listed: {scratches:?}"
    );
    assert!(
        !dir::path().join("beta.sock").exists(),
        "and its leftovers are gone"
    );
}

/// A live scratch is listed, with what it said about itself.
#[test]
fn a_live_tab_is_listed_with_its_meta() {
    let (_dir, _lock) = scratch();
    dir::open_checked().expect("scratch directory");
    let held = hold("gamma").expect("lock").expect("free");
    std::fs::write(
        Paths::new("gamma").meta(),
        Meta {
            cwd: "/tmp/somewhere".into(),
            started: 1000,
            pid: 42,
            keeper: 41,
        }
        .encode(),
    )
    .expect("write");

    let scratches = list().expect("list");
    assert_eq!(scratches.len(), 1);
    assert_eq!(scratches[0].0, "gamma");
    assert_eq!(scratches[0].1.cwd, "/tmp/somewhere");
    assert_eq!(scratches[0].1.pid, 42);
    drop(held);
}

/// Newest first, so the picker opens on what you were just doing.
#[test]
fn the_newest_tab_is_first() {
    let (_dir, _lock) = scratch();
    dir::open_checked().expect("scratch directory");
    let mut held = Vec::new();
    for (name, started) in [("alpha", 100u64), ("beta", 300), ("gamma", 200)] {
        held.push(hold(name).expect("lock").expect("free"));
        let meta = Meta {
            cwd: "/".into(),
            started,
            pid: 1,
            keeper: 1,
        };
        std::fs::write(Paths::new(name).meta(), meta.encode()).expect("write");
    }
    let names: Vec<String> = list().expect("list").into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["beta", "gamma", "alpha"]);
}

/// A torn `.meta` costs the decoration, never the scratch.
#[test]
fn an_unreadable_meta_still_lists_the_tab() {
    let (_dir, _lock) = scratch();
    dir::open_checked().expect("scratch directory");
    let held = hold("delta").expect("lock").expect("free");
    std::fs::write(
        Paths::new("delta").meta(),
        b"this is not key=value\n\x00\xff",
    )
    .expect("write");

    let scratches = list().expect("list");
    assert_eq!(scratches.len(), 1, "still listed");
    assert_eq!(scratches[0].1.pid, 0, "with the fields at their defaults");
    drop(held);
}

#[test]
fn meta_round_trips() {
    let meta = Meta {
        cwd: "/home/x/y".into(),
        started: 1_700_000_000,
        pid: 4321,
        keeper: 4320,
    };
    assert_eq!(Meta::decode(&meta.encode()), meta);
}

/// A file in the directory that is not a scratch is not read as one.
#[test]
fn only_lock_files_name_a_tab() {
    let (_scratch, _lock) = scratch();
    dir::open_checked().expect("scratch directory");
    std::fs::write(dir::path().join("notes.txt"), b"").expect("write");
    std::fs::write(dir::path().join("..lock"), b"").expect("write");
    assert!(list().expect("list").is_empty());
}

/// A name nothing is holding is already ended, so ending it is a tidy-up rather than a failure —
/// which is what makes the finder's delete key safe to press on a row that died a moment ago.
#[test]
fn killing_what_is_not_running_tidies_rather_than_fails() {
    let (_dir, _lock) = scratch();
    dir::open_checked().expect("scratch directory");
    std::fs::write(Paths::new("ghost").meta(), Meta::default().encode()).expect("write");

    kill("ghost").expect("a dead scratch is not an error");
    assert!(
        !Paths::new("ghost").meta().exists(),
        "the leftovers went too"
    );
}

/// **What stops a second terminal attaching in silence.** The keeper drops a second client without
/// a word, so the terminal side asks this first — and the answer is the same shape as `alive`: if
/// it can be taken, nobody is looking at that scratch.
#[test]
fn one_terminal_at_a_time_holds_the_attach_lock() {
    let (_dir, _lock) = scratch();
    dir::open_checked().expect("scratch directory");

    let looking = attached("alpha").expect("lock").expect("nobody is in it");
    assert!(
        attached("alpha").expect("lock").is_none(),
        "a second terminal is refused while the first is in there"
    );

    drop(looking);
    assert!(
        attached("alpha").expect("lock").is_some(),
        "and let in the moment the first leaves"
    );
}

/// A scratch's own lock and the terminal looking at it are different questions, so holding one
/// must never answer the other.
#[test]
fn attaching_and_being_alive_are_separate_locks() {
    let (_dir, _lock) = scratch();
    dir::open_checked().expect("scratch directory");

    let held = hold("beta").expect("lock").expect("free");
    assert!(
        attached("beta").expect("lock").is_some(),
        "a running scratch nobody is attached to"
    );
    drop(held);
}

/// **A socket path that will not fit is refused before anything is created.**
///
/// It used to be found at `bind`, four steps too late: the lock, the attach file and the meta file
/// all existed by then, and a shell had been forked to sit behind a socket that would never listen.
/// The keeper exited and left that state under a name `scratch -l` still showed — a scratch that
/// looked real and answered nothing.
#[test]
fn a_socket_path_that_cannot_fit_is_refused_early() {
    let (_dir, _lock) = crate::scratch::scratch();
    // 108 bytes is the whole of `sun_path`; this is comfortably past it.
    let deep = std::path::Path::new("/tmp").join("x".repeat(120));
    // SAFETY: the guard above serialises every test that touches this variable.
    unsafe { std::env::set_var("OSLO_SCRATCH_DIR", &deep) };

    let refused = super::room_for_a_socket("demo").expect_err("a path this long cannot bind");
    let said = refused.to_string();
    assert!(said.contains("108"), "the limit is named: {said}");
    assert!(said.contains("demo.sock"), "the path is named: {said}");
    assert!(said.contains("OSLO_SCRATCH_DIR"), "and what to do: {said}");
}

/// An ordinary directory has room, and is not refused.
#[test]
fn a_short_enough_path_is_allowed() {
    let (_dir, _lock) = crate::scratch::scratch();
    assert!(super::room_for_a_socket("demo").is_ok());
}
