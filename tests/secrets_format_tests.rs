//! The sealed format through the real binary: the attacks the audit ran, each refused now, and the
//! older format still read until a store is rotated.
#![cfg(feature = "crypt")]

mod common;

use common::oslo_bin;
use oslo::secrets::{native, sync};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Run `oslo secret …` against a home of its own, with `input` on standard input.
fn secret(home: &Path, args: &[&str], input: &[u8]) -> (String, String, i32) {
    let mut child = Command::new(oslo_bin())
        .arg("secret")
        .args(args)
        .env("XDG_DATA_HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("OSLO_SECRET_IDENTITY")
        .env_remove("OSLO_SECRET_CONF")
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

fn sealed(home: &Path, name: &str) -> PathBuf {
    home.join(format!("oslo/secrets/{name}.sealed"))
}

/// The public half this home's `user` store publishes, once it has a key.
fn published(home: &Path) -> String {
    let (out, err, status) = secret(home, &["recipient"], b"");
    assert_eq!(status, 0, "{err}");
    out.split_whitespace()
        .find(|word| word.starts_with("OSLO-PUB-1:"))
        .expect("a public half")
        .to_string()
}

/// `name`'s file as it is on disk: its stamp, and the sealed body.
fn kept(home: &Path, name: &str) -> sync::Kept {
    sync::unwrap(&std::fs::read(sealed(home, name)).expect("read")).expect("a header")
}

/// Put `body` in place of the sealed part of `name`'s file, keeping its stamp.
fn replace_body(home: &Path, name: &str, body: Vec<u8>) {
    let path = sealed(home, name);
    let stamp = kept(home, name).stamp;
    std::fs::write(
        &path,
        sync::wrap(&sync::Kept {
            stamp,
            sealed: body,
        }),
    )
    .expect("write");
}

/// **h3, forge**: somebody holding only the published half wrote a file that opened. Now the file
/// names its writer, and a writer that is not on the list is refused.
#[test]
fn a_file_forged_from_a_public_half_is_refused() {
    let home = tempfile::tempdir().expect("tempdir");
    secret(home.path(), &["set", "token"], b"genuine");
    let victim = native::read_public(&published(home.path())).expect("a recipient");

    let attacker = native::generate_secret().expect("a key");
    let here = native::Context::Named {
        store: "user",
        name: "token",
    };
    let forged = native::seal(&attacker, &[victim], &here, b"attacker-chosen").expect("seal");
    replace_body(home.path(), "token", forged);

    let (out, err, status) = secret(home.path(), &["get", "token"], b"");
    assert_ne!(status, 0, "a forged file opened: {out}");
    assert!(!out.contains("attacker-chosen"));
    assert!(err.contains("not one of the recipients"), "{err}");
    let theirs = native::write_public(&native::public_of(&attacker));
    assert!(err.contains(&theirs), "the writer is not named: {err}");
}

/// **h3, swap**: `cp prod-token.sealed test-token.sealed` made `get test-token` answer the prod
/// value. A file is now bound to its name and its store.
#[test]
fn a_file_swapped_for_another_does_not_open() {
    let home = tempfile::tempdir().expect("tempdir");
    secret(home.path(), &["set", "prod-token"], b"the-prod-value");
    secret(home.path(), &["set", "test-token"], b"the-test-value");
    std::fs::copy(
        sealed(home.path(), "prod-token"),
        sealed(home.path(), "test-token"),
    )
    .expect("copy");

    let (out, err, status) = secret(home.path(), &["get", "test-token"], b"");
    assert_ne!(status, 0, "the swapped file opened: {out}");
    assert!(!out.contains("the-prod-value"));
    assert!(err.contains("under another name or store"), "{err}");

    // And into another store with the same key behind it.
    let key = home.path().join("shared-key");
    for store in ["one", "two"] {
        let path = key.to_string_lossy();
        let args = ["--store", store, "key", "add", "file", &path];
        assert_eq!(secret(home.path(), &args, b"").2, 0);
    }
    secret(
        home.path(),
        &["--store", "one", "set", "deploy"],
        b"from-one",
    );
    let into = home.path().join("oslo/stores/two");
    std::fs::create_dir_all(&into).expect("mkdir");
    std::fs::copy(
        home.path().join("oslo/stores/one/deploy.sealed"),
        into.join("deploy.sealed"),
    )
    .expect("copy");
    let (out, _, status) = secret(home.path(), &["--store", "two", "get", "deploy"], b"");
    assert_ne!(status, 0, "a file moved between stores opened: {out}");
}

/// **h3, strip**: dropping a recipient's stanza and the count with it left a file that opened.
#[test]
fn a_stripped_recipient_does_not_open() {
    let home = tempfile::tempdir().expect("tempdir");
    secret(home.path(), &["set", "token"], b"value");
    let other = native::write_public(&native::public_of(&native::generate_secret().expect("key")));
    assert_eq!(secret(home.path(), &["recipient", "add", &other], b"").2, 0);
    assert_eq!(secret(home.path(), &["rotate"], b"").2, 0);

    let body = kept(home.path(), "token").sealed;
    let (count, stanza) = (4 + 1 + 32, 32 + 48);
    assert_eq!(body[count], 2, "two recipients");
    let mut stripped = body[..count].to_vec();
    stripped.push(1);
    stripped.extend_from_slice(&body[count + 1..count + 1 + stanza]);
    stripped.extend_from_slice(&body[count + 1 + 2 * stanza..]);
    replace_body(home.path(), "token", stripped);

    let (out, err, status) = secret(home.path(), &["get", "token"], b"");
    assert_ne!(status, 0, "a stripped file opened: {out}");
    assert!(err.contains("header was changed"), "{err}");
}

/// **Each way a file fails to open says which it is**, and none of them prints the value.
#[test]
fn a_failure_to_open_says_which_failure_it_is() {
    let home = tempfile::tempdir().expect("tempdir");
    secret(home.path(), &["set", "token"], b"the-real-value");
    let body = kept(home.path(), "token").sealed;

    let mut bent = body.clone();
    *bent.last_mut().expect("a byte") ^= 1;
    let cases = [
        (bent, "changed or damaged"),
        (body[..body.len() - 20].to_vec(), "stops inside"),
        (b"not a sealed file at all".to_vec(), "no OSLO header"),
    ];
    for (file, said) in cases {
        replace_body(home.path(), "token", file);
        let (out, err, status) = secret(home.path(), &["get", "token"], b"");
        assert_ne!(status, 0, "{said}: it opened");
        assert!(out.is_empty() && !err.contains("the-real-value"), "{err}");
        assert!(err.contains(said), "wanted {said:?}: {err}");
    }

    // A file sealed for another key altogether.
    let stranger = native::generate_secret().expect("a key");
    let theirs = native::public_of(&stranger);
    let here = native::Context::Named {
        store: "user",
        name: "token",
    };
    replace_body(
        home.path(),
        "token",
        native::seal(&stranger, &[theirs], &here, b"x").expect("seal"),
    );
    let (_, err, _) = secret(home.path(), &["get", "token"], b"");
    assert!(err.contains("no key opened it, of the 1"), "{err}");
}

/// **h2**: `recipient add OSLO-PUB-1:AAAA…=` returned 0, and every later write was readable by
/// anybody. A low-order point is refused on the command line, from a file, and in `secrets.conf`.
#[test]
fn a_low_order_recipient_is_refused_wherever_it_comes_in() {
    let low = "OSLO-PUB-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let home = tempfile::tempdir().expect("tempdir");
    secret(home.path(), &["set", "before"], b"v");

    let (_, err, status) = secret(home.path(), &["recipient", "add", low], b"");
    assert_ne!(status, 0, "a low-order recipient was added");
    assert!(err.contains("low-order"), "{err}");

    let list = home.path().join("RECIPIENTS");
    std::fs::write(&list, format!("# from a colleague\n{low}\n")).expect("write");
    let (_, err, status) = secret(
        home.path(),
        &["recipient", "add", "--from", &list.to_string_lossy()],
        b"",
    );
    assert_ne!(status, 0, "a low-order recipient was added from a file");
    assert!(err.contains("low-order"), "{err}");
    let conf = home.path().join("state/oslo/secrets.conf");
    assert!(
        !std::fs::read_to_string(&conf)
            .unwrap_or_default()
            .contains(low)
    );

    let mine = published(home.path());
    std::fs::write(
        &conf,
        format!("[user]\nrecipient {mine}\nrecipient {low}\n"),
    )
    .expect("write");
    let (_, err, status) = secret(home.path(), &["set", "after"], b"v");
    assert_ne!(status, 0, "a write was sealed to a low-order point");
    assert!(err.contains("low-order"), "{err}");
    assert!(!sealed(home.path(), "after").exists());
    let (out, _, _) = secret(home.path(), &["recipient"], b"");
    assert!(out.contains("UNUSABLE"), "{out}");
}

/// **No error repeats a private key**, wherever it was pasted, and the grammar of the empty case.
#[test]
fn a_private_key_is_never_repeated_back() {
    let home = tempfile::tempdir().expect("tempdir");
    let key = "OSLO-KEY-1:YSBkZW1vIGtleSwgdGhpcnR5LXR3byBieXRlcyBsb24=";
    let profile = "OSLO-PROFILE-1:0b526c7519703067fdef7d83ba0439b86400313dd155a23c10c6329e19bde2f7";
    for text in [key, profile] {
        let body = &text[text.find(':').expect("a prefix") + 1..];
        for args in [
            vec!["recipient", "add", text],
            vec!["recipient", text],
            vec!["key", "add", text],
            vec!["key", text],
            vec!["cipher", text],
            vec![text],
            vec!["get", text],
            vec!["get", "name", text],
            vec!["--store", text, "list"],
            vec!["run", text, "--", "true"],
        ] {
            let (out, err, status) = secret(home.path(), &args, b"");
            assert_ne!(status, 0, "{args:?} succeeded");
            let said = format!("{out}{err}").to_lowercase();
            assert!(
                !said.contains(&body.to_lowercase()),
                "{args:?} echoed it: {err}"
            );
        }
    }
    let (_, err, _) = secret(home.path(), &["recipient", "add", key], b"");
    assert!(
        err.contains("OSLO-KEY-1:…") && err.contains("private"),
        "{err}"
    );

    let empty = home.path().join("empty-key");
    std::fs::write(&empty, "# nothing yet\n").expect("write");
    let path = empty.to_string_lossy();
    secret(
        home.path(),
        &["--store", "w", "key", "add", "file", &path],
        b"",
    );
    let (_, err, _) = secret(home.path(), &["--store", "w", "set", "x"], b"v");
    assert!(err.contains("there is no key in it"), "{err}");
}

/// The profile key and the `OSLO2` file `oslo 0.8.1` wrote for `legacy = from-oslo-0.8.1`.
const OLD_KEY: &str =
    "OSLO-PROFILE-1:0b526c7519703067fdef7d83ba0439b86400313dd155a23c10c6329e19bde2f7\n";
const OLD_FILE: &str = "\
4f534c4f534543312031206c697665206166623631343939613635343738616432613465613939326532303263613865\
0a4f534c4f32019ac26d7c3a834b837651e9d42776f8e10af333b91e0e074a96b68e3d36f683103a02ed0e43649232a0\
1763ac54c24c07dcce12617bb9eb285727ee4fe291975dac441a3e975c4ed26be26f89dbbadcc01d29974b374f4c8bba\
35d74513c69aa184797c2ee15072df036b9edd9ce30cb5d2f8d983f81181a7d4e407e1e5131f8fa0110d2eca303a";

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|at| u8::from_str_radix(&text[at * 2..at * 2 + 2], 16).expect("hex"))
        .collect()
}

/// **`OSLO2` is read, with one warning, until the store is rotated.** `rotate` rewrites it as
/// `OSLO3` and writes `refuse OSLO2`, so an `OSLO2` file planted afterwards is refused.
#[test]
fn an_oslo2_file_is_read_until_the_store_is_rotated() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().expect("tempdir");
    let profiles = home.path().join("state/oslo/profiles");
    std::fs::create_dir_all(&profiles).expect("mkdir");
    std::fs::write(profiles.join("default.key"), OLD_KEY).expect("write");
    std::fs::set_permissions(
        profiles.join("default.key"),
        PermissionsExt::from_mode(0o600),
    )
    .expect("chmod");
    std::fs::create_dir_all(home.path().join("oslo/secrets")).expect("mkdir");
    std::fs::write(sealed(home.path(), "legacy"), unhex(OLD_FILE)).expect("write");

    let (out, err, status) = secret(home.path(), &["get", "legacy"], b"");
    assert_eq!((out.as_str(), status), ("from-oslo-0.8.1", 0), "{err}");
    assert!(
        err.contains("OSLO2") && err.contains("rotate"),
        "no warning: {err:?}"
    );
    assert_eq!(err.lines().count(), 1, "more than one line: {err}");

    // A forged OSLO2 file cannot be told from a real one, so rotate names every one it rewrote.
    let (out, err, status) = secret(home.path(), &["rotate"], b"");
    assert_eq!(status, 0, "{err}");
    assert_eq!(out, "1 re-encrypted\n");
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(err.contains("planted: legacy"), "{err}");
    let rewritten = kept(home.path(), "legacy");
    assert!(
        rewritten.sealed.starts_with(b"OSLO3"),
        "rotate left it as it was"
    );
    let conf = std::fs::read_to_string(home.path().join("state/oslo/secrets.conf")).expect("conf");
    assert!(conf.contains("refuse OSLO2"), "{conf}");

    let (out, err, status) = secret(home.path(), &["get", "legacy"], b"");
    assert_eq!((out.as_str(), status), ("from-oslo-0.8.1", 0), "{err}");
    assert!(err.is_empty(), "{err}");

    std::fs::write(sealed(home.path(), "legacy"), unhex(OLD_FILE)).expect("write");
    let (out, err, status) = secret(home.path(), &["get", "legacy"], b"");
    assert_ne!(status, 0, "an OSLO2 file opened after rotate: {out}");
    assert!(err.contains("OSLO2") && err.contains("refuse"), "{err}");
}
