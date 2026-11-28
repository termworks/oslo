//! Stores, keys and recipients through the real binary: several of each, and the fences on them.
#![cfg(feature = "secrets")]

mod common;

use common::oslo_bin;
use std::io::Write;
use std::process::{Command, Stdio};

/// Give a build with no crypto of its own something to encrypt with.
///
/// **Not encryption, and it does not pretend to be**: it is a reversible filter, so what is under
/// test is the plumbing — that the store, the names, `run` and the file layout work when the crypto
/// belongs to somebody else. A build with `crypt` needs none of this and gets none of it.
#[cfg(not(feature = "crypt"))]
fn a_mechanism(home: &std::path::Path) {
    let conf = home.join("state/oslo/secrets.conf");
    if conf.exists() {
        return;
    }
    let filter = home.join("filter");
    std::fs::write(
        &filter,
        "#!/bin/sh\ncase \"$1\" in\n  -e) base64 ;;\n  -d) base64 -d ;;\nesac\n",
    )
    .expect("write");
    std::fs::set_permissions(&filter, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    std::fs::create_dir_all(conf.parent().expect("a parent")).expect("mkdir");
    let filter = filter.to_string_lossy();
    // Every store the suite names, because `secrets.conf` has no section that stands for all of
    // them — which is itself worth knowing, and is why this is a list rather than one entry.
    let mut text = String::new();
    for store in [
        "user", "work", "friend", "byhand", "broken", "yubi", "cmd", "yk",
    ] {
        text.push_str(&format!(
            "[{store}]\nencrypt command {filter} -e\ndecrypt command {filter} -d\n\n"
        ));
    }
    std::fs::write(&conf, text).expect("write");
}

#[cfg(feature = "crypt")]
fn a_mechanism(_home: &std::path::Path) {}

/// Run `oslo secret …` against a store of its own, with `input` on standard input.
fn secret(home: &std::path::Path, args: &[&str], input: &[u8]) -> (String, String, i32) {
    a_mechanism(home);
    let mut child = Command::new(oslo_bin())
        .arg("secret")
        .args(args)
        .env("XDG_DATA_HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("OSLO_SECRET_IDENTITY")
        .env_remove("OSLO_SECRET_STORE")
        .env_remove("OSLO_SECRET_NO_EXEC")
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

/// **A key can be a program**, which is what reaches a password manager, a smartcard, or anything
/// else oslo will never compile in — and `$OSLO_SECRET_NO_EXEC` is the switch that says this
/// invocation will not fork, for a cron job or a container to export once.
#[test]
#[cfg(feature = "crypt")]
fn a_key_can_come_from_a_program_and_can_be_refused() {
    let home = tempfile::tempdir().expect("tempdir");
    secret(home.path(), &["set", "unrelated"], b"v");
    // A program that prints a key, which is the whole contract — there is no per-store key file to
    // `cat` any more, because a store's key comes from the profile unless it is told otherwise.
    let giver = home.path().join("give-key");
    std::fs::write(
        &giver,
        "#!/bin/sh\necho OSLO-KEY-1:YSBkZW1vIGtleSwgdGhpcnR5LXR3byBieXRlcyBsb24=\n",
    )
    .expect("write");
    std::fs::set_permissions(&giver, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");

    let (_, err, status) = secret(
        home.path(),
        &[
            "--store",
            "byhand",
            "key",
            "add",
            "command",
            &giver.to_string_lossy(),
        ],
        b"",
    );
    assert_eq!(status, 0, "{err}");

    let (_, err, status) = secret(home.path(), &["--store", "byhand", "set", "thing"], b"kept");
    assert_eq!(status, 0, "{err}");
    assert_eq!(
        secret(home.path(), &["--store", "byhand", "get", "thing"], b"").0,
        "kept"
    );

    // The same read, with the switch on: refused, and it says which one it did not run.
    let out = Command::new(oslo_bin())
        .args(["secret", "--store", "byhand", "get", "thing"])
        .env("XDG_DATA_HOME", home.path())
        .env("XDG_STATE_HOME", home.path().join("state"))
        .env("OSLO_SECRET_NO_EXEC", "1")
        .output()
        .expect("spawn oslo");
    assert!(!out.status.success(), "it ran the program anyway");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("OSLO_SECRET_NO_EXEC"), "{said:?}");
}

/// **A plugin's store never forks**, whichever door the command comes through.
#[test]
#[cfg(feature = "crypt")]
fn a_plugin_store_may_not_run_a_key_command() {
    let home = tempfile::tempdir().expect("tempdir");
    let (_, err, status) = secret(
        home.path(),
        &[
            "--store",
            "plugin.notes",
            "key",
            "add",
            "command",
            "/bin/cat",
        ],
        b"",
    );
    assert_ne!(status, 0, "it was accepted");
    assert!(err.contains("may not run a command"), "{err:?}");

    // And by hand, in the file the command would have written.
    let conf = home.path().join("state/oslo/secrets.conf");
    std::fs::create_dir_all(conf.parent().expect("a parent")).expect("mkdir");
    std::fs::write(&conf, "[plugin.notes]\nkey command /bin/cat\n").expect("write");
    let (_, err, status) = secret(home.path(), &["--store", "plugin.notes", "list"], b"");
    assert_ne!(status, 0, "the hand-written one was accepted");
    assert!(err.contains("may not run a command"), "{err:?}");
}

/// Which store an invocation means: the argument, then the environment, then the file, then `user`.
#[test]
fn the_store_is_chosen_in_one_order() {
    let home = tempfile::tempdir().expect("tempdir");
    secret(home.path(), &["set", "name"], b"in-user");
    secret(home.path(), &["--store", "work", "set", "name"], b"in-work");

    assert_eq!(secret(home.path(), &["get", "name"], b"").0, "in-user");
    assert_eq!(
        secret(home.path(), &["--store", "work", "get", "name"], b"").0,
        "in-work"
    );

    // The environment, which is what a Makefile or a cron line sets once for everything under it.
    let from_environment = |store: &str| {
        let out = Command::new(oslo_bin())
            .args(["secret", "get", "name"])
            .env("XDG_DATA_HOME", home.path())
            .env("XDG_STATE_HOME", home.path().join("state"))
            .env("OSLO_SECRET_STORE", store)
            .output()
            .expect("spawn oslo");
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    assert_eq!(from_environment("work"), "in-work");

    // And the file, which is what the machine says when nothing else does.
    let conf = home.path().join("state/oslo/secrets.conf");
    let text = std::fs::read_to_string(&conf).unwrap_or_default();
    std::fs::write(&conf, format!("default work\n{text}")).expect("write");
    assert_eq!(secret(home.path(), &["get", "name"], b"").0, "in-work");

    // The argument still wins over both.
    assert_eq!(
        secret(home.path(), &["--store", "user", "get", "name"], b"").0,
        "in-user"
    );
}

/// `run` puts the value in one child and nowhere else — no shell in between to keep a copy.
#[test]
fn run_hands_the_value_to_one_child() {
    let home = tempfile::tempdir().expect("tempdir");
    secret(home.path(), &["set", "gh-token"], b"tok");

    let (out, err, status) = secret(
        home.path(),
        &["run", "TOKEN=gh-token", "--", "sh", "-c", "echo \"$TOKEN\""],
        b"",
    );
    assert_eq!(status, 0, "{err}");
    assert_eq!(out, "tok\n");

    // The name may be left off, and is then the variable, lowercased and hyphenated.
    let (out, err, status) = secret(
        home.path(),
        &["run", "GH_TOKEN=", "--", "sh", "-c", "echo \"$GH_TOKEN\""],
        b"",
    );
    assert_eq!(status, 0, "{err}");
    assert_eq!(out, "tok\n");

    // A secret that is not there is a failure before the command runs, not an empty variable.
    let (_, _, status) = secret(
        home.path(),
        &["run", "X=missing", "--", "sh", "-c", "exit 0"],
        b"",
    );
    assert_ne!(status, 0);
}

/// **A key in hardware never leaves the hardware**, so there is no identity for `key command` to
/// print — and the whole operation goes to a program that can reach it instead. Here that program
/// stands in for `age` calling `age-plugin-yubikey`; what is under test is the plumbing, which is
/// that oslo's own age is not involved at all and everything above it still works.
#[test]
fn a_store_can_hand_its_crypto_to_another_program() {
    let home = tempfile::tempdir().expect("tempdir");
    let filter = home.path().join("pretend-age");
    std::fs::write(
        &filter,
        "#!/bin/sh\ncase \"$1\" in\n  -R) printf 'PRETEND-AGE\\n'; base64 ;;\n  -d) tail -n +2 | base64 -d ;;\nesac\n",
    )
    .expect("write");
    std::fs::set_permissions(&filter, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    let filter = filter.to_string_lossy().to_string();

    for half in ["encrypt", "decrypt"] {
        let flag = if half == "encrypt" { "-R" } else { "-d" };
        let (_, err, status) = secret(
            home.path(),
            &["--store", "yubi", "cipher", half, "--", &filter, flag],
            b"",
        );
        assert_eq!(status, 0, "{err}");
    }

    let (_, err, status) = secret(
        home.path(),
        &["--store", "yubi", "set", "deploy"],
        b"held-in-hardware",
    );
    assert_eq!(status, 0, "{err}");

    // The body is the other program's format — oslo did not write it — behind oslo's own one-line
    // header, which is what a sync reads and is deliberately outside the sealed part.
    let kept = std::fs::read_to_string(home.path().join("oslo/stores/yubi/deploy.sealed"))
        .expect("the secret was written");
    let (header, body) = kept.split_once('\n').expect("a header and a body");
    assert!(header.starts_with("OSLOSEC1 "), "{header:?}");
    assert!(body.starts_with("PRETEND-AGE"), "{body:?}");
    assert!(!kept.contains("held-in-hardware"), "in the clear: {kept:?}");

    assert_eq!(
        secret(home.path(), &["--store", "yubi", "get", "deploy"], b"").0,
        "held-in-hardware"
    );

    // And everything above the store inherits it, `run` included.
    let (out, err, status) = secret(
        home.path(),
        &[
            "--store",
            "yubi",
            "run",
            "TOKEN=deploy",
            "--",
            "sh",
            "-c",
            "echo \"$TOKEN\"",
        ],
        b"",
    );
    assert_eq!(status, 0, "{err}");
    assert_eq!(out, "held-in-hardware\n");

    // A program that fails is an error, never an empty value quietly written down.
    secret(
        home.path(),
        &["--store", "broken", "cipher", "encrypt", "--", "false"],
        b"",
    );
    let (_, err, status) = secret(home.path(), &["--store", "broken", "set", "x"], b"v");
    assert_ne!(status, 0, "a failing encrypter was accepted");
    assert!(err.contains("exited 1"), "{err:?}");
}

/// A plugin's store may not run a command, whichever half of the crypto it is.
#[test]
fn a_plugin_store_may_not_hand_off_its_crypto() {
    let home = tempfile::tempdir().expect("tempdir");
    let (_, err, status) = secret(
        home.path(),
        &[
            "--store",
            "plugin.notes",
            "cipher",
            "decrypt",
            "--",
            "/bin/cat",
        ],
        b"",
    );
    assert_ne!(status, 0, "it was accepted");
    assert!(err.contains("may not run a command"), "{err:?}");
}

/// **Two stores with two keys do not open each other's files.** The built-in mechanism is one key
/// and one AEAD, so this is the whole of what "a store is private" means here.
#[test]
#[cfg(feature = "crypt")]
fn a_key_opens_its_own_store_and_no_other() {
    let home = tempfile::tempdir().expect("tempdir");
    secret(home.path(), &["set", "mine"], b"in-the-user-store");

    // A store with a key of its own.
    let elsewhere = home.path().join("other-key");
    let (_, err, status) = secret(
        home.path(),
        &[
            "--store",
            "work",
            "key",
            "add",
            "file",
            &elsewhere.to_string_lossy(),
        ],
        b"",
    );
    assert_eq!(status, 0, "{err}");
    secret(
        home.path(),
        &["--store", "work", "set", "theirs"],
        b"in-the-work-store",
    );
    assert!(elsewhere.exists(), "the key was not made on first use");

    // Each opens its own…
    assert_eq!(
        secret(home.path(), &["get", "mine"], b"").0,
        "in-the-user-store"
    );
    assert_eq!(
        secret(home.path(), &["--store", "work", "get", "theirs"], b"").0,
        "in-the-work-store"
    );

    // …and the user store's key does not open the other's file, even pointed straight at it.
    let theirs = home.path().join("oslo/stores/work/theirs.sealed");
    let mine = home.path().join("oslo/secrets/theirs.sealed");
    std::fs::copy(&theirs, &mine).expect("copy");
    let (_, err, status) = secret(home.path(), &["get", "theirs"], b"");
    assert_ne!(status, 0, "the wrong key opened it");
    assert!(err.contains("no key opened it"), "{err:?}");
}

/// A key file that is not a key says so, rather than failing later as "wrong key".
#[test]
#[cfg(feature = "crypt")]
fn something_that_is_not_a_key_is_named_as_such() {
    let home = tempfile::tempdir().expect("tempdir");
    let wrong = home.path().join("not-a-key");
    std::fs::write(&wrong, "hello\n").expect("write");
    secret(
        home.path(),
        &[
            "--store",
            "w",
            "key",
            "add",
            "file",
            &wrong.to_string_lossy(),
        ],
        b"",
    );
    let (_, err, status) = secret(home.path(), &["--store", "w", "set", "x"], b"v");
    assert_ne!(status, 0);
    assert!(err.contains("OSLO-KEY-1"), "{err:?}");
}

/// **The whole reason the built-in mechanism is not one symmetric key.** A colleague publishes a
/// recipient, you add it and rotate, and now two keys open the store — with neither of you ever
/// having handed the other a private key.
#[test]
#[cfg(feature = "crypt")]
fn a_second_recipient_reads_it_with_a_key_of_their_own() {
    let mine = tempfile::tempdir().expect("tempdir");
    let theirs = tempfile::tempdir().expect("tempdir");

    // Their key is their *profile's*, and writing one secret is what makes it. The half they would
    // publish comes off the store that key derives.
    secret(
        theirs.path(),
        &["set", "anything"],
        b"so the profile key exists",
    );
    let (out, err, status) = secret(theirs.path(), &["recipient"], b"");
    assert_eq!(status, 0, "{err}");
    let published = out
        .split_whitespace()
        .find(|word| word.starts_with("OSLO-PUB-1:"))
        .expect("a public half")
        .to_string();

    secret(mine.path(), &["set", "token"], b"shared-value");
    let (_, err, status) = secret(mine.path(), &["recipient", "add", &published], b"");
    assert_eq!(status, 0, "{err}");
    // Adding does not reach what is already written; rotate is the deliberate second step.
    let (_, err, status) = secret(mine.path(), &["rotate"], b"");
    assert_eq!(status, 0, "{err}");

    assert_eq!(
        secret(mine.path(), &["get", "token"], b"").0,
        "shared-value"
    );

    // The store travels; the key does not.
    let theirs_store = theirs.path().join("oslo/secrets");
    std::fs::create_dir_all(&theirs_store).expect("mkdir");
    std::fs::copy(
        mine.path().join("oslo/secrets/token.sealed"),
        theirs_store.join("token.sealed"),
    )
    .expect("copy the store");
    assert_eq!(
        secret(theirs.path(), &["get", "token"], b"").0,
        "shared-value",
        "the second recipient could not open it"
    );

    // And the list is what it says: mine first, because adding the first one writes down the
    // recipient that was until then implied.
    let (listed, _, _) = secret(mine.path(), &["recipient"], b"");
    assert_eq!(listed.lines().count(), 2, "{listed}");
    assert!(listed.contains(&published), "{listed}");
}

/// **A recipient is not a key.** Pasting the published half where the secret belongs must be
/// refused, or a store would be readable by anybody holding the half meant to be published.
#[test]
#[cfg(feature = "crypt")]
fn a_published_recipient_is_refused_as_a_key() {
    let home = tempfile::tempdir().expect("tempdir");
    secret(
        home.path(),
        &["set", "anything"],
        b"so the profile key exists",
    );
    let (out, _, _) = secret(home.path(), &["recipient"], b"");
    let published = out
        .split_whitespace()
        .find(|word| word.starts_with("OSLO-PUB-1:"))
        .expect("a public half")
        .to_string();

    let wrong = home.path().join("wrong-key");
    std::fs::write(&wrong, format!("{published}\n")).expect("write");
    secret(
        home.path(),
        &[
            "--store",
            "w",
            "key",
            "add",
            "file",
            &wrong.to_string_lossy(),
        ],
        b"",
    );
    let (_, err, status) = secret(home.path(), &["--store", "w", "set", "x"], b"v");
    assert_ne!(status, 0, "a recipient was accepted as a key");
    assert!(err.contains("OSLO-KEY-1"), "{err:?}");
}
