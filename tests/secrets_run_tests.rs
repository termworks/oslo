//! `oslo secret run` through the real binary: an env file, every secret at once, and a value handed
//! over as a sealed in-memory file.
#![cfg(all(feature = "secrets", feature = "crypt"))]

mod common;

use common::oslo_bin;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Run `oslo secret …` against a home of its own, with `input` on standard input.
fn secret(home: &Path, args: &[&str], input: &[u8]) -> (String, String, i32) {
    let mut child = Command::new(oslo_bin())
        .arg("secret")
        .args(args)
        .current_dir(home)
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

fn keep(home: &Path, args: &[&str], value: &[u8]) {
    let (_, err, status) = secret(home, args, value);
    assert_eq!(status, 0, "{args:?}: {err}");
}

/// **A committed `.env` that names secrets instead of holding them**, in this store or another,
/// beside plain values — with the quoting and comments a dotenv file has.
#[test]
fn an_env_file_mixes_plain_values_and_secrets() {
    let home = tempfile::tempdir().expect("tempdir");
    keep(home.path(), &["set", "gh-token"], b"from-user");
    keep(
        home.path(),
        &["--store", "work", "set", "deploy"],
        b"from-work",
    );
    std::fs::write(
        home.path().join(".env"),
        "# committed\n\
         REGION=eu-west-1\n\
         export GH=secret:gh-token\n\
         DEPLOY=\"secret:work/deploy\"   # another store\n\
         GREETING='hello $world'\n",
    )
    .expect("write");
    let (out, err, status) = secret(
        home.path(),
        &[
            "run",
            "--env-file",
            ".env",
            "--",
            "sh",
            "-c",
            "printf '%s|%s|%s|%s\\n' \"$REGION\" \"$GH\" \"$DEPLOY\" \"$GREETING\"",
        ],
        b"",
    );
    assert_eq!(status, 0, "{err}");
    assert_eq!(out, "eu-west-1|from-user|from-work|hello $world\n");
}

/// **Every secret is opened before anything runs**, so one that fails leaves no child behind —
/// not a command started with half its environment.
#[test]
fn one_secret_that_fails_starts_no_child() {
    let home = tempfile::tempdir().expect("tempdir");
    keep(home.path(), &["set", "present"], b"here");
    std::fs::write(
        home.path().join(".env"),
        "A=secret:present\nB=secret:absent\n",
    )
    .expect("write");
    let marker = home.path().join("it-ran");
    let touch = format!("touch {}", marker.display());
    for args in [
        vec!["run", "--env-file", ".env", "--", "sh", "-c", &touch],
        vec!["run", "A=present", "B=absent", "--", "sh", "-c", &touch],
    ] {
        let (_, err, status) = secret(home.path(), &args, b"");
        assert_ne!(status, 0, "{args:?}");
        assert!(err.contains("absent"), "says which: {err}");
        assert!(!marker.exists(), "a child ran: {args:?}");
    }

    std::fs::write(home.path().join("bad.env"), "A=1\nnot a line\n").expect("write");
    let (_, err, status) = secret(
        home.path(),
        &["run", "--env-file", "bad.env", "--", "sh", "-c", &touch],
        b"",
    );
    assert_ne!(status, 0);
    assert!(err.contains("bad.env:2:"), "names the line: {err}");
    assert!(!marker.exists());
}

/// **`--all` makes every secret a variable**, and the more explicit sources win: a pair over the
/// file, the file over `--all`.
#[test]
fn all_with_a_prefix_and_the_order_of_precedence() {
    let home = tempfile::tempdir().expect("tempdir");
    keep(home.path(), &["set", "gh-token"], b"from-all");
    keep(home.path(), &["set", "db-url"], b"from-all");
    keep(home.path(), &["set", "api-key"], b"from-all");
    keep(home.path(), &["set", "other"], b"from-pair");
    std::fs::write(home.path().join(".env"), "APP_DB_URL=from-file\n").expect("write");
    let (out, err, status) = secret(
        home.path(),
        &[
            "run",
            "--all",
            "--prefix",
            "APP_",
            "--env-file",
            ".env",
            "APP_API_KEY=other",
            "--",
            "sh",
            "-c",
            "echo \"$APP_GH_TOKEN $APP_DB_URL $APP_API_KEY $APP_OTHER\"",
        ],
        b"",
    );
    assert_eq!(status, 0, "{err}");
    assert_eq!(out, "from-all from-file from-pair from-pair\n");
}

/// **`--file` hands over a sealed memfd**: the child reads the bytes, binary ones included, through
/// `/dev/fd/N`, and the descriptor names no file on any disk and refuses a write.
#[test]
fn file_hands_over_a_sealed_in_memory_file() {
    let home = tempfile::tempdir().expect("tempdir");
    let value: &[u8] = b"line one\n\0\xff\xfebinary";
    keep(home.path(), &["set", "blob"], value);
    let copy = home.path().join("copy");
    let script = format!(
        "cat \"$CFG\" > {copy}; n=${{CFG#/dev/fd/}}; readlink /proc/$$/fd/$n; echo \"$CFG\"; \
         printf more >> \"$CFG\" 2>/dev/null && echo WROTE; exit 0",
        copy = copy.display()
    );
    let (out, err, status) = secret(
        home.path(),
        &["run", "--file", "CFG=blob", "--", "sh", "-c", &script],
        b"",
    );
    assert_eq!(status, 0, "{err}");
    assert_eq!(std::fs::read(&copy).expect("copied"), value);
    let mut lines = out.lines();
    let link = lines.next().unwrap_or_default();
    assert!(link.starts_with("/memfd:oslo-secret"), "{out:?}");
    assert!(link.ends_with("(deleted)"), "on no filesystem: {out:?}");
    assert!(
        lines.next().unwrap_or_default().starts_with("/dev/fd/"),
        "{out:?}"
    );
    assert!(!out.contains("WROTE"), "the file is sealed: {out:?}");
}

/// A value that is not text is refused as a variable, and the refusal says `--file` takes it.
#[test]
fn a_binary_value_points_at_file() {
    let home = tempfile::tempdir().expect("tempdir");
    keep(home.path(), &["set", "blob"], b"\xff\xfe");
    let (_, err, status) = secret(
        home.path(),
        &["run", "B=blob", "--", "sh", "-c", "exit 0"],
        b"",
    );
    assert_ne!(status, 0);
    assert!(err.contains("--file B=blob"), "{err}");
}

/// **What follows `--` is the command's**, so a `--store` in it is handed on rather than taken.
#[test]
fn a_store_flag_after_the_dashes_belongs_to_the_command() {
    let home = tempfile::tempdir().expect("tempdir");
    keep(home.path(), &["set", "gh-token"], b"tok");
    let (out, err, status) = secret(
        home.path(),
        &[
            "run",
            "T=gh-token",
            "--",
            "sh",
            "-c",
            "echo \"$T $1 $2\"",
            "sh",
            "--store",
            "elsewhere",
        ],
        b"",
    );
    assert_eq!(status, 0, "{err}");
    assert_eq!(out, "tok --store elsewhere\n");
}
