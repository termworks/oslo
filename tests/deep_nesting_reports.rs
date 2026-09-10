//! Too deep is an error, not an abort.
//!
//! Three constructs re-enter the interpreter and spend one stack: a function calling itself, a
//! `source`/`eval` chain, and a nested compound command. Each used to be counted against a limit of
//! its own, measured while the other two were idle — so a script doing all three at once overflowed
//! the stack and the process aborted, at a recursion depth the counter said was nowhere near its
//! limit.
//!
//! **Out of process on purpose.** The failure being tested was a `SIGABRT`, which no in-process
//! assertion survives to report: a test thread that overflows takes the whole test binary with it.
//! Spawning the shell is what lets the abort be observed as a status rather than as a missing
//! result. See `oslo_base::stack` for the guard itself.

mod common;

use common::oslo_bin;
use std::process::{Command, Output};

/// A script whose function recursion sits *inside* `nesting` levels of `{ … ; }`, so every level's
/// compound frames are live at once rather than unwinding between calls.
///
/// **Braces rather than subshells, and it is the difference between a test and a coffee break.**
/// `( … )` forks, so `{ ( … ) }` nested forty-five deep around a recursion of a thousand spawns tens
/// of thousands of processes: the same coverage took 537 seconds that way and 66 milliseconds this
/// way. What is being tested is how much *stack* a level costs, and a brace group costs the
/// evaluator frames without costing a process. `wrap_in_subshell` adds the one fork that the
/// containment case actually needs.
fn nested_recursion(nesting: usize, depth: usize) -> String {
    let open = " { ".repeat(nesting);
    let close = " ; } ".repeat(nesting);
    format!(
        "f() {{ [ \"$1\" -eq 0 ] && return 0;{open} f $(( $1 - 1 )) {close}; }}\n\
         f {depth}\n\
         echo \"status=$?\"\n\
         echo alive\n"
    )
}

/// The same, with the recursion inside a subshell so its failure is that subshell's status.
fn nested_recursion_in_subshell(nesting: usize, depth: usize) -> String {
    let open = " { ".repeat(nesting);
    let close = " ; } ".repeat(nesting);
    format!(
        "f() {{ [ \"$1\" -eq 0 ] && return 0;{open} f $(( $1 - 1 )) {close}; }}\n\
         ( f {depth} )\n\
         echo \"status=$?\"\n\
         echo alive\n"
    )
}

fn run(script: &str) -> Output {
    Command::new(oslo_bin())
        .arg("-c")
        .arg(script)
        .output()
        .expect("the shell ran")
}

/// The signature of the bug: the runtime's own words on the way out.
fn aborted(out: &Output) -> bool {
    let stderr = String::from_utf8_lossy(&out.stderr);
    stderr.contains("stack overflow") || out.status.code().is_none()
}

/// The shape that used to abort, over the range of shapes around it.
///
/// A range rather than the one measured case, because the whole point is that no single depth is
/// the boundary — how much stack a level costs depends on what is nested inside it, which is why
/// counting levels could not work.
#[test]
fn no_combination_of_depth_and_nesting_aborts() {
    // Corners rather than a grid: shallow nesting with the deepest recursion, the deepest nesting
    // with the shallowest, and the two between. A full sweep found nothing the corners did not.
    for (nesting, depth) in [(1, 5000), (45, 20), (10, 300), (45, 5000)] {
        let out = run(&nested_recursion(nesting, depth));
        assert!(
            !aborted(&out),
            "nesting {nesting}, depth {depth} aborted: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // Exceeding the limit ends the script, as it does in bash — so what is asserted is that it
        // ended by *reporting*, with an ordinary status, and not by dying of a signal.
        assert!(
            out.status.code().is_some(),
            "nesting {nesting}, depth {depth} was signalled, not reported"
        );
    }
}

/// When it does refuse, it refuses by name — and inside a subshell the parent carries on.
///
/// The containment is the reason the two cases read differently: a nesting failure ends the script
/// it happened in, and in a `( … )` that script is the subshell. Its status is 1, the shell around
/// it is untouched, and the next command runs.
#[test]
fn going_too_deep_is_reported_and_survivable() {
    let out = run(&nested_recursion_in_subshell(45, 5000));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("maximum nesting level exceeded"),
        "expected the nesting diagnostic, got: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("status=1"), "the subshell failed: {stdout}");
    assert!(stdout.contains("alive"), "the shell carried on: {stdout}");
}

/// **And the guard does not fire early.** A plain recursion still reaches the depth the counter
/// permits — the stack should bind when the stack is the problem, and not before. A test that only
/// checked "deep things are refused" would pass on a shell that refused everything.
#[test]
fn an_ordinary_recursion_still_reaches_the_counted_limit() {
    let script =
        "f() { [ \"$1\" -eq 0 ] && return 0; f $(( $1 - 1 )); }\nf 999\necho \"status=$?\"\n";
    let out = run(script);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("status=0"),
        "999 is under the limit of 1000 and must run: {stdout} / {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A `source`/`eval` chain spends the same stack, and is refused rather than allowed to overflow.
///
/// **Exceeding a nesting limit ends the script, and that is bash's behaviour, not a shortcoming.**
/// `bash -c 'FUNCNEST=50; …'` past the limit prints its message and stops — no further command runs
/// and the status is 1. What this asserts is the part that was wrong: it stops by *reporting*, with
/// an ordinary exit status, rather than by dying of a signal.
///
/// The nested case above survives to run the next command only because the guard fires inside a
/// `( … )` subshell, where the error is that subshell's status and the parent carries on.
#[test]
fn an_eval_chain_is_refused_rather_than_overflowed() {
    let script = "c() { [ \"$1\" -eq 0 ] && return 0; eval 'c $(( $1 - 1 ))'; }\n\
                  c 200\n\
                  echo alive\n";
    let out = run(script);
    assert!(
        !aborted(&out),
        "an eval chain aborted: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("maximum nesting level exceeded"),
        "expected the nesting diagnostic, got: {stderr}"
    );
    assert_eq!(out.status.code(), Some(1), "reported, not signalled");
}
