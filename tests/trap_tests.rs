//! End-to-end tests for `trap` (PLAN R6.5).
//!
//! These go through the real binary because the thing being tested is what happens *as the shell
//! exits*, and an in-process test cannot observe that: `main` is the only place the EXIT trap can
//! run, and `std::process::exit` is the last thing it does.
//!
//! The case that matters most is the last one. Every other assertion here is about text on
//! stdout; `an_exit_trap_actually_removes_the_temp_file` is about the filesystem afterwards,
//! which is the only evidence that a cleanup handler did its job. Before R6.5 the handler was
//! stored and never read, so every one of these scripts left its temp file behind while printing
//! nothing to say so.

mod common;

use common::{oslo_bin, run_in};
use std::process::{Command, Stdio};

/// The acceptance test: `trap 'rm -f …' EXIT` and the file is gone afterwards.
#[test]
fn an_exit_trap_actually_removes_the_temp_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scratch = dir.path().join("work.tmp");

    let r = run_in(
        dir.path(),
        r#"tmp=work.tmp
trap 'rm -f "$tmp"' EXIT
echo payload > "$tmp"
test -f "$tmp" && echo created
exit 4"#,
    );

    assert_eq!(r.out(), "created", "stderr: {}", r.stderr);
    assert_eq!(r.status, 4, "the handler must not change the exit status");
    assert!(
        !scratch.exists(),
        "the EXIT trap did not run: {} is still there",
        scratch.display()
    );
}

/// The same cleanup has to happen on the runs that went wrong, which is what it is for.
#[test]
fn the_exit_trap_runs_after_a_fatal_error_too() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scratch = dir.path().join("work.tmp");

    // An unset parameter under `set -u` is a *fatal* expansion error: it unwinds past everything
    // and ends the shell, which is exactly the path most likely to skip cleanup.
    let r = run_in(
        dir.path(),
        r#"trap 'rm -f work.tmp' EXIT
: > work.tmp
set -u
echo "$definitely_not_set"
echo NOT_REACHED"#,
    );

    assert!(!r.out().contains("NOT_REACHED"));
    assert!(!scratch.exists(), "cleanup was skipped on the error path");
}

/// `trap - EXIT` must undo the handler, not install one whose command name is `-`.
#[test]
fn resetting_removes_the_handler_rather_than_renaming_it() {
    let r = run_in(
        tempfile::tempdir().expect("tempdir").path(),
        "trap 'echo should_not_run' EXIT\ntrap - EXIT\necho body",
    );
    assert_eq!(r.out(), "body");
    // The old bug stored `-` as the handler text and then tried to run it, so the shell reported
    // a command not found on its way out.
    assert!(!r.stderr.contains("not found"), "stderr: {}", r.stderr);
}

/// A trapped signal runs its handler at the next command boundary and leaves `$?` alone.
#[test]
fn a_trapped_signal_runs_its_handler_between_commands() {
    let r = run_in(
        tempfile::tempdir().expect("tempdir").path(),
        "trap 'echo caught' USR1\nfalse\nkill -USR1 $$\necho \"after $?\"",
    );
    // `$?` belongs to the interrupted command sequence, not to the handler: `kill` succeeded, so
    // the status the next command sees is 0.
    assert_eq!(r.out(), "caught\nafter 0", "stderr: {}", r.stderr);
}

/// An untrapped SIGUSR1 kills the shell; the point of the trap is that it stops doing so.
#[test]
fn an_untrapped_signal_still_has_its_default_effect() {
    let r = run_in(
        tempfile::tempdir().expect("tempdir").path(),
        "kill -USR1 $$\necho NOT_REACHED",
    );
    assert!(!r.out().contains("NOT_REACHED"));
}

/// `trap '' SIG` has to survive what would otherwise end the shell.
#[test]
fn an_ignored_signal_is_discarded() {
    let r = run_in(
        tempfile::tempdir().expect("tempdir").path(),
        "trap '' USR1\nkill -USR1 $$\necho survived",
    );
    assert_eq!(r.out(), "survived", "stderr: {}", r.stderr);
}

/// Not implemented has to *say* not implemented. A stored-and-never-run handler is the one
/// outcome a script cannot detect for itself.
///
/// `DEBUG` left this list when it became real, and `ERR` has now followed it — see
/// [`the_err_trap_fires_for_a_failed_command`]. `RETURN` is what is left.
#[test]
fn the_unsupported_conditions_refuse_loudly() {
    let condition = "RETURN";
    let r = run_in(
        tempfile::tempdir().expect("tempdir").path(),
        &format!("trap 'echo handler' {condition}\necho \"status=$?\""),
    );
    assert_eq!(r.out(), "status=1", "{condition}");
    assert!(
        r.stderr.contains("not supported"),
        "{condition}: stderr said {:?}",
        r.stderr
    );
}

/// `trap -l` names every signal this system can deliver, in the spelling the other operands take.
#[test]
fn dash_l_lists_the_signal_names() {
    let r = run_in(tempfile::tempdir().expect("tempdir").path(), "trap -l");
    let names: Vec<&str> = r.out().split_whitespace().collect();
    assert!(names.contains(&"INT"), "{:?}", r.out());
    assert!(names.contains(&"TERM"));
    assert!(names.contains(&"KILL"));
}

/// End of input is a shell ending like any other, so the EXIT trap fires there too.
///
/// Fed on stdin rather than through `-c`: that is the path `main` handles separately, and it was
/// the one most likely to be forgotten.
#[test]
fn the_exit_trap_runs_at_end_of_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scratch = dir.path().join("work.tmp");
    std::fs::write(&scratch, b"x").expect("seed the file");

    let mut child = Command::new(oslo_bin())
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oslo");
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(b"trap 'rm -f work.tmp' EXIT\necho body\n")
            .expect("write script");
    }
    let out = child.wait_with_output().expect("wait");

    assert!(
        String::from_utf8_lossy(&out.stdout).contains("body"),
        "the script did not run at all"
    );
    assert!(
        !scratch.exists(),
        "the EXIT trap did not run at end of input"
    );
}

/// The DEBUG trap fires before each simple command.
///
/// End to end through a real shell rather than the unit test, because the two halves live apart:
/// `trap` records the condition and `exec::simple` fires it, and a test of either alone passed
/// while the pair did nothing.
///
/// The count is what is asserted, not `$BASH_COMMAND` — oslo does not set it, and
/// `tests/corpus/trap_debug.sh` says why.
#[test]
fn the_debug_trap_fires_before_each_command() {
    let r = run_in(
        tempfile::tempdir().expect("tempdir").path(),
        "n=0\ntrap 'n=$((n + 1))' DEBUG\nx=1\necho $n",
    );
    assert_eq!(r.out(), "2", "stderr: {}", r.stderr);
}

/// A handler's own commands must not fire it again, or the first `trap 'date' DEBUG` recurses
/// until the stack runs out.
#[test]
fn a_debug_handler_does_not_fire_itself() {
    let r = run_in(
        tempfile::tempdir().expect("tempdir").path(),
        "trap 'echo tick' DEBUG\necho done",
    );
    assert_eq!(r.out(), "tick\ndone", "stderr: {}", r.stderr);
}

/// **`set -e; trap cleanup ERR` is the error-handling idiom**, and the condition used to be
/// refused outright. It fires for a command that failed, under exactly the exemptions `set -e`
/// uses — an `if`/`while` condition, every command of an and-or list but the last, anything under
/// `!` — and once per failure, however many constructs carry that status out.
#[test]
fn the_err_trap_fires_for_a_failed_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let r = run_in(
        dir.path(),
        "trap 'echo caught' ERR\nfalse\nif false; then :; fi\nfalse || true\n! true\necho done",
    );
    assert_eq!(r.out(), "caught\ndone", "stderr: {}", r.stderr);
}

/// One failing command is one ERR. A function body's failure and the call reporting the status it
/// left are the same failure, and bash fires once for them.
#[test]
fn the_err_trap_fires_once_per_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let r = run_in(
        dir.path(),
        "trap 'echo caught' ERR\nf() { false; }\nf\ncase x in x) false;; esac\necho done",
    );
    assert_eq!(r.out(), "caught\ncaught\ndone", "stderr: {}", r.stderr);
}
