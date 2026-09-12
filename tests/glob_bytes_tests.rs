//! A filename that is not UTF-8 survives a glob and comes out as the same bytes.
//!
//! `bad\xffname` used to expand to `bad\u{FFFD}name` — a path that does not exist — so
//! `rm b*` removed nothing and `[ -e "$f" ]` said no. GNU bash hands the raw byte through.

mod common;

use common::run_in;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

const NAME: &[u8] = b"bad\xffname";

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join(OsStr::from_bytes(NAME)), "x").expect("file");
    dir
}

#[test]
fn a_glob_match_names_the_file_that_is_there() {
    let dir = tree();
    let r = run_in(
        dir.path(),
        "for f in b*; do [ -e \"$f\" ] && echo exists || echo MISSING; done",
    );
    assert_eq!(r.out(), "exists", "{}", r.stderr);
}

#[test]
fn printf_writes_the_bytes_the_name_has() {
    let dir = tree();
    let r = run_in(dir.path(), "printf '%s\\n' b* > listing");
    assert_eq!(r.status, 0, "{}", r.stderr);
    let written = std::fs::read(dir.path().join("listing")).expect("listing");
    assert_eq!(written, [NAME, b"\n"].concat());
}

#[test]
fn a_redirection_to_the_match_writes_into_that_file() {
    let dir = tree();
    let r = run_in(dir.path(), "echo new > b*");
    assert_eq!(r.status, 0, "{}", r.stderr);
    let content = std::fs::read(dir.path().join(OsStr::from_bytes(NAME))).expect("file");
    assert_eq!(content, b"new\n");
}

#[test]
fn rm_removes_the_file_the_glob_found() {
    let dir = tree();
    let r = run_in(dir.path(), "rm b* && echo gone");
    assert_eq!(r.out(), "gone", "{}", r.stderr);
    assert!(!dir.path().join(OsStr::from_bytes(NAME)).exists());
}

#[test]
fn an_external_command_gets_the_real_bytes() {
    let dir = tree();
    let r = run_in(dir.path(), "/bin/ls b* > listing");
    assert_eq!(r.status, 0, "{}", r.stderr);
    let written = std::fs::read(dir.path().join("listing")).expect("listing");
    assert_eq!(written, [NAME, b"\n"].concat());
}
