//! `glob`: pathname expansion as a command, with qualifiers and regex, for scripts.

mod common;

use common::{Run, run_in};

/// `a.log` (100 B), `b.log` (5 KiB), `c.txt`, and `sub/deep.log`.
fn sh(line: &str) -> Run {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.log"), vec![b'x'; 100]).unwrap();
    std::fs::write(dir.path().join("b.log"), vec![b'x'; 5 * 1024]).unwrap();
    std::fs::write(dir.path().join("c.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/deep.log"), "").unwrap();
    run_in(dir.path(), line)
}

fn out(line: &str) -> String {
    let r = sh(line);
    assert_ne!(r.status, 127, "`{line}`: {}", r.stderr);
    r.out().to_string()
}

#[test]
fn a_pattern_lists_its_matches_one_per_line() {
    assert_eq!(out("glob '*.log'"), "a.log\nb.log");
    assert_eq!(out("glob '**/*.log'"), "a.log\nb.log\nsub/deep.log");
    assert_eq!(out("glob --no-globstar '**/*.log'"), "sub/deep.log");
}

#[test]
fn qualifiers_are_options() {
    assert_eq!(out("glob '**/*.log' --larger 1k"), "b.log");
    assert_eq!(out("glob '*' --dir"), "sub");
    assert_eq!(out("glob '*' --re '^[ab]\\.log$'"), "a.log\nb.log");
    assert_eq!(out("glob '**/*' --path-re 'sub/'"), "sub/deep.log");
    assert_eq!(out("glob '*.log' --largest 1"), "b.log");
}

#[test]
fn count_nul_and_rows() {
    assert_eq!(out("glob --count '*.log'"), "2");
    assert_eq!(out("glob -0 '*.log' | tr '\\0' ,"), "a.log,b.log,");
    let rows = out("glob --rows 'c.txt'");
    let mut lines = rows.lines();
    assert_eq!(
        lines.next(),
        Some("path\tname\text\tkind\tsize\tmodified\tmode\tdepth")
    );
    assert!(
        lines
            .next()
            .is_some_and(|row| row.starts_with("c.txt\tc.txt\ttxt\tfile\t0\t"))
    );
}

#[test]
fn match_tests_a_name_without_the_filesystem() {
    assert_eq!(out("glob --match 'a*.txt' abc.txt && echo y"), "y");
    assert_eq!(out("glob --match 'a*.txt' abc.log; echo $?"), "1");
}

/// No match is empty output and 1 — never the pattern text.
#[test]
fn nothing_matching_is_status_one_and_no_output() {
    assert_eq!(out("glob 'zz*'; echo rc=$?"), "rc=1");
}

#[test]
fn a_bad_qualifier_is_named() {
    let r = sh("glob '*' --oldr 1");
    assert_eq!(r.status, 2);
    assert!(
        r.err().contains("`oldr` is not a qualifier"),
        "{}",
        r.stderr
    );
}
