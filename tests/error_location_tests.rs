//! Where a diagnostic says it came from.
//!
//! A script that fails should name the file and the line, because that is the one thing the person
//! fixing it needs and the one thing every other shell already tells them. oslo prefixed every
//! diagnostic with `oslo: ` instead, so a failure two hundred lines into a script named the shell
//! and the command and nothing about *where*.
//!
//! The rule is bash's: a file gets `name: line N: `, and a prompt or a `-c` gets `oslo: ` — those
//! have no line worth naming, since every typed command is line one.

mod common;

use common::oslo_bin;
use std::process::{Command, Stdio};

/// Run `script` as a file and answer what reached stderr, with the temporary path shortened to the
/// bare filename so a case can assert on it.
#[track_caller]
fn in_a_file(script: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("case.sh");
    std::fs::write(&path, script).expect("write script");
    let out = Command::new(oslo_bin())
        .arg(&path)
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env_remove("ENV")
        .stdin(Stdio::null())
        .output()
        .expect("spawn oslo");
    String::from_utf8_lossy(&out.stderr)
        .replace(&path.to_string_lossy().to_string(), "case.sh")
        .trim_end()
        .to_string()
}

/// The same source through `-c`, which has no file.
#[track_caller]
fn with_dash_c(script: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = Command::new(oslo_bin())
        .arg("-c")
        .arg(script)
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env_remove("ENV")
        .stdin(Stdio::null())
        .output()
        .expect("spawn oslo");
    String::from_utf8_lossy(&out.stderr).trim_end().to_string()
}

/// **The line is the line the command is on**, not the line the script ends on.
#[test]
fn a_command_that_is_not_there_names_the_file_and_the_line() {
    let seen = in_a_file("echo one\n\n\nnosuchcommand_xyz\necho two\n");
    assert_eq!(
        seen,
        "case.sh: line 4: nosuchcommand_xyz: command not found"
    );
}

#[test]
fn a_file_that_cannot_be_run_names_where_it_was_run_from() {
    let seen = in_a_file("echo one\n/etc/passwd\n");
    assert!(seen.starts_with("case.sh: line 2: /etc/passwd: "), "{seen}");
}

/// `cd` is a builtin and had its own diagnostic; it has to carry the location too, or the rule is
/// one a reader cannot rely on.
#[test]
fn a_builtin_that_fails_names_the_file_and_the_line() {
    let seen = in_a_file("echo one\necho two\ncd /nonexistent-dir-xyz\n");
    assert!(seen.starts_with("case.sh: line 3: cd: "), "{seen}");
}

/// A redirection that cannot be opened is the command's failure, and is reported the same way.
#[test]
fn a_redirection_that_fails_names_the_file_and_the_line() {
    let seen = in_a_file("echo one\ncat < /nonexistent-zz\n");
    assert!(seen.starts_with("case.sh: line 2: "), "{seen}");
    assert!(seen.contains("/nonexistent-zz"), "{seen}");
}

/// **Every line, not just the first.** The number comes from `$LINENO`, which is republished at
/// each command boundary — a version that reported the same line twice would look right on any
/// one-failure test.
#[test]
fn each_failure_names_its_own_line() {
    let seen = in_a_file("nosuch_a\necho middle\nnosuch_b\n\nnosuch_c\n");
    let lines: Vec<&str> = seen.lines().collect();
    assert_eq!(lines.len(), 3, "{seen}");
    assert!(
        lines[0].starts_with("case.sh: line 1: nosuch_a: "),
        "{seen}"
    );
    assert!(
        lines[1].starts_with("case.sh: line 3: nosuch_b: "),
        "{seen}"
    );
    assert!(
        lines[2].starts_with("case.sh: line 5: nosuch_c: "),
        "{seen}"
    );
}

/// A line inside a loop is the line of the command, however many times it runs.
#[test]
fn a_line_inside_a_loop_is_the_commands_own() {
    let seen = in_a_file("for i in 1 2; do\n  nosuchcommand_xyz\ndone\n");
    for line in seen.lines() {
        assert!(line.starts_with("case.sh: line 2: "), "{seen}");
    }
    assert_eq!(seen.lines().count(), 2, "{seen}");
}

/// **`-c` keeps `oslo: `**, because there is no file to name — and that is what bash does too.
#[test]
fn dash_c_has_no_file_and_says_so_the_old_way() {
    let seen = with_dash_c("nosuchcommand_xyz");
    assert_eq!(seen, "oslo: nosuchcommand_xyz: command not found");
}

#[test]
fn dash_c_keeps_the_old_prefix_for_a_builtin_too() {
    let seen = with_dash_c("cd /nonexistent-dir-xyz");
    assert!(seen.starts_with("oslo: cd: "), "{seen}");
}

/// A sourced file names *itself*, not the script that sourced it, and the line is its own.
///
/// **Not by swapping `$0`**, which stays the outer script — POSIX says a sourced file shares the
/// caller's positional parameters and `$0` is one of them, and bash agrees. The location is tracked
/// separately; both halves were checked against bash 5.3.9.
#[test]
fn a_sourced_file_names_itself() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inner = dir.path().join("inner.sh");
    std::fs::write(&inner, "echo inner\nnosuchcommand_xyz\n").expect("write inner");
    let outer = dir.path().join("outer.sh");
    std::fs::write(&outer, format!("echo outer\n. {}\n", inner.display())).expect("write outer");

    let out = Command::new(oslo_bin())
        .arg(&outer)
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env_remove("ENV")
        .stdin(Stdio::null())
        .output()
        .expect("spawn oslo");
    let seen = String::from_utf8_lossy(&out.stderr)
        .replace(&inner.to_string_lossy().to_string(), "inner.sh")
        .trim_end()
        .to_string();
    assert!(
        seen.starts_with("inner.sh: line 2: nosuchcommand_xyz: "),
        "{seen}"
    );
}

/// **The errors that end the run had been the ones not saying where.**
///
/// A command that is not found, a builtin that fails and a redirection that cannot be opened all
/// named the file and the line. The four fatal expansion errors did not — they are printed on the
/// way out, past the point every other diagnostic goes through — so the failures that actually
/// stop a script were the ones a reader could not locate. Checked against bash, which words three
/// of these identically.
#[test]
fn a_fatal_expansion_error_names_the_file_and_the_line() {
    assert_eq!(
        in_a_file("echo one\nset -u\necho \"$nope\"\n"),
        "case.sh: line 3: nope: unbound variable"
    );
    assert_eq!(
        in_a_file("echo one\necho $((1/0))\n"),
        "case.sh: line 2: division by 0"
    );
    assert_eq!(
        in_a_file("echo one\necho ${x!!}\n"),
        "case.sh: line 2: ${x!!}: bad substitution"
    );
    assert_eq!(
        in_a_file("echo one\nunset v\necho \"${v:?gone}\"\n"),
        "case.sh: line 3: v: gone"
    );
}

/// `-c` keeps `oslo: ` for these too: there is no file to name, and that is bash's answer as well.
#[test]
fn a_fatal_expansion_error_under_dash_c_names_the_shell() {
    assert_eq!(
        with_dash_c("set -u; echo \"$nope\""),
        "oslo: nope: unbound variable"
    );
    assert_eq!(with_dash_c("echo $((1/0))"), "oslo: division by 0");
}

/// **The category is written once, and in lower case.** A parser whose own wording opened
/// `syntax error at …` and an unconditional `Syntax error: ` in front of it produced
/// `Syntax error: syntax error at end of input` — the same words twice, the second time as though
/// a new sentence had started in the middle of the line.
#[test]
fn a_syntax_error_says_so_once() {
    let said = with_dash_c("if");
    assert_eq!(said, "oslo: syntax error: this `if` was never closed");
    assert!(!said.contains("Syntax"), "{said}");

    // And a message that does *not* open with the category still gets one.
    assert!(
        with_dash_c("fi").starts_with("oslo: syntax error: "),
        "{}",
        with_dash_c("fi")
    );
}
/// **A script that does not parse keeps its line numbers.**
///
/// A program with a syntax error anywhere in it is run a command at a time — that is the only way
/// to run the lines before the mistake — and each of those commands is parsed on its own, so its
/// tree counts from 1. Every diagnostic before the mistake therefore said `line 1`, which is worse
/// than saying nothing: a line number is believed.
#[test]
fn a_syntax_error_later_does_not_flatten_the_lines_before_it() {
    let err = in_a_file(
        "echo one\nnosuchcommand_a\necho three\nnosuchcommand_b\nif true; then echo \"unclosed\n",
    );
    assert!(
        err.contains("case.sh: line 2: nosuchcommand_a"),
        "stderr: {err}"
    );
    assert!(
        err.contains("case.sh: line 4: nosuchcommand_b"),
        "stderr: {err}"
    );
    assert!(
        !err.contains("case.sh: line 1:"),
        "nothing is on line 1: {err}"
    );
}

/// The syntax error names the line **it** stopped on, not the last line that ran.
///
/// `origin` reports the last published line, and a parse failure publishes nothing — so a mistake
/// on line 4 was announced as line 3, which is the line above it and reads as a different bug.
#[test]
fn a_syntax_error_names_the_line_it_stopped_on() {
    let err = in_a_file("echo one\necho two\necho three\necho \"unclosed\n");
    assert!(
        err.contains("case.sh: line 4: syntax error"),
        "stderr: {err}"
    );
}

/// `$LINENO` is the file's line in both paths — a script that parses and one that does not.
#[test]
fn lineno_is_the_files_own_line_either_way() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = |name: &str, text: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).expect("write");
        let out = Command::new(oslo_bin())
            .arg(&path)
            .stdin(Stdio::null())
            .output()
            .expect("spawn oslo");
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    };
    assert_eq!(
        out("whole.sh", "echo $LINENO\necho $LINENO\n\necho $LINENO\n"),
        "1\n2\n4"
    );
    assert_eq!(
        out(
            "pieces.sh",
            "echo $LINENO\necho $LINENO\n\necho $LINENO\nif true; then echo \"unclosed\n"
        ),
        "1\n2\n4",
        "the same numbers, though this one is run a command at a time"
    );
}

/// **An error with no position names the line that was left open.**
///
/// An unfinished construct is about the absence of text, so there is no column for it — and
/// `origin` reports the last line that *ran*, which is a different line entirely. The chunk that
/// failed begins at the construct that never closed, because everything before it parsed and ran,
/// so its first line is what somebody has to go and fix. It is the line bash names when it says
/// `syntax error: unexpected end of file from 'if' command on line 2`.
#[test]
fn an_unfinished_construct_names_where_it_started() {
    let err = in_a_file("echo one\nif true; then\necho three\nnosuchcommand_after\n");
    assert!(
        err.contains("case.sh: line 2: syntax error: this `if` was never closed"),
        "line 2 is the `if`, not line 1 where `echo one` ran: {err}"
    );
}

/// The same when the file opens with it: there is no earlier line to fall back to, and the answer
/// is still the construct's own.
#[test]
fn an_unfinished_construct_on_line_one_says_line_one() {
    let err = in_a_file("if true; then\necho x\n");
    assert!(err.contains("case.sh: line 1: syntax error"), "{err}");
}
