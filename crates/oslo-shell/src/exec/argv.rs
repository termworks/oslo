//! Running a command from an argv list, with output optionally captured.
//!
//! This is what Lua's `oslo.run{…}` and `sh.grep(…)` reach, and it is deliberately *not* a second
//! executor: it hands the list to [`crate::exec::simple::run_argv`], which is the same command
//! search the shell itself uses. What lives here is only the part the shell expresses with syntax
//! instead — whether output is captured, and what the result looks like afterwards.
//!
//! Capture is opt-in. A shell that buffered every command's output by default would hold all of
//! `cargo build` in memory to answer a question nobody asked; running with output going straight
//! to the terminal is both the common case and the cheap one.

mod drain;

use crate::env::Environment;
use crate::exec::compound::flush_stdout;
use crate::exec::pipeline::status_of;
use drain::{drain, drain_until, pipe_pair};

use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{ForkResult, close, dup2, fork};
use oslo_base::error::{Result, ShellError};
use std::os::fd::{AsRawFd, IntoRawFd, OwnedFd};

/// Which streams the caller wants back rather than on the terminal.
#[derive(Debug, Clone, Copy, Default)]
pub struct Capture {
    pub stdout: bool,
    pub stderr: bool,
}

impl Capture {
    /// Whether anything at all is captured, which decides whether a fork is needed.
    pub fn any(self) -> bool {
        self.stdout || self.stderr
    }
}

/// What a command left behind.
///
/// `out` and `err` are `None` when the stream was not captured, never `Some("")` — "the command
/// printed nothing" and "nobody was listening" are different facts, and a script that cannot tell
/// them apart will eventually treat one as the other.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub status: i32,
    pub out: Option<String>,
    pub err: Option<String>,
    /// The signal that killed the command, when one did.
    ///
    /// Kept separate from `status` because `128 + n` cannot distinguish a process killed by
    /// signal `n` from one that called `exit(128 + n)`, and `oslo.run{"sh", "-c", "exit 130"}`
    /// is not an interrupt.
    pub signal: Option<i32>,
    /// Whether the deadline passed with the command still running. See [`Limit`].
    pub timed_out: bool,
}

/// How long a command may take, and what to send when it does not finish in time.
#[derive(Debug, Clone, Copy)]
pub struct Limit {
    pub ms: u64,
    /// Sent at the deadline. Not an escalation ladder — a TERM-then-KILL sequence is a second
    /// timer and a second decision, and a caller who wants `KILL` says so.
    pub signal: nix::sys::signal::Signal,
}

/// Resolve `argv[0]` **before** forking, so the shell's own table learns where it lives.
///
/// **Every function in this file forks and then resolves the command in the child**, which execs
/// and dies — so the hash table, which lives in the shell process, was never warmed by anything
/// spawned this way. The lookup happened, was paid for, and was thrown away with the child.
///
/// A prompt calling `oslo.run{"hexe", …}` therefore re-walked `$PATH` on every prompt. `execvp`
/// tries each entry in turn, so with a Nix dev shell's 48-entry `$PATH` and the binary near the
/// end that is 48 `execve` calls per spawn, five spawns per prompt, forever: measured at 242
/// wasted `execve` per command in this repository. One lookup here in the parent makes the next
/// one a table hit and the child's exec a single `execve`.
///
/// A word with a `/` is not a search, and a builtin or function is not one either; those are left
/// alone rather than being taught to the table as commands they are not.
fn warm(env: &Environment, argv: &[String]) {
    let Some(name) = argv.first() else {
        return;
    };
    if name.contains('/') || env.get_builtin(name).is_some() || env.get_function(name).is_some() {
        return;
    }
    let _ = crate::env::builtins::hash_lookup(name);
}

/// Run `argv`, capturing whatever `capture` asks for.
pub fn run(env: &mut Environment, argv: &[String], capture: Capture) -> Result<Outcome> {
    run_limited(env, argv, capture, None)
}

/// [`run`], with a deadline.
///
/// **A limit forces the forking path**, and that is a semantic change worth stating: without one, a
/// command with nothing to capture runs *in this shell*, which is what makes `sh.cd("/tmp")` move
/// it. There is no way to time-limit that — it dispatches builtins and shell functions in-process —
/// so `oslo.run{"cd", "/tmp", timeout_ms = 100}` runs in a child and no longer moves the shell.
/// That is the honest reading of a timeout: you cannot interrupt the thing that *is* the shell.
pub fn run_limited(
    env: &mut Environment,
    argv: &[String],
    capture: Capture,
    limit: Option<Limit>,
) -> Result<Outcome> {
    if argv.is_empty() {
        return Err(ShellError::ExecutionError(
            "oslo.run: the command list is empty".to_string(),
        ));
    }

    warm(env, argv);

    // Nothing to capture means nothing to fork for, and running in this shell is what makes
    // `sh.cd("/tmp")` and `sh.export(…)` affect the shell rather than a child that then exits.
    if !capture.any() && limit.is_none() {
        let status = status_of(crate::exec::simple::run_argv(env, argv));
        return Ok(Outcome {
            status,
            ..Outcome::default()
        });
    }

    capture_run(env, argv, capture, limit)
}

/// The forking path: the command runs in a child with its streams on pipes.
fn capture_run(
    env: &mut Environment,
    argv: &[String],
    capture: Capture,
    limit: Option<Limit>,
) -> Result<Outcome> {
    let out_pipe = capture.stdout.then(pipe_pair).transpose()?;
    let err_pipe = capture.stderr.then(pipe_pair).transpose()?;

    // Anything already buffered belongs to this shell's stdout, not to the captured output.
    flush_stdout();

    // Safety: between `fork` and `exit`/`exec` the child touches only `close`, `dup2` and the
    // shell state it already owns a copy of — the same discipline every other fork here follows.
    match oslo_base::forksafe::quiesced(|| unsafe { fork() }) {
        Ok(ForkResult::Child) => {
            crate::exec::job::reset_signals_for_child();
            // **Its own process group, but only when there is a deadline to enforce.**
            //
            // This child is *oslo*, running the command in a subshell — so the command itself is a
            // grandchild, and signalling the child alone leaves it sitting in `wait` while the
            // grandchild runs on. Measured: `sh -c 'echo before; sleep 10'` with a 300 ms deadline
            // reported `timed_out` at 300 ms and then took the full ten seconds to return.
            //
            // A group makes one signal reach both, which is what `timeout(1)` does. Only under a
            // limit, because a group of its own takes the command out of this shell's job control,
            // and that is a real cost to pay for something nobody asked for.
            if limit.is_some() {
                let _ = nix::unistd::setpgid(
                    nix::unistd::Pid::from_raw(0),
                    nix::unistd::Pid::from_raw(0),
                );
            }
            if let Some((reader, writer)) = out_pipe {
                let _ = close(reader.into_raw_fd());
                let _ = dup2(writer.as_raw_fd(), 1);
                let _ = close(writer.into_raw_fd());
            }
            if let Some((reader, writer)) = err_pipe {
                let _ = close(reader.into_raw_fd());
                let _ = dup2(writer.as_raw_fd(), 2);
                let _ = close(writer.into_raw_fd());
            }
            env.enter_subshell();
            let status = status_of(crate::exec::simple::run_argv(env, argv));
            flush_stdout();
            finish_child(status);
        }
        Ok(ForkResult::Parent { child }) => {
            let out_reader = out_pipe.map(|(reader, writer)| {
                let _ = close(writer.into_raw_fd());
                reader
            });
            let err_reader = err_pipe.map(|(reader, writer)| {
                let _ = close(writer.into_raw_fd());
                reader
            });
            let (out, err, timed_out) = drain_until(out_reader, err_reader, child, limit);
            let (status, signal) = wait(child);
            Ok(Outcome {
                status,
                out: capture.stdout.then_some(out),
                err: capture.stderr.then_some(err),
                signal,
                timed_out,
            })
        }
        Err(e) => Err(ShellError::ExecutionError(format!("Fork failed: {e}"))),
    }
}

pub(super) fn pipe_failed(e: nix::Error) -> ShellError {
    ShellError::ExecutionError(format!("Pipe failed: {e}"))
}

/// Start `argv` with its stdout on a pipe, and hand back the child and the read end.
///
/// The caller reads as the command writes, which is the difference between `for line in
/// oslo.lines{"journalctl", "-f"}` working and hanging: capture buffers everything and answers
/// when the command ends, and a command that never ends never answers.
pub fn spawn_reading(
    env: &mut Environment,
    argv: &[String],
) -> Result<(nix::unistd::Pid, OwnedFd)> {
    spawn_reading_streams(env, argv, false)
}

/// [`spawn_reading`], with `merge_stderr` putting the command's stderr down the same pipe.
///
/// One pipe rather than two, because the caller that wants both wants them *interleaved* the way
/// they appeared — `keep make build` is worth having only if the error is in its place among the
/// lines that led to it, and two pipes drained separately cannot say which came first.
pub fn spawn_reading_streams(
    env: &mut Environment,
    argv: &[String],
    merge_stderr: bool,
) -> Result<(nix::unistd::Pid, OwnedFd)> {
    if argv.is_empty() {
        return Err(ShellError::ExecutionError(
            "oslo.lines: the command list is empty".to_string(),
        ));
    }
    warm(env, argv);
    let (reader, writer) = pipe_pair()?;
    flush_stdout();

    // Safety: as elsewhere in this file — the child rearranges its own descriptors and then runs
    // the command, never returning here.
    match oslo_base::forksafe::quiesced(|| unsafe { fork() }) {
        Ok(ForkResult::Child) => {
            crate::exec::job::reset_signals_for_child();
            let _ = close(reader.into_raw_fd());
            let _ = dup2(writer.as_raw_fd(), 1);
            if merge_stderr {
                let _ = dup2(writer.as_raw_fd(), 2);
            }
            let _ = close(writer.into_raw_fd());
            env.enter_subshell();
            let status = status_of(crate::exec::simple::run_argv(env, argv));
            flush_stdout();
            finish_child(status);
        }
        Ok(ForkResult::Parent { child }) => {
            let _ = close(writer.into_raw_fd());
            Ok((child, reader))
        }
        Err(e) => Err(ShellError::ExecutionError(format!("Fork failed: {e}"))),
    }
}

/// Reap a child started by [`spawn_reading`], once its output has run out.
pub fn reap(child: nix::unistd::Pid) -> i32 {
    wait(child).0
}

thread_local! {
    /// How the last external command ended, when a signal ended it.
    ///
    /// Set by [`crate::exec::simple::external`], which is the only place the real `WaitStatus`
    /// exists — everywhere else it has already been flattened to `128 + n`, which cannot tell a
    /// signal death from `exit(128 + n)`. Thread-local for the same reason the rest of the
    /// evaluator's ambient state is: the test binaries run scripts on several threads at once.
    static LAST_SIGNAL: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
}

/// Record how the command that just finished ended. See [`LAST_SIGNAL`].
pub(crate) fn note_signal(signal: Option<i32>) {
    LAST_SIGNAL.set(signal);
}

/// Leave a capture child, preserving *how* the command ended and not merely its number.
///
/// A shell running a command in a subshell has one channel back to its parent — a wait status —
/// so the only way to report "killed by SIGINT" rather than "exited 130" is to be killed by
/// SIGINT too. Re-raising is what bash does here for the same reason.
fn finish_child(status: i32) -> ! {
    if let Some(signal) = LAST_SIGNAL.get()
        && let Ok(sig) = nix::sys::signal::Signal::try_from(signal)
    {
        // Safety: restoring the default disposition for one signal in a process that is about to
        // end either way, before raising it at itself.
        unsafe {
            let _ = nix::sys::signal::signal(sig, nix::sys::signal::SigHandler::SigDfl);
        }
        let _ = nix::sys::signal::raise(sig);
    }
    std::process::exit(status);
}

/// Run several argvs as a pipeline, each stage's stdout feeding the next stage's stdin.
///
/// The status is the *last* stage's, as a shell pipeline's is. Every stage is waited for, so none
/// is left as a zombie even when an earlier one outlives the reader that was draining it.
pub fn pipe(env: &mut Environment, stages: &[Vec<String>], capture: Capture) -> Result<Outcome> {
    match stages.len() {
        0 => Err(ShellError::ExecutionError(
            "oslo.pipe: no stages".to_string(),
        )),
        // One stage is not a pipeline, and running it in this shell is what keeps
        // `oslo.pipe({"cd", "/tmp"})` meaningful.
        1 => run(env, &stages[0], capture),
        _ => spawn_pipeline(env, stages, capture),
    }
}

fn spawn_pipeline(
    env: &mut Environment,
    stages: &[Vec<String>],
    capture: Capture,
) -> Result<Outcome> {
    let out_pipe = capture.stdout.then(pipe_pair).transpose()?;
    let err_pipe = capture.stderr.then(pipe_pair).transpose()?;
    flush_stdout();

    let mut children = Vec::with_capacity(stages.len());
    // Carries the read end of the pipe the previous stage writes into.
    let mut upstream: Option<OwnedFd> = None;

    for (i, argv) in stages.iter().enumerate() {
        let last = i + 1 == stages.len();
        warm(env, argv);
        let downstream = if last { None } else { Some(pipe_pair()?) };

        // Safety: as in `capture_run` — the child only rearranges descriptors it owns before
        // running the command, and never returns to this loop.
        match oslo_base::forksafe::quiesced(|| unsafe { fork() }) {
            Ok(ForkResult::Child) => {
                crate::exec::job::reset_signals_for_child();
                if let Some(reader) = upstream {
                    let _ = dup2(reader.as_raw_fd(), 0);
                    let _ = close(reader.into_raw_fd());
                }
                if let Some((reader, writer)) = downstream {
                    let _ = close(reader.into_raw_fd());
                    let _ = dup2(writer.as_raw_fd(), 1);
                    let _ = close(writer.into_raw_fd());
                    // **And the capture pipe, which belongs to the last stage alone.** This was an
                    // `else if`, so a non-final stage kept *both* ends of a pipe it has nothing to
                    // do with: the write end holds EOF off from the reader, and the read end is a
                    // descriptor the command can see.
                    if let Some((reader, writer)) = out_pipe {
                        let _ = close(reader.into_raw_fd());
                        let _ = close(writer.into_raw_fd());
                    }
                } else if let Some((reader, writer)) = out_pipe {
                    let _ = close(reader.into_raw_fd());
                    let _ = dup2(writer.as_raw_fd(), 1);
                    let _ = close(writer.into_raw_fd());
                }
                // Every stage's stderr goes to the same place, which is what a shell pipeline
                // does: `a | b 2>&1` redirects only `b`, and there is no syntax here for that.
                if let Some((reader, writer)) = err_pipe {
                    let _ = close(reader.into_raw_fd());
                    let _ = dup2(writer.as_raw_fd(), 2);
                    let _ = close(writer.into_raw_fd());
                }
                env.enter_subshell();
                let status = status_of(crate::exec::simple::run_argv(env, argv));
                flush_stdout();
                finish_child(status);
            }
            Ok(ForkResult::Parent { child }) => {
                children.push(child);
                // The parent holds no end of a pipe it is not reading. Leaving the write end open
                // here is the classic pipeline hang: the downstream stage waits for an EOF that
                // only this process could send.
                upstream = downstream.map(|(reader, writer)| {
                    let _ = close(writer.into_raw_fd());
                    reader
                });
            }
            Err(e) => return Err(ShellError::ExecutionError(format!("Fork failed: {e}"))),
        }
    }
    if let Some(reader) = upstream {
        let _ = close(reader.into_raw_fd());
    }

    let out_reader = out_pipe.map(|(reader, writer)| {
        let _ = close(writer.into_raw_fd());
        reader
    });
    let err_reader = err_pipe.map(|(reader, writer)| {
        let _ = close(writer.into_raw_fd());
        reader
    });
    let (out, err) = drain(out_reader, err_reader);

    let mut last = (0, None);
    for child in children {
        last = wait(child);
    }
    Ok(Outcome {
        status: last.0,
        out: capture.stdout.then_some(out),
        err: capture.stderr.then_some(err),
        signal: last.1,
        // `oslo.pipe` takes no deadline: a limit belongs to one command, and which stage of a
        // pipeline a shared one applied to would be a question with no good answer.
        timed_out: false,
    })
}

/// Wait for the child, keeping "exited with 130" distinct from "killed by SIGINT".
fn wait(child: nix::unistd::Pid) -> (i32, Option<i32>) {
    loop {
        return match waitpid(child, Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Exited(_, code)) => (code, None),
            Ok(WaitStatus::Signaled(_, sig, _)) => (128 + sig as i32, Some(sig as i32)),
            Ok(WaitStatus::Stopped(_, sig)) => (128 + sig as i32, Some(sig as i32)),
            Ok(WaitStatus::StillAlive) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            // Nothing was reaped, so there is no status to report: 127 is what a shell says when
            // it cannot run or account for a command.
            _ => (127, None),
        };
    }
}
