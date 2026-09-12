//! End-to-end checks that history expansion happens at the prompt and *only* at the prompt.
//!
//! The expansion algorithm itself is unit-tested next to the code, where every event and word
//! designator is a table row. What no unit test can show is the wiring: that the REPL runs the
//! pass at all, that the rewritten line is what lands in history, and — the part that matters for
//! safety — that `-c` and script input never go near it. A `!` inside data must stay data.

mod common;

use std::io::Write;
use std::process::{Command, Stdio};

/// Feed `lines` to an interactive oslo and return `(stdout, stderr)`.
///
/// `-i` forces the REPL even though stdin is a pipe, which is what makes this scriptable. `HOME`
/// points at a scratch directory so the developer's own `~/.oslo_history` neither leaks into the
/// event numbering nor gets written to.
fn repl(dir: &std::path::Path, lines: &str) -> (String, String) {
    let mut child = Command::new(common::oslo_bin())
        .arg("-i")
        .current_dir(dir)
        .env("HOME", dir)
        // Asked for explicitly: there is no default history file any more, and this test is about
        // what gets written into one.
        .env("HISTFILE", dir.join(".oslo_history"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oslo -i");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(lines.as_bytes())
        .expect("write script");
    let out = child.wait_with_output().expect("wait for oslo");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// **The forms a prompt still expands.** `!` at the start of a shell line is also the prefix that
/// runs one line as Lua, so `startup::mode::classify` keeps only the events no Lua expression can
/// begin with — `!!`, `!$`, `!^`, `!*`, `!?str?` — and sends the rest to Lua. `!name` and the
/// numbered events are covered by [`a_named_event_is_lua_at_the_prompt`] instead.
#[test]
fn the_prompt_expands_history_references() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (stdout, stderr) = repl(
        dir.path(),
        "echo alpha beta gamma\n!!\necho pre !$ post\n^pre^POST\nexit\n",
    );

    let count = |want: &str| stdout.lines().filter(|line| line.ends_with(want)).count();
    assert_eq!(
        count("alpha beta gamma"),
        2,
        "the typed line runs, then `!!` runs it again: {stdout:?}"
    );
    assert_eq!(
        count("pre gamma post"),
        1,
        "`!$` takes the previous line's last word: {stdout:?}"
    );
    assert_eq!(
        count("POST gamma post"),
        1,
        "^pre^POST edits the previous line: {stdout:?}"
    );
    // bash echoes the rewritten line on stderr so the user sees what actually ran.
    assert!(
        stderr.contains("echo alpha beta gamma"),
        "the expansion must be echoed: {stderr:?}"
    );
}

#[test]
fn an_unresolvable_reference_runs_nothing_and_leaves_the_status_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    // `!?nosuch?` rather than `!nosuch`: the bare-word form is Lua now, and this is the
    // containing-text form, which stays history because Lua has no `?`.
    let (stdout, stderr) = repl(dir.path(), "true\n!?nosuch?\necho status=$?\nexit\n");

    assert!(
        stderr.contains("event not found"),
        "the reason must be reported: {stderr:?}"
    );
    assert!(
        stdout.lines().any(|line| line.ends_with("status=0")),
        "a failed expansion runs nothing, so `$?` is untouched: {stdout:?}"
    );
}

/// **A `!` followed by a name is one line of Lua**, not the last command starting with that name.
///
/// The two cannot be told apart — `!print` is a plausible spelling of both — so the split is made
/// on what *can* begin a Lua expression. A bare word can, so it is Lua; `!!` and `!$` cannot, so
/// they stay history. The history finder is what replaces `!name` at the prompt.
#[test]
fn a_named_event_is_lua_at_the_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (stdout, _) = repl(dir.path(), "echo findme\n!print(2 * 21)\nexit\n");

    assert!(
        stdout
            .lines()
            .any(|line| line.split_whitespace().last() == Some("42")),
        "`!print(...)` should have run as Lua: {stdout:?}"
    );
    assert_eq!(
        stdout.lines().filter(|l| l.ends_with("findme")).count(),
        1,
        "nothing should have been re-run from history: {stdout:?}"
    );
}

#[test]
fn history_expansion_never_reaches_a_non_interactive_shell() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The security-relevant half of the feature: `-c` text and script text are data the shell was
    // handed, not something a user typed at a prompt, so a `!` in them is a literal `!`.
    for script in ["echo '!!'", "echo !!", "echo a!$b", "echo ^x^y"] {
        let run = common::run_in(dir.path(), script);
        assert_eq!(
            run.status, 0,
            "{script:?} should just run: {:?}",
            run.stderr
        );
        assert!(
            run.out().contains('!') || run.out().contains('^'),
            "{script:?} must print its `!`/`^` verbatim, got {:?}",
            run.out()
        );
    }
    assert_eq!(common::run_in(dir.path(), "echo !!").out(), "!!");
    assert_eq!(common::run_in(dir.path(), "echo ^x^y").out(), "^x^y");
}

#[test]
fn history_records_the_expanded_line_not_the_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    // `!!` twice in a row must not recall itself: the first one has to be stored as what it
    // became, or the second would resolve to the literal `!!` and be a syntax error.
    let (stdout, _) = repl(dir.path(), "echo kept\n!!\n!!\nexit\n");
    assert_eq!(
        stdout.lines().filter(|line| line.ends_with("kept")).count(),
        3,
        "each !! re-runs the stored expansion: {stdout:?}"
    );

    let saved = std::fs::read_to_string(dir.path().join(".oslo_history")).expect("history file");
    assert!(
        !saved.contains("!!"),
        "the raw reference must never be written to the history file: {saved:?}"
    );
}
