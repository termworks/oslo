//! `oslo secret` through the real binary: kept encrypted, handed back whole.
//!
//! **Two features, not one.** `secrets` is the store and its plumbing, where sealing is delegated
//! to the `encrypt`/`decrypt command` a store names; `crypt` adds oslo's own crypto — the identity,
//! the key file, and the native seal. A test that makes a key or round-trips a value needs the
//! second one, and the six that do say so, or `cargo test --features secrets` is red on a build
//! that is behaving exactly as designed. See `oslo_base::secrets::Store::seal_natively`.
#![cfg(feature = "secrets")]

mod common;

use common::oslo_bin;
use std::io::Write;
use std::process::{Command, Stdio};

/// Run `oslo secret …` against a store of its own, with `input` on standard input.
///
/// The key goes under `$XDG_STATE_HOME`, which is a different directory on purpose — both are
/// pointed at the same temporary root here only so a test cleans up after itself.
fn secret(store: &std::path::Path, args: &[&str], input: &[u8]) -> (String, String, i32) {
    let mut child = Command::new(oslo_bin())
        .arg("secret")
        .args(args)
        .env("XDG_DATA_HOME", store)
        .env("XDG_STATE_HOME", store.join("state"))
        .env_remove("OSLO_SECRET_IDENTITY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oslo");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input)
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// The whole point: what goes in comes back out, and what is on disk is not it.
#[test]
#[cfg(feature = "crypt")]
fn a_secret_survives_the_round_trip_without_being_written_down() {
    let store = tempfile::tempdir().expect("tempdir");
    let value = "hunter2-correct-horse";

    let (_, err, status) = secret(store.path(), &["set", "token"], value.as_bytes());
    assert_eq!(status, 0, "{err}");

    let (out, err, status) = secret(store.path(), &["get", "token"], b"");
    assert_eq!(status, 0, "{err}");
    assert_eq!(out, value, "what came back was not what went in");

    // **The file is the reason this exists.** A store that held the value in the clear would be a
    // `.env` with extra steps.
    let file = store.path().join("oslo/secrets/token.sealed");
    let kept = std::fs::read(&file).expect("the secret was written");
    assert!(
        !String::from_utf8_lossy(&kept).contains(value),
        "the value is on disk in the clear: {}",
        file.display()
    );
}

/// A value is bytes, not a line: the newline a `printf` or a heredoc leaves behind is dropped, and
/// nothing else is. A token with `\n` on the end fails authentication in a way that takes an hour
/// to find.
#[test]
#[cfg(feature = "crypt")]
fn one_trailing_newline_is_dropped_and_no_more() {
    let store = tempfile::tempdir().expect("tempdir");
    secret(store.path(), &["set", "one"], b"value\n");
    secret(store.path(), &["set", "two"], b"value\n\n");

    assert_eq!(secret(store.path(), &["get", "one"], b"").0, "value");
    assert_eq!(secret(store.path(), &["get", "two"], b"").0, "value\n");
}

/// The key is the one thing that must not be readable by anybody else, from the moment it exists.
///
/// It is the *profile's* key: a store derives its own from that rather than keeping a second one,
/// so this is the file that matters on the whole machine.
#[test]
#[cfg(feature = "crypt")]
fn the_identity_is_private_from_the_start() {
    use std::os::unix::fs::PermissionsExt;
    let store = tempfile::tempdir().expect("tempdir");
    secret(store.path(), &["set", "token"], b"value");

    let identity = store.path().join("state/oslo/profiles/default.key");
    let mode = std::fs::metadata(&identity)
        .expect("an identity")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "the key is readable by somebody else: {mode:04o}"
    );
}

/// **The key is not in the store, and that is the whole arrangement.** The store is encrypted so it
/// can go in a dotfiles repository; a key inside it would be committed along with it, which turns
/// the feature into the accident it exists to prevent.
#[test]
#[cfg(feature = "crypt")]
fn the_key_is_not_where_the_secrets_are() {
    let store = tempfile::tempdir().expect("tempdir");
    secret(store.path(), &["set", "token"], b"value");

    let secrets = store.path().join("oslo/secrets");
    assert!(secrets.join("token.sealed").exists(), "the store is here");
    let stray: Vec<_> = std::fs::read_dir(&secrets)
        .expect("a store")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.ends_with(".sealed"))
        .collect();
    assert!(
        stray.is_empty(),
        "something other than ciphertext: {stray:?}"
    );
}

/// `$OSLO_SECRET_IDENTITY` puts the key anywhere — a USB stick, an encrypted volume, `~/.ssh`.
#[test]
#[cfg(feature = "crypt")]
fn the_key_can_be_put_anywhere() {
    let store = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let key = elsewhere.path().join("deep/inside/identity");

    let run = |args: &[&str], input: &[u8]| {
        let mut child = Command::new(oslo_bin())
            .arg("secret")
            .args(args)
            .env("XDG_DATA_HOME", store.path())
            .env("OSLO_SECRET_IDENTITY", &key)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn oslo");
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(input)
            .expect("write");
        let out = child.wait_with_output().expect("wait");
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    run(&["set", "token"], b"value");
    assert!(
        key.exists(),
        "the key was not written where it was asked for"
    );
    assert_eq!(run(&["get", "token"], b""), "value");
}

/// **A key under a `.git` is one `git add -A` from being published**, and the person that happens
/// to did not choose it — they moved a directory a year later. Saying so is the only defence a
/// shell has.
#[test]
fn a_key_inside_a_repository_is_called_out() {
    let store = tempfile::tempdir().expect("tempdir");
    let repository = tempfile::tempdir().expect("tempdir");
    // A repository, not merely a directory with the name — an empty `.git` is neither, and the
    // check knows the difference.
    std::fs::create_dir_all(repository.path().join(".git")).expect("a repository");
    std::fs::write(
        repository.path().join(".git/HEAD"),
        "ref: refs/heads/main\n",
    )
    .expect("a HEAD");
    let key = repository.path().join("state/identity");

    let out = Command::new(oslo_bin())
        .args(["secret", "list"])
        .env("XDG_DATA_HOME", store.path())
        .env("OSLO_SECRET_IDENTITY", &key)
        .output()
        .expect("spawn oslo");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("inside the git repository"), "{said:?}");
    assert!(
        said.contains("OSLO_SECRET_IDENTITY"),
        "a way out is named: {said:?}"
    );
}

/// Names are listed; a name that was forgotten is not.
#[test]
#[cfg(feature = "crypt")]
fn what_is_kept_can_be_listed_and_forgotten() {
    let store = tempfile::tempdir().expect("tempdir");
    secret(store.path(), &["set", "alpha"], b"1");
    secret(store.path(), &["set", "beta"], b"2");

    let (out, _, _) = secret(store.path(), &["list"], b"");
    assert_eq!(out, "alpha\nbeta\n");

    assert_eq!(secret(store.path(), &["rm", "alpha"], b"").2, 0);
    assert_eq!(secret(store.path(), &["list"], b"").0, "beta\n");
}

/// Asking for one that is not there fails rather than printing nothing and succeeding — `$(oslo
/// secret get x)` in a script has to be able to tell those apart.
#[test]
fn a_name_that_is_not_there_is_an_error() {
    let store = tempfile::tempdir().expect("tempdir");
    let (out, _, status) = secret(store.path(), &["get", "missing"], b"");
    assert_eq!(status, 1);
    assert!(out.is_empty());
}

/// A name is a filename, and one that reaches out of the store is refused.
#[test]
fn a_name_cannot_reach_out_of_the_store() {
    let store = tempfile::tempdir().expect("tempdir");
    let (_, err, status) = secret(store.path(), &["set", "../elsewhere"], b"value");
    assert_eq!(status, 1, "{err}");
    assert!(!store.path().join("oslo/elsewhere.age").exists());
}
