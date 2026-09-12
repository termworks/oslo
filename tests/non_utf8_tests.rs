//! Bytes the shell cannot represent, in the places it cannot refuse them.
//!
//! `std::env::vars()` and `std::env::args()` both panic on a byte that is not UTF-8, and both run
//! before the shell has done anything — so one Latin-1 byte in the environment, in an argument, or
//! in the name of the directory the shell started in killed the process with `SIGABRT` and no
//! message anybody could act on. bash passes the same bytes through and runs.
//!
//! Spawned as a process rather than tested in-crate: the environment and the working directory are
//! process-wide, and a test that set either would be racing every other test in its binary.

mod common;

use common::oslo_bin;
use std::os::unix::ffi::OsStrExt;
use std::process::Command;

/// A value the shell cannot hold: valid as bytes, not as UTF-8.
fn latin1(bytes: &[u8]) -> &std::ffi::OsStr {
    std::ffi::OsStr::from_bytes(bytes)
}

/// **The variable is skipped, not mangled**, and the shell runs.
///
/// Skipped rather than replaced because the shell cannot hold the bytes and a child expects the
/// real ones — which it still gets, through the environment `execve` inherits.
#[test]
fn a_variable_that_is_not_utf8_does_not_stop_the_shell() {
    let out = Command::new(oslo_bin())
        .env("BAD", latin1(b"pre\xffpost"))
        .args(["-c", "echo it-ran; echo \"[${BAD-unset}]\""])
        .output()
        .expect("spawn");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("it-ran"),
        "the shell ran: {text:?} {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("[unset]"),
        "and the value it cannot hold is absent: {text:?}"
    );
    assert_eq!(out.status.code(), Some(0), "not an abort");
}

/// **A positional keeps its place**, so the byte is replaced rather than the argument dropped:
/// `$2` has to stay `$2`.
#[test]
fn an_argument_that_is_not_utf8_keeps_its_position() {
    let out = Command::new(oslo_bin())
        .arg("-c")
        .arg("echo \"0=[$0] 1=[$1] 2=[$2]\"")
        .arg("name")
        .arg(latin1(b"arg\xffbad"))
        .arg("third")
        .output()
        .expect("spawn");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("0=[name]"), "{text:?}");
    assert!(
        text.contains("2=[third]"),
        "the one after it is still third: {text:?}"
    );
    assert_eq!(out.status.code(), Some(0), "not an abort");
}

/// The commonest way to meet one of these bytes: a directory named in another locale.
#[test]
fn a_working_directory_that_is_not_utf8_does_not_stop_the_shell() {
    let dir = tempfile::tempdir().expect("tempdir");
    let here = dir.path().join(latin1(b"caf\xe9"));
    std::fs::create_dir(&here).expect("mkdir");
    let out = Command::new(oslo_bin())
        .current_dir(&here)
        .args(["-c", "echo ran-here"])
        .output()
        .expect("spawn");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("ran-here"),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0), "not an abort");
}
