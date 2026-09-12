//! `oslo.glob`, `oslo.fs.glob`, `oslo.fs.match` and `oslo.shopt`, run as a real Lua script.
//!
//! `oslo.glob("c.txt")` answered `{}` for a file that was there, and `**` in Lua meant `*` unless
//! the shell had happened to run `shopt -s globstar` first.

mod common;

use common::{oslo_bin, run_in};

/// `a.log` (10 B), `b.log` (5000 B), `c.txt` (10 B) and `sub/deep.log`; `script` is run by oslo.
fn lua(script: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, size) in [("a.log", 10), ("b.log", 5000), ("c.txt", 10)] {
        std::fs::write(dir.path().join(name), vec![b'x'; size]).unwrap();
    }
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/deep.log"), "").unwrap();
    std::fs::write(dir.path().join("probe.lua"), script).unwrap();
    let r = run_in(
        dir.path(),
        &format!("{} --norc probe.lua", oslo_bin().display()),
    );
    assert_eq!(r.status, 0, "{script}\n{}", r.stderr);
    r.out().to_string()
}

#[test]
fn a_pattern_lists_its_matches_and_a_plain_name_finds_its_file() {
    assert_eq!(
        lua(r#"print(table.concat(oslo.glob("*.log"), " "))"#),
        "a.log b.log"
    );
    assert_eq!(
        lua(r#"print(#oslo.glob("c.txt"), #oslo.glob("nope.txt"))"#),
        "1\t0"
    );
}

#[test]
fn a_globstar_crosses_directories_whatever_the_shell_has_set() {
    assert_eq!(
        lua(r#"print(table.concat(oslo.glob("**/*.log"), " "))"#),
        "a.log b.log sub/deep.log"
    );
    assert_eq!(
        lua(r#"print(table.concat(oslo.glob("**/*.log", { globstar = false }), " "))"#),
        "sub/deep.log"
    );
}

#[test]
fn qualifiers_are_options_and_rows_are_tables() {
    assert_eq!(
        lua(r#"print(table.concat(oslo.glob("*.log", { larger = "1k" }), " "))"#),
        "b.log"
    );
    assert_eq!(lua(r#"print(#oslo.fs.glob("*", { dir = true }))"#), "1");
    assert_eq!(
        lua(r#"local r = oslo.glob("c.txt", { rows = true })[1]; print(r.name, r.kind, r.size)"#),
        "c.txt\tfile\t10"
    );
    assert!(lua(r#"print(pcall(oslo.glob, "*", { oldr = 1 }))"#).starts_with("false"));
}

#[test]
fn match_asks_nothing_of_the_filesystem() {
    assert_eq!(
        lua(r#"print(oslo.fs.match("abc.txt", "a*.txt"), oslo.fs.match("abc.log", "a*.txt"))"#),
        "true\tfalse"
    );
}

#[test]
fn shopt_reads_and_sets_the_shells_options() {
    assert_eq!(
        lua(
            r#"print(oslo.shopt("nullglob")); oslo.shopt("nullglob", true); print(oslo.shopt("nullglob"))"#
        ),
        "false\ntrue"
    );
    // `nil, message`, the convention every fallible `oslo.*` call answers with.
    let answer = lua(r#"print(oslo.shopt("nosuch"))"#);
    assert!(answer.starts_with("nil\t"), "{answer}");
    assert!(answer.contains("invalid shell option name"), "{answer}");
}
