//! Who owns a child's exit status, and who is still responsible for reaping it.
//!
//! Every test here reproduces a bug that a static reading of the source predicted and that was then
//! confirmed by running it. They are written as scripts through the real binary because all three
//! failures are about what the *kernel* and the job table each believe, which no in-process test of
//! either half alone can see.

mod common;

use common::run;

/// **A disowned child must still be reaped.**
///
/// `disown` says *stop managing this job*, not *stop being its parent* — the kernel does not care
/// what a shell's job table thinks, and only this process can reap it. Dropping the pid left a
/// `<defunct>` entry for the rest of the session.
#[test]
fn a_disowned_child_does_not_become_a_zombie() {
    let r = run(r#"sleep 0.2 & pid=$!
           disown
           sleep 1.2
           ps -o stat= -p $pid 2>/dev/null | tr -d ' ' || true"#);
    assert!(
        !r.out().starts_with('Z'),
        "the disowned child is a zombie: {:?}",
        r.out()
    );
}

/// The same shape without `disown`, so a failure above is about disowning rather than about
/// reaping in general.
#[test]
fn an_ordinary_background_child_is_reaped() {
    let r = run(r#"sleep 0.2 & pid=$!
           sleep 1.2
           ps -o stat= -p $pid 2>/dev/null | tr -d ' ' || true"#);
    assert!(
        !r.out().starts_with('Z'),
        "an ordinary background child is a zombie: {:?}",
        r.out()
    );
}

/// **`wait -n` on a child that has already finished must report its status.**
///
/// The status was collected by the opportunistic reaper and remembered; `-n` then threw it away and
/// waited on a pid the kernel had already forgotten, so the answer was 127 — while a plain `wait`
/// on the very same child answered correctly.
#[test]
fn wait_dash_n_reports_a_target_that_already_finished() {
    let r = run(r#"sh -c "exit 7" & p=$!
           sleep 0.4
           wait -n $p
           echo "got=$?""#);
    assert_eq!(r.out(), "got=7", "{}", r.err());
}

/// And the plain form, which was always right — the two must not disagree about one child.
#[test]
fn wait_reports_a_target_that_already_finished() {
    let r = run(r#"sh -c "exit 7" & p=$!
           sleep 0.4
           wait $p
           echo "got=$?""#);
    assert_eq!(r.out(), "got=7", "{}", r.err());
}

/// **A child reaped while waiting for somebody else keeps its status.**
///
/// `wait -n` waits with `waitpid(-1)`, so it collects whichever child ends first. One that is not
/// the target was noted as reaped and its exit code dropped on the floor, and the later `wait` that
/// asked for it got 127 and "not a child of this shell".
#[test]
fn a_child_reaped_while_waiting_for_another_keeps_its_status() {
    let r = run(r#"sh -c "sleep 0.1; exit 3" & a=$!
           sh -c "sleep 0.6; exit 4" & b=$!
           wait -n $b
           echo "b=$?"
           wait $a
           echo "a=$?""#);
    assert_eq!(r.lines(), vec!["b=4", "a=3"], "{}", r.err());
}

/// A pipeline reports its last stage, even when an earlier stage outlives it.
///
/// Predicted to be broken by reap order and found already correct; kept because the prediction was
/// specific and the property is worth pinning either way.
#[test]
fn a_pipeline_reports_its_last_stage_whatever_the_reap_order() {
    let background = run(r#"sh -c "sleep 0.4; exit 5" | sh -c "exit 9" & j=$!
           wait $j
           echo "got=$?""#);
    assert_eq!(background.out(), "got=9", "{}", background.err());

    let foreground = run(r#"sh -c "sleep 0.4; exit 5" | sh -c "exit 9"; echo "got=$?""#);
    assert_eq!(foreground.out(), "got=9", "{}", foreground.err());
}

/// A second `wait` for the same pid answers the same status, as bash 5.3's does.
#[test]
fn a_second_wait_answers_the_same_status() {
    let r = run(r#"sh -c "exit 7" & p=$!
           sleep 0.3
           wait $p; echo "first=$?"
           wait $p 2>/dev/null; echo "second=$?""#);
    assert_eq!(r.lines(), vec!["first=7", "second=7"], "{}", r.err());
}
