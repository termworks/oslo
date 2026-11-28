//! The two hooks a bash prompt integration hangs off.
//!
//! `$PROMPT_COMMAND` runs before every prompt and `$RPS1` gives the right-hand side. Both are only
//! *drawn* by an interactive shell, so what a non-interactive test can pin is that they are
//! ordinary variables that expand like `PS1` and are not run by accident. The drawing was checked
//! against a live pty.
//!
//! This file replaced `bind_tests.rs`. `bind` was removed — around 875 lines so that interactive
//! plugins could claim a keystroke, in a shell whose first job is to be `/bin/sh`.

mod common;

use common::run_in;

/// A non-interactive shell has no prompt, so it must not run `$PROMPT_COMMAND` at all.
#[test]
fn prompt_command_is_a_variable_not_a_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let r = run_in(
        dir.path(),
        "PROMPT_COMMAND='echo should-not-run'\necho \"set=$?\"",
    );
    assert_eq!(r.out(), "set=0", "stderr: {}", r.stderr);
    assert!(
        !r.out().contains("should-not-run"),
        "a script has no prompt and must not run it: {:?}",
        r.out()
    );
}

/// `$RPS1` / `$RPROMPT` hold their text unexpanded until a prompt is drawn, exactly as `PS1` does.
///
/// bash has no right prompt, so there was no name to inherit; both zsh spellings are accepted and
/// `RPS1` wins when both are set. Verified interactively: both spellings, prompt escapes, command
/// substitution, and that an empty value suppresses oslo's own right prompt rather than falling
/// back to it.
#[test]
fn the_right_prompt_variables_expand_like_ps1() {
    let dir = tempfile::tempdir().expect("tempdir");
    let r = run_in(
        dir.path(),
        "RPS1='$(echo substituted)'\nRPROMPT=other\necho \"[$RPS1] [$RPROMPT]\"",
    );
    assert_eq!(
        r.out(),
        "[$(echo substituted)] [other]",
        "stderr: {}",
        r.stderr
    );
}

/// `bind` is bash's builtin: a script without line editing gets a warning on stderr and status 0.
#[test]
fn bind_answers_as_bash_does_without_line_editing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let r = run_in(
        dir.path(),
        "bind -x '\"\\C-r\": handler' 2>/dev/null\necho \"status=$?\"",
    );
    assert_eq!(r.out(), "status=0");
}
