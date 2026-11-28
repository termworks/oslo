//! The audit's probes, as tests: key sources that fail, revisions that pin, links and FIFOs in a
//! store, racing writers, and a store with half its crypto.

use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn store_with(at: &Path, keys: Vec<KeySource>, crypto: Crypto) -> Store {
    let directory = at.join("store");
    std::fs::create_dir_all(&directory).expect("directory");
    Store {
        name: "work".to_string(),
        directory,
        keys,
        recipients: Vec::new(),
        crypto,
        timeout: child::DEFAULT_TIMEOUT,
    }
}

fn with_a_key(at: &Path) -> Store {
    let key = at.join("main.key");
    key::generate(&key).expect("key");
    store_with(at, vec![KeySource::File(key)], Crypto::Native)
}

fn script(at: &Path, name: &str, body: &str) -> String {
    let path = at.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path.to_string_lossy().into_owned()
}

fn header(store: &Store, name: &str, revision: &str, state: &str) -> PathBuf {
    let path = store.path(name).expect("path");
    let line = format!("OSLOSEC1 {revision} {state} 00000000000000000000000000000000\n");
    std::fs::write(&path, line).expect("write");
    path
}

/// The audit's h6: a `key file` that is not there beside a `key command` that fails. The write is
/// refused, names the command, and no key is made at the file's path.
#[test]
fn a_failing_key_command_never_mints_a_key() {
    let dir = tempfile::tempdir().expect("dir");
    let missing = dir.path().join("laptop-key");
    let failing = script(dir.path(), "mkkey.sh", "exit 1");
    let store = store_with(
        dir.path(),
        vec![
            KeySource::File(missing.clone()),
            KeySource::Command(vec![failing]),
        ],
        Crypto::Native,
    );
    let refused = store.set("b", b"value").expect_err("a key was minted");
    assert!(refused.contains("key command"), "{refused}");
    assert!(!missing.exists(), "a key was made beside a key command");
    assert!(!store.path("b").expect("path").exists());
}

/// The audit's h7: the first `key file` cannot be read and the second is not there. The write is
/// refused, names the first, and leaves it exactly as it was.
#[test]
fn an_unreadable_key_file_is_never_replaced() {
    let dir = tempfile::tempdir().expect("dir");
    let main = dir.path().join("keys/main");
    std::fs::create_dir_all(&main).expect("a directory where the key should be");
    let store = store_with(
        dir.path(),
        vec![
            KeySource::File(main.clone()),
            KeySource::File(dir.path().join("usb/backup-key")),
        ],
        Crypto::Native,
    );
    let refused = store.set("b", b"value").expect_err("written anyway");
    assert!(refused.contains("key file"), "{refused}");
    assert!(refused.contains("keys/main"), "{refused}");
    assert!(main.is_dir());
    assert!(!dir.path().join("usb/backup-key").exists());

    // The same with a real key file nobody may read, where the test is not running as root.
    let locked = dir.path().join("locked.key");
    let original = key::generate(&locked).expect("key");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    if std::fs::read(&locked).is_err() {
        let store = store_with(
            dir.path(),
            vec![
                KeySource::File(locked.clone()),
                KeySource::File(dir.path().join("spare")),
            ],
            Crypto::Native,
        );
        assert!(store.set("c", b"value").is_err());
    }
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let text = std::fs::read_to_string(&locked).expect("read");
    assert_eq!(native::read_secret(&text).expect("a key"), original);
}

/// Making a key never replaces anything at its path.
#[test]
fn generating_a_key_refuses_an_existing_path() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("key");
    std::fs::write(&path, b"somebody's file").expect("write");
    assert!(key::generate(&path).is_err());
    assert_eq!(std::fs::read(&path).expect("read"), b"somebody's file");
}

/// The audit's h4: a revision at `u64::MAX` is a damaged header, so `set` and `rm` refuse it
/// rather than reporting success while nothing could move.
#[test]
fn an_absurd_revision_is_damage_not_a_pin() {
    let dir = tempfile::tempdir().expect("dir");
    let store = with_a_key(dir.path());
    let path = header(&store, "pinned", &u64::MAX.to_string(), "live");
    let before = std::fs::read(&path).expect("read");

    for refused in [
        store.set("pinned", b"new").expect_err("set"),
        store.forget("pinned").expect_err("rm reported success"),
        store.get("pinned").expect_err("get"),
    ] {
        assert!(refused.contains("damaged header"), "{refused}");
    }
    assert_eq!(std::fs::read(&path).expect("read"), before);
    assert!(store.names().is_empty());

    // At the bound itself the header is sound, and still nothing claims to have moved it.
    let path = header(&store, "top", &sync::MAX_REVISION.to_string(), "live");
    assert!(store.set("top", b"new").is_err());
    assert!(store.forget("top").is_err());
    assert!(
        std::fs::read_to_string(&path)
            .expect("read")
            .contains(" live ")
    );
}

/// The audit's h5: a far end offering a tombstone at `u64::MAX` cannot pin a name dead for good.
/// The merge stops before writing anything.
#[test]
fn a_merge_refuses_a_pinned_tombstone() {
    let dir = tempfile::tempdir().expect("dir");
    let here = with_a_key(dir.path());
    here.set("tok", b"mine").expect("set");
    let there = Store {
        directory: dir.path().join("peer"),
        ..here.clone()
    };
    std::fs::create_dir_all(&there.directory).expect("dir");
    header(&there, "tok", &u64::MAX.to_string(), "dead");

    let refused = sync::merge(&here, &there, false).expect_err("merged");
    assert!(refused.contains("damaged header"), "{refused}");
    assert_eq!(here.get("tok").expect("get"), b"mine");
}

/// The audit's h8: many writers at once. Every file stays whole, and the revision counts every
/// write, so none was lost under another.
#[test]
fn parallel_sets_leave_every_value_readable() {
    let dir = tempfile::tempdir().expect("dir");
    let store = with_a_key(dir.path());
    std::thread::scope(|scope| {
        for writer in 0..8 {
            let store = &store;
            scope.spawn(move || {
                for round in 0..10 {
                    store
                        .set("c", format!("{writer}-{round}").as_bytes())
                        .expect("set");
                }
            });
        }
    });
    let value = String::from_utf8(store.get("c").expect("readable")).expect("text");
    assert!(value.contains('-'), "{value}");
    let kept = sync::read_kept(&store.path("c").expect("path"))
        .expect("read")
        .expect("there");
    assert_eq!(kept.stamp.revision, 80, "a write was lost under another");
}

/// A link or a FIFO named like a secret is neither listed nor read, and nothing hangs on it.
#[test]
fn links_and_fifos_in_a_store_are_passed_over() {
    let dir = tempfile::tempdir().expect("dir");
    let store = with_a_key(dir.path());
    store.set("real", b"value").expect("set");
    std::os::unix::fs::symlink("/dev/zero", store.directory.join("zero.sealed")).expect("link");
    nix::unistd::mkfifo(
        &store.directory.join("ff.sealed"),
        nix::sys::stat::Mode::S_IRWXU,
    )
    .expect("mkfifo");
    std::fs::write(store.directory.join("\x1b[2Jx.sealed"), b"x").expect("write");

    assert_eq!(store.names(), ["real"]);
    assert!(
        store
            .get("zero")
            .expect_err("followed")
            .contains("symbolic link")
    );
    assert!(store.get("ff").expect_err("opened").contains("FIFO"));
}

/// The audit's h9: a write never lands through a link planted in the store. A link at
/// `NAME.new` is ignored; a link at the secret's own name is refused, and named.
#[test]
fn a_planted_link_does_not_carry_a_write_elsewhere() {
    let dir = tempfile::tempdir().expect("dir");
    let store = with_a_key(dir.path());
    let target = dir.path().join("authorized_keys");
    let original = b"ssh-ed25519 AAAA mine\n";
    std::fs::write(&target, original).expect("write");
    std::os::unix::fs::symlink(&target, store.directory.join("deploy.new")).expect("link");
    store.set("deploy", b"value").expect("set");
    assert_eq!(store.get("deploy").expect("get"), b"value");

    std::os::unix::fs::symlink(&target, store.directory.join("other.sealed")).expect("link");
    let refused = store.set("other", b"value").expect_err("written through");
    assert!(refused.contains("symbolic link"), "{refused}");
    assert_eq!(std::fs::read(&target).expect("read"), original);
}

/// A store that hands over only one half of its crypto cannot do the other with oslo's own.
#[test]
fn half_a_cipher_is_an_error_not_a_fallback() {
    let dir = tempfile::tempdir().expect("dir");
    let cat = vec!["cat".to_string()];
    let writes = store_with(
        dir.path(),
        Vec::new(),
        Crypto::Command(Cipher {
            encrypt: Some(cat.clone()),
            decrypt: None,
        }),
    );
    writes.set("one", b"value").expect("set");
    let refused = writes.get("one").expect_err("read with oslo's own");
    assert!(refused.contains("no `decrypt command`"), "{refused}");

    let reads = Store {
        crypto: Crypto::Command(Cipher {
            encrypt: None,
            decrypt: Some(cat),
        }),
        ..writes
    };
    let refused = reads
        .set("two", b"value")
        .expect_err("written with oslo's own");
    assert!(refused.contains("no `encrypt command`"), "{refused}");
    assert!(!reads.path("two").expect("path").exists());
}

/// A key command that stalls is stopped at the store's `key timeout`.
#[test]
fn a_stalled_key_command_is_stopped() {
    let dir = tempfile::tempdir().expect("dir");
    let stall = script(
        dir.path(),
        "stall.sh",
        "echo 'touch your key' >&2; sleep 10",
    );
    let store = Store {
        timeout: std::time::Duration::from_millis(300),
        ..store_with(
            dir.path(),
            vec![KeySource::Command(vec![stall])],
            Crypto::Native,
        )
    };
    let started = std::time::Instant::now();
    let refused = store.set("x", b"v").expect_err("it waited");
    assert!(refused.contains("no answer after 300ms"), "{refused}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

/// A store directory oslo did not choose is never chmodded, and every write is private.
#[test]
fn a_chosen_directory_keeps_its_mode_and_files_are_private() {
    let dir = tempfile::tempdir().expect("dir");
    let store = with_a_key(dir.path());
    std::fs::set_permissions(&store.directory, std::fs::Permissions::from_mode(0o755))
        .expect("chmod");
    store.set("x", b"v").expect("set");
    let mode = |path: &Path| std::fs::metadata(path).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode(&store.directory), 0o755);
    assert_eq!(mode(&store.path("x").expect("path")), 0o600);
}
