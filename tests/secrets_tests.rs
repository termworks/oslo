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
        .env_remove("OSLO_SECRET_CONF")
        .env_remove("OSLO_SECRET_STORE")
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
    // The `.lock` a write holds is the only other thing there, and it is empty.
    let stray: Vec<_> = std::fs::read_dir(&secrets)
        .expect("a store")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.ends_with(".sealed") && name != ".lock")
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

/// `get`, `rm` and `run` on a name that is not there say so, never the raw error with the path.
#[test]
fn a_missing_name_reads_as_no_such_secret() {
    let store = tempfile::tempdir().expect("tempdir");
    for args in [
        &["get", "nope"][..],
        &["rm", "nope"],
        &["run", "X=nope", "--", "true"],
    ] {
        let (_, err, status) = secret(store.path(), args, b"");
        assert_eq!(status, 1, "{args:?}: {err}");
        assert!(err.contains("nope: no such secret"), "{args:?}: {err}");
        assert!(!err.contains("os error"), "{args:?}: {err}");
    }
}

/// A name that would write to the terminal is refused on the way in, and a file somebody put in
/// the store under one is never listed.
#[test]
#[cfg(feature = "crypt")]
fn a_name_cannot_carry_an_escape_sequence() {
    let store = tempfile::tempdir().expect("tempdir");
    let (_, err, status) = secret(store.path(), &["set", "\x1b[2Jwiped"], b"value");
    assert_eq!(status, 1, "{err}");
    assert!(!err.contains('\x1b'), "the escape reached stderr: {err:?}");

    secret(store.path(), &["set", "fine"], b"value");
    let planted = store.path().join("oslo/secrets/\x1b]0;owned\x07.sealed");
    std::fs::copy(store.path().join("oslo/secrets/fine.sealed"), planted).expect("copy");
    let (out, _, _) = secret(store.path(), &["list"], b"");
    assert_eq!(out, "fine\n");
}

/// The audit's h8, across processes: first `set`s racing to make the profile key agree on one key,
/// and writers racing on one name leave it whole. Every value written is readable after.
#[test]
#[cfg(feature = "crypt")]
fn racing_writers_leave_every_secret_readable() {
    let store = tempfile::tempdir().expect("tempdir");
    let racers: Vec<_> = (0..8)
        .map(|n| (format!("first-{n}"), format!("value-{n}")))
        .chain((0..8).map(|n| ("shared".to_string(), format!("shared-{n}"))))
        .collect();
    std::thread::scope(|scope| {
        for (name, value) in &racers {
            let home = store.path();
            scope.spawn(move || {
                let (_, err, status) = secret(home, &["set", name], value.as_bytes());
                assert_eq!(status, 0, "{name}: {err}");
            });
        }
    });
    for n in 0..8 {
        let (out, err, status) = secret(store.path(), &["get", &format!("first-{n}")], b"");
        assert_eq!(status, 0, "first-{n} is unreadable: {err}");
        assert_eq!(out, format!("value-{n}"));
    }
    let (out, err, status) = secret(store.path(), &["get", "shared"], b"");
    assert_eq!(status, 0, "{err}");
    assert!(out.starts_with("shared-"), "{out:?}");
}

/// The store, `$XDG_STATE_HOME/oslo` and `profiles/` are private, and a loose store oslo made is
/// tightened on the next write.
#[test]
#[cfg(feature = "crypt")]
fn the_directories_are_private_and_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let store = tempfile::tempdir().expect("tempdir");
    let mode = |path: &str| {
        std::fs::metadata(store.path().join(path))
            .expect(path)
            .permissions()
            .mode()
            & 0o777
    };
    secret(store.path(), &["set", "token"], b"value");
    for path in ["oslo/secrets", "state/oslo", "state/oslo/profiles"] {
        assert_eq!(mode(path), 0o700, "{path}");
    }
    let secrets = store.path().join("oslo/secrets");
    std::fs::set_permissions(&secrets, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    secret(store.path(), &["set", "other"], b"value");
    assert_eq!(mode("oslo/secrets"), 0o700);
    // A key an older oslo left in loose directories: a read tightens them.
    for path in ["state/oslo", "state/oslo/profiles"] {
        let loose = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(store.path().join(path), loose).expect("chmod");
    }
    secret(store.path(), &["get", "token"], b"");
    for path in ["state/oslo", "state/oslo/profiles"] {
        assert_eq!(mode(path), 0o700, "{path} after a read");
    }
}

/// `where` names the key this store really reads: the profile's, at its real path.
#[test]
#[cfg(feature = "crypt")]
fn where_names_the_key_in_use() {
    let store = tempfile::tempdir().expect("tempdir");
    let (out, err, status) = secret(store.path(), &["where"], b"");
    assert_eq!(status, 0, "{err}");
    let key = store.path().join("state/oslo/profiles/default.key");
    assert!(out.contains(&key.display().to_string()), "{out}");
    assert!(!out.contains("state/oslo/key "), "{out}");
}

/// Run `oslo secret …` with a `secrets.conf` of the test's own and `$HOME` at the store.
#[cfg(feature = "crypt")]
fn configured(home: &std::path::Path, conf: &str, args: &[&str]) -> (String, String, i32) {
    let state = home.join("state/oslo");
    std::fs::create_dir_all(&state).expect("mkdir");
    std::fs::write(state.join("secrets.conf"), conf).expect("write");
    let out = Command::new(oslo_bin())
        .arg("secret")
        .args(args)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("OSLO_SECRET_IDENTITY")
        .env_remove("OSLO_SECRET_CONF")
        .stdin(Stdio::null())
        .output()
        .expect("spawn oslo");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// A relative `key file` or `directory` is refused; `~/` means `$HOME`, not a directory called `~`.
#[test]
#[cfg(feature = "crypt")]
fn a_conf_path_is_absolute_or_from_home() {
    let home = tempfile::tempdir().expect("tempdir");
    for line in ["key file keys/work", "directory secrets/work"] {
        let (_, err, status) = configured(home.path(), &format!("[work]\n{line}\n"), &["list"]);
        assert_ne!(status, 0, "{line} was accepted");
        assert!(err.contains("relative"), "{line}: {err}");
    }
    let conf = "[work]\nkey file ~/keys/work\ndirectory ~/kept\n";
    let (out, err, status) = configured(home.path(), conf, &["--store", "work", "where"]);
    assert_eq!(status, 0, "{err}");
    let key = home.path().join("keys/work");
    assert!(out.contains(&key.display().to_string()), "{out}");
    assert!(
        out.contains(&home.path().join("kept").display().to_string()),
        "{out}"
    );
    assert!(!out.contains('~'), "{out}");
}

/// `$OSLO_SECRET_IDENTITY` is held to the same rule: a relative one is refused, not read from
/// whatever directory the command happens to run in.
#[test]
fn a_relative_identity_is_refused() {
    let store = tempfile::tempdir().expect("tempdir");
    let out = Command::new(oslo_bin())
        .args(["secret", "set", "token"])
        .env("XDG_DATA_HOME", store.path())
        .env("XDG_STATE_HOME", store.path().join("state"))
        .env("OSLO_SECRET_IDENTITY", "keys/identity")
        .current_dir(store.path())
        .stdin(Stdio::null())
        .output()
        .expect("spawn oslo");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("relative"));
    assert!(!store.path().join("keys/identity").exists());
}

/// A key file inside a repository is called out, whichever source names it.
#[test]
#[cfg(feature = "crypt")]
fn a_key_file_inside_a_repository_is_called_out() {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(home.path().join("dots/.git")).expect("mkdir");
    std::fs::write(home.path().join("dots/.git/HEAD"), "ref: refs/heads/main\n").expect("HEAD");
    let conf = format!(
        "[work]\nkey file {}\n",
        home.path().join("dots/k").display()
    );
    let (_, err, _) = configured(home.path(), &conf, &["--store", "work", "list"]);
    assert!(err.contains("inside the git repository"), "{err}");
}

/// A `key command` that stalls is stopped at `key timeout`, and what it says on standard error
/// reaches the terminal rather than being swallowed.
#[test]
#[cfg(feature = "crypt")]
fn a_stalled_key_command_is_stopped_and_heard() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().expect("tempdir");
    let stall = home.path().join("touch.sh");
    // `exec`, so the program killed is the one holding standard error open.
    std::fs::write(
        &stall,
        "#!/bin/sh\necho 'touch your key' >&2\nexec sleep 20\n",
    )
    .expect("write");
    std::fs::set_permissions(&stall, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let conf = format!("[yk]\nkey command {}\nkey timeout 1s\n", stall.display());
    let started = std::time::Instant::now();
    let (_, err, status) = configured(home.path(), &conf, &["--store", "yk", "set", "x"]);
    assert_ne!(status, 0);
    assert!(err.contains("touch your key"), "{err}");
    assert!(err.contains("no answer after 1s"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}
