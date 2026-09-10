//! Evaluating command lists, and-or lists and pipelines.
//!
//! The top of the evaluator: a script is a list of and-or lists, each a chain of pipelines,
//! each a chain of commands connected by pipes. Individual commands are handed to
//! [`crate::exec::simple`] or [`crate::exec::compound`].
//!
//! This module also owns the three things every fork site in the shell needs to agree on:
//! [`status_of`] (what an evaluation result becomes when the only channel left is an exit
//! status), [`wait_for_status`] (what a reaped child's wait status becomes), and
//! [`set_interactive`] (whether there is a person watching). Subshells and command
//! substitutions live in other modules but must use these, or the same status bug reappears
//! once per fork site.
//!
//! `set -e` is decided here too, in [`eval_and_or_list`], because that is the only place that
//! knows which command of a list POSIX makes it answerable for. Constructs whose *conditions* are
//! exempt reach in through `suspend_errexit`.

mod coordinates;
mod describe;
mod errexit;
mod interrupt;
mod jobs;
/// What each link of `a && b` did — see the module docs.
pub mod segments;
mod structured;
mod timing;

pub(crate) use errexit::{clear_status_exempt, errexit_suspended, suspend_errexit};
use errexit::{err_reported, set_err_reported, set_status_exempt, status_exempt};

use crate::env::Environment;
use crate::env::builtins::run_exit_trap;
use crate::exec::compound::{eval_compound_command, flush_stdout};
use crate::exec::job;
use crate::exec::redirect::RedirectGuard;
use crate::exec::simple::{eval_simple_command, report_redirect_failure};
use interrupt::ListFrame;
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, close, dup2, fork, pipe};
use oslo_base::ast::*;
use oslo_base::error::{Result, ShellError};
use std::os::fd::{AsRawFd, IntoRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
/// Waiting on a child, and turning what came back into a status.
mod waiting;
pub(crate) use waiting::wait_for_child;
pub use waiting::{
    is_interactive, report_error_status, set_interactive, status_of, wait_for_status,
};

/// Run a command list, and absorb an interrupt that unwound all the way out of it.
///
/// The two responsibilities are split so that the *frame* — which knows whether this is the
/// outermost list in this process — is entered and left exactly once, on every path including the
/// error one. See the `interrupt` submodule for why the outermost frame is where a Ctrl-C
/// stops being an unwind and becomes a status.
pub fn eval_command_list(env: &mut Environment, cmd_list: &CommandList) -> Result<i32> {
    // **The third spender of the stack, and the one nothing counted.** A function call and a
    // `source` chain each had a counter; a nested compound command had only the *parse-time* limit
    // on how deep the input may be, which says nothing about how much stack running it costs. This
    // is where `{ … }`, `( … )`, a loop body and a branch all re-enter, so it is where the question
    // is asked. See `oslo_base::stack`.
    if oslo_base::stack::exhausted() {
        return Err(ShellError::ExecutionError(
            "maximum nesting level exceeded".to_string(),
        ));
    }
    let frame = ListFrame::enter();
    frame.absorb(run_list_items(env, cmd_list))
}

fn run_list_items(env: &mut Environment, cmd_list: &CommandList) -> Result<i32> {
    let mut last_status = 0;

    for item in &cmd_list.items {
        // `set -n`: read the program, run none of it. Checked per command rather than once before
        // the list so that `set -n` *within* a script stops execution from that point, which is
        // where bash stops too — and so that `oslo -n script` never reaches a command at all.
        //
        // POSIX excuses interactive shells from honouring it, which is what stops a stray `set -n`
        // at a prompt from turning the session into a shell that silently ignores everything.
        //
        // This is a gate on *execution*, not on parsing: the program is already parsed by the time
        // it gets here, so a syntax error has been reported whether or not this option is set,
        // which is the whole point of `sh -n`.
        if env.noexec() {
            return Ok(last_status);
        }
        // `$LINENO` is the line of the command about to run, so it is set here rather than at
        // parse time. Items built by the shell itself (a `$(…)` body re-lexed into a list, an
        // `eval` string) carry 0 and leave whatever the enclosing script set, which is what a
        // reader of `$LINENO` inside a function expects to see.
        if item.line > 0 {
            env.note_line(item.line);
        }
        // R7.4: a command boundary is the one moment the shell is provably not waiting for a
        // foreground child, which is what makes reaping every finished background job here safe.
        job::reap_background_jobs();
        // R7.2: and the one moment a Ctrl-C can be acted on when nothing has entered the kernel —
        // `while true; do :; done` never blocks, so polling is the only way out of it.
        if job::interrupt_pending() {
            return Err(interrupt::raise(env));
        }
        // A SIGTERM or SIGHUP that arrived while an EXIT trap was set. Ending as an ordinary
        // `exit 128 + signo` is what runs that trap: the shell dies of the signal either way, and
        // this is the difference between the cleanup happening and not.
        if let Some(signum) = job::fatal_signal_pending() {
            return Err(ShellError::Exit(128 + signum));
        }

        if item.op == ListOp::Background {
            jobs::spawn_background(env, &item.and_or)?;
            // Starting a job always succeeds from the list's point of view; the job's own status
            // is collected later by `wait`.
            last_status = 0;
            env.last_status = 0;
        } else {
            last_status = eval_and_or_list(env, &item.and_or)?;
        }
    }

    // And once more on the way out, for a signal that arrived during the *last* command of the
    // list: there is no boundary after it, and the shell would otherwise end normally and report
    // the command's status rather than the signal's.
    if let Some(signum) = job::fatal_signal_pending() {
        return Err(ShellError::Exit(128 + signum));
    }

    Ok(last_status)
}

pub fn eval_and_or_list(env: &mut Environment, and_or: &AndOrList) -> Result<i32> {
    // Whether this is the chain the user typed rather than one inside a compound's body. Answered
    // and released around the whole walk, including the error path, so a chain that aborts does
    // not leave the recorder thinking it is still inside one.
    let outermost = segments::enter();
    let result = walk_and_or(env, and_or, outermost);
    segments::leave(outermost);
    result
}

/// The chain itself, with `recording` saying whether each link is worth writing down.
fn walk_and_or(env: &mut Environment, and_or: &AndOrList, recording: bool) -> Result<i32> {
    // Only the *last* pipeline of the chain is judged by `set -e`: POSIX exempts "any command of
    // an AND-OR list other than the last", and a short-circuited chain therefore never fails the
    // shell at all — `set -e; false && echo no` carries on, because the only command that ran was
    // an exempt one. A lone pipeline is its own last, so `set -e; false` still aborts.
    let last = and_or.rest.len();
    let mut status = timed_link(
        env,
        &and_or.first,
        last == 0,
        recording,
        segments::Join::First,
    )?;

    for (i, (op, next_pipeline)) in and_or.rest.iter().enumerate() {
        let run = match op {
            AndOrOp::And => status == 0,
            AndOrOp::Or => status != 0,
        };
        let join = segments::Join::of(*op);
        if run {
            status = timed_link(env, next_pipeline, i + 1 == last, recording, join)?;
        } else if recording {
            // **The link that never ran.** Recording it is the point: "the chain stopped before
            // here" cannot be told from "this succeeded" without it, and that is what a resume
            // offer has to know.
            segments::record_skipped(join, describe::describe_pipeline(next_pipeline));
        }
    }

    Ok(status)
}

/// [`run_and_record`], timed and written to the segment buffer when the REPL asked for it.
///
/// The clock is only read when recording: a script runs the same chain with no `Instant::now`
/// anywhere near it.
fn timed_link(
    env: &mut Environment,
    pipeline: &Pipeline,
    judged: bool,
    recording: bool,
    join: segments::Join,
) -> Result<i32> {
    if !recording {
        return run_and_record(env, pipeline, judged);
    }
    let started = std::time::Instant::now();
    let outcome = run_and_record(env, pipeline, judged);
    let elapsed = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    // Recorded whatever happened. A link that ended the shell or raised still ran, and a report
    // that omitted it would be describing a different line than the one that was typed.
    if let Ok(status) = &outcome {
        segments::record(
            join,
            describe::describe_pipeline(pipeline),
            *status,
            elapsed,
        );
    }
    outcome
}

/// Run one pipeline, publish its status as `$?`, and let `set -e` judge it if it is judgeable.
///
/// `$?` used to be written once per top-level list item, so the right-hand side of an and-or list
/// saw the status from *before* the list started: `true; false || echo $?` printed 0. Every
/// pipeline is a command whose status POSIX makes visible to the next one, and the and-or
/// operators are exactly where that is observable.
///
/// `judged` says whether this pipeline is the last of its and-or chain. When it is not, the
/// exemption covers the whole evaluation — not just this pipeline's own status — so that a
/// function on the left of `||` runs to its end instead of dying inside.
fn run_and_record(env: &mut Environment, pipeline: &Pipeline, judged: bool) -> Result<i32> {
    // Cleared first, so what is read below can only have been set by *this* pipeline's own
    // evaluation — a compound's body setting it on the way out — and never left over from an
    // unrelated command that ran earlier.
    set_status_exempt(false);
    set_err_reported(false);

    if !judged {
        let _exempt = suspend_errexit();
        let status = eval_pipeline(env, pipeline)?;
        env.last_status = status;
        // Every command of an and-or list but the last is exempt, and so is the status it leaves.
        set_status_exempt(true);
        return Ok(status);
    }

    let status = eval_pipeline(env, pipeline)?;
    env.last_status = status;
    // **A compound inherits its body's last status, exemption included.** `if true; then false &&
    // echo no; fi` reports 1 because the `&&` short-circuited, and that 1 was never judgeable —
    // so the `if` carrying it is not judgeable either.
    let inherited = status_exempt();
    // `! cmd` is exempt whichever way it comes out, so `set -e; ! true` does not end the shell
    // even though the pipeline reports 1.
    // The ERR trap shares this condition and not `set -e` itself: bash fires it for exactly the
    // failures errexit is allowed to judge, whether or not errexit is on. See `run_err_trap`.
    if status != 0 && !pipeline.negated && !inherited && !errexit_suspended() {
        // Once per failure, not once per construct carrying its status out — see `ERR_REPORTED`.
        if !err_reported() {
            crate::env::builtins::run_err_trap(env)?;
            set_err_reported(true);
        }
        if env.errexit() {
            return Err(ShellError::Exit(status));
        }
    }
    // Carried on rather than cleared, so it survives another layer of nesting: the outer `if` of
    // `if true; then if true; then false && :; fi; fi` inherits from the inner one, which
    // inherited from the `&&`. Clearing here stopped the exemption one level short.
    set_status_exempt(pipeline.negated || inherited);
    Ok(status)
}

/// Run a pipeline, and — if the `time` keyword prefixed it — report how long it took.
///
/// R8.7: `timed` was parsed and dropped, so `time sleep 0.2` printed nothing and reported 0. The
/// timing is strictly additive: the pipeline's status, its stdout and its `$?` are whatever they
/// would have been untimed, and the report goes to stderr so that `x=$(time cmd)` still captures
/// `cmd`'s output alone.
pub fn eval_pipeline(env: &mut Environment, pipeline: &Pipeline) -> Result<i32> {
    if !pipeline.timed {
        return run_pipeline(env, pipeline);
    }

    let timer = timing::Timer::start();
    let result = run_pipeline(env, pipeline);
    // Stopped on the error path too: bash prints the three lines for `time exit 3` and *then*
    // exits 3, and `exit`/`return`/`break` all reach here as errors carrying their status.
    //
    // The flush is not cosmetic. stdout is buffered and stderr is not, so `time echo hi >f 2>&1`
    // would otherwise record the timing above the output it timed.
    flush_stdout();
    timer.report();
    result
}

/// Everything `time` measures: the stages, and the `!` applied to what they returned.
fn run_pipeline(env: &mut Environment, pipeline: &Pipeline) -> Result<i32> {
    if pipeline.commands.is_empty() {
        return Ok(0);
    }

    // R6.2: a pipeline under `!` is exempt from `set -e`, and so is everything it reaches —
    // `set -e; ! f` runs `f` to its end rather than dying at the first failing command in it.
    let _exempt = pipeline.negated.then(suspend_errexit);
    let status = run_stages(env, pipeline)?;

    Ok(if pipeline.negated {
        i32::from(status == 0)
    } else {
        status
    })
}

/// Run the pipeline's stages and report the status the pipeline itself has, before `!` is applied.
///
/// With `pipefail` that is the rightmost stage that failed, not the rightmost stage: the option
/// exists so that `generate | filter` cannot report success when `generate` died.
fn run_stages(env: &mut Environment, pipeline: &Pipeline) -> Result<i32> {
    // **The one new branch.** The planner decides, before any stage runs, whether any edge of this
    // pipeline carries structured rows. Until a structured tool exists it answers no for every
    // pipeline ever written, and the rest of this function is reached exactly as it always was.
    //
    // Deliberately a question asked here rather than a check scattered through the stages: there
    // is one place where the byte path can be left, and it is this line.
    // **Refused before the byte path applies anything.** A structured verb in the middle of a
    // pipeline that redirects its own stdout has no coherent meaning — its rows go to the next
    // stage, not to a descriptor — and letting it fall through answered `first: command not found`
    // while creating the empty file the redirection named. A name only oslo invented can reach
    // this, so no script written against another shell can.
    if let Some(problem) = structured::refuse_redirected_middle(pipeline) {
        eprintln!("{}{problem}", crate::env::origin_now());
        env.set_pipeline_status(vec![2]);
        return Ok(2);
    }
    if let Some(sinks) = structured::structured_sinks(pipeline) {
        // Nothing takes this path yet — the tools that would are the next stage of the work. It is
        // wired now, with the corpus proving it is never reached, so that the commit which adds
        // the first structured tool is small and its blast radius is already measured.
        return structured::run(env, pipeline, &sinks, run_byte_stages);
    }
    // **The second question asked before anything runs**, and the same shape as the first: a stage
    // that reads its upstream by coordinate cannot start until that upstream has finished, so the
    // whole pipeline runs one stage at a time. A pipeline with no coordinate in it — which is
    // nearly all of them — answers `false` here and forks concurrently as it always has.
    if coordinates::uses_coordinates(pipeline) {
        return coordinates::run(env, pipeline);
    }
    run_byte_stages(env, pipeline)
}

/// The pipeline as oslo has always run it: one process per stage, bytes on every descriptor.
fn run_byte_stages(env: &mut Environment, pipeline: &Pipeline) -> Result<i32> {
    if pipeline.commands.len() == 1 {
        let status = eval_command(env, &pipeline.commands[0])?;
        // R4.10: a one-command pipeline still has a stage vector, as bash's `PIPESTATUS` does.
        env.set_pipeline_status(vec![status]);
        return Ok(status);
    }

    let num_cmds = pipeline.commands.len();
    let mut pipes = Vec::new();

    for _ in 0..num_cmds - 1 {
        let p = pipe()
            .map_err(|e| ShellError::ExecutionError(format!("Pipe creation failed: {}", e)))?;
        pipes.push(p);
    }

    let mut pids = Vec::new();
    // R7.1: every stage of a foreground pipeline shares one process group, led by the first
    // stage. That is what makes Ctrl-C reach the whole pipe and only the pipe — `kill(-pgid)` is
    // how the tty driver delivers it — and what lets Ctrl-Z stop all of it at once.
    let mut pgid: Option<Pid> = None;

    for (idx, cmd) in pipeline.commands.iter().enumerate() {
        unsafe {
            match fork() {
                Ok(ForkResult::Child) => {
                    // Before the signal reset, and before any of this stage's own work: the
                    // parent is about to hand the terminal to this group, and a stage that had
                    // not joined it yet would be a background process reading from the tty.
                    job::join_foreground_group(pgid);
                    // R4.7: a stage is a new process running commands, so it gets a fresh signal
                    // state — otherwise the Rust runtime's inherited `SIG_IGN` for SIGPIPE turns
                    // `while :; do echo x; done | head -1` from an instant death on the closed
                    // pipe into a loop that spins to completion swallowing EPIPE.
                    crate::exec::job::reset_signals_for_child();
                    if idx > 0 {
                        let prev_read = pipes[idx - 1].0.as_raw_fd();
                        let _ = dup2(prev_read, 0);
                    }
                    if idx < num_cmds - 1 {
                        let curr_write = pipes[idx].1.as_raw_fd();
                        let _ = dup2(curr_write, 1);
                    }

                    for p in &pipes {
                        let _ = close(p.0.as_raw_fd());
                        let _ = close(p.1.as_raw_fd());
                    }

                    // R4.1: a stage is a subshell — same environment, subshell-local state only.
                    env.enter_subshell();
                    // R4.2: `echo x | exit 3` exits the stage with 3, not with 1.
                    let status = status_of(eval_command(env, cmd));
                    flush_stdout();
                    // Each stage is a subshell, so a trap it installed for its own exit fires here.
                    std::process::exit(run_exit_trap(env, status));
                }
                Ok(ForkResult::Parent { child }) => {
                    // The same `setpgid` the child just made, from the other side: whichever
                    // process is scheduled first, the group exists before anyone signals it.
                    pgid = job::place_foreground_child(child, pgid).or(pgid);
                    pids.push(child);
                }
                Err(e) => return Err(ShellError::ExecutionError(format!("Fork failed: {}", e))),
            }
        }
    }

    for p in pipes {
        let _ = close(p.0.into_raw_fd());
        let _ = close(p.1.into_raw_fd());
    }

    // R7.1: the pipeline is the foreground job now, so it — not the shell — owns the terminal
    // until it is done. The handover and the take-back are both `tcsetpgrp` calls made from a
    // process that is no longer the foreground group, so both go through the SIGTTOU block.
    if let Some(pgid) = pgid {
        job::give_terminal_to(pgid);
    }

    // R4.10: every stage's status is kept, not just the last one, so `false | true` leaves a
    // trace of the failure for `PIPESTATUS` (Round 8) and `pipefail` (Round 6) to read.
    let mut stage_statuses = Vec::with_capacity(pids.len());
    let mut stopped = false;
    for pid in &pids {
        // R4.3: exactly one `waitpid` per child, inside `wait_for_child`.
        let (status, was_stopped) = wait_for_child(*pid);
        stopped |= was_stopped;
        stage_statuses.push(status);
    }
    // Every stage exited on its own, and none was stopped: the terminal is as they left it on
    // purpose. See `job::reclaim_terminal`.
    job::reclaim_terminal(
        !stopped
            && stage_statuses
                .iter()
                .all(|status| job::left_it_deliberately(*status)),
    );
    if stopped && let Some(pgid) = pgid {
        jobs::remember_stopped_pipeline(pgid, &pids, pipeline);
    }
    segments::note_pipeline(&pipeline.commands, &stage_statuses);
    let final_status = pipeline_status(env, &stage_statuses);
    env.set_pipeline_status(stage_statuses);
    Ok(final_status)
}

/// The status a pipeline reports, given what each of its stages returned.
///
/// R6.4: `set -o pipefail` swaps "the last stage" for "the rightmost stage that failed", so
/// `false | true` reports 1 while `true | false | true` still reports the 1 in the middle. Without
/// the option the leftmost stages are invisible, which is the default POSIX rule.
pub(super) fn pipeline_status(env: &Environment, stages: &[i32]) -> i32 {
    if env.pipefail() {
        // Rightmost, not first: `exit 3 | exit 4` is 4 in bash, the failure closest to the output.
        if let Some(failed) = stages.iter().rev().find(|s| **s != 0) {
            return *failed;
        }
    }
    stages.last().copied().unwrap_or(0)
}

pub fn eval_command(env: &mut Environment, command: &Command) -> Result<i32> {
    match command {
        Command::Simple(simple) if is_assignment_only(simple) => {
            assignment_only_status(env, simple)
        }
        Command::Simple(simple) => eval_simple_command(env, simple),
        Command::Compound { kind, redirections } => {
            let mut guard = RedirectGuard::new();
            // R4.8: a redirection the shell cannot perform fails the *command*, it does not abort
            // the script — `{ echo hi; } < /nonexistent` prints a diagnostic, leaves `$?` at 1 and
            // the next command still runs. Propagating the error here made it fatal, which is the
            // one thing bash reserves for a special builtin in POSIX mode.
            if let Err(e) = guard.apply(env, redirections) {
                return Ok(report_redirect_failure(&env.origin(), &e));
            }
            eval_compound_command(env, kind)
        }
        Command::FunctionDef { name, body } => {
            env.set_function(name, *body.clone());
            Ok(0)
        }
    }
}

/// A command that is nothing but variable assignments: `x=1`, `x=$(cmd) y=2`.
fn is_assignment_only(simple: &SimpleCommand) -> bool {
    simple.words.is_empty() && !simple.assignments.is_empty()
}

/// Run an assignment-only command and give it the status POSIX 2.9.1 assigns it: that of the
/// last command substitution performed, or 0 if there was none.
///
/// `x=$(exit 5)` reports 5 and `x=plain` reports 0 — an assignment cannot itself fail, so the
/// only thing left to report is what ran inside it. Returning a flat `Ok(0)` hid every failure a
/// script captures this way, and `out=$(cmd) || handle` is a common idiom.
///
/// `$?` deliberately still holds the *previous* command's status while the right-hand sides are
/// expanded, so `x=$?` keeps working; the substitution's own status travels on the side channel
/// and only becomes `$?` once the whole command is done.
fn assignment_only_status(env: &mut Environment, simple: &SimpleCommand) -> Result<i32> {
    // Discard anything a previous command left: `echo $(false); x=1` reports 0, because that
    // substitution belonged to the `echo`.
    env.take_substitution_status();
    // A redirection that could not be performed outranks the substitution (R4.8):
    // `x=$(true) < /nonexistent` is a failed command, whatever the substitution did.
    let status = eval_simple_command(env, simple)?;
    if status != 0 {
        return Ok(status);
    }
    Ok(env.take_substitution_status().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::{errexit_suspended, pipeline_status, suspend_errexit};
    use crate::env::Environment;
    use crate::env::options::ShellOption;

    fn env_with_pipefail(on: bool) -> Environment {
        let mut env = Environment::new();
        env.set_option(ShellOption::PipeFail, on);
        env
    }

    /// The POSIX default: the stages to the left of the last one cannot be seen at all.
    #[test]
    fn without_pipefail_only_the_last_stage_decides() {
        let env = env_with_pipefail(false);
        assert_eq!(pipeline_status(&env, &[1, 0]), 0);
        assert_eq!(pipeline_status(&env, &[0, 3]), 3);
        assert_eq!(pipeline_status(&env, &[4, 5, 0]), 0);
    }

    /// Rightmost, not first: `(exit 3) | (exit 4)` is 4, the failure nearest the output.
    #[test]
    fn pipefail_reports_the_rightmost_failure() {
        let env = env_with_pipefail(true);
        assert_eq!(pipeline_status(&env, &[1, 0]), 1);
        assert_eq!(pipeline_status(&env, &[3, 4, 0]), 4);
        assert_eq!(pipeline_status(&env, &[0, 5, 0]), 5);
        assert_eq!(pipeline_status(&env, &[0, 0, 0]), 0);
        assert_eq!(pipeline_status(&env, &[0, 0, 6]), 6);
    }

    /// An empty stage vector is not a failure; a pipeline with no commands reports success.
    #[test]
    fn no_stages_is_zero_either_way() {
        assert_eq!(pipeline_status(&env_with_pipefail(true), &[]), 0);
        assert_eq!(pipeline_status(&env_with_pipefail(false), &[]), 0);
    }

    /// The exemptions nest — `if f; then` inside `g || true` — so the flag has to be a count.
    /// Setting a bool instead is how `set -e` comes back on halfway through a condition.
    #[test]
    fn errexit_suspensions_nest_and_unwind() {
        assert!(!errexit_suspended());
        let outer = suspend_errexit();
        {
            let _inner = suspend_errexit();
            assert!(errexit_suspended());
        }
        assert!(
            errexit_suspended(),
            "the outer suspension is still in force"
        );
        drop(outer);
        assert!(!errexit_suspended());
    }
}
