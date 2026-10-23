//! Running a pipeline whose stages read each other by coordinate.
//!
//! ```text
//! cat hosts.txt | ssh {0:0} uptime
//! ```
//!
//! `ssh`'s command line is not known until `cat` has produced a line, so the two cannot run at
//! once. This path runs the stages **one at a time**, keeping what each printed, and rewrites the
//! next stage's words from it before starting it.
//!
//! # Nothing else pays for it
//!
//! [`super::run_stages`] asks [`uses_coordinates`] before any stage runs, and a pipeline that says
//! no goes down the concurrent path it always did. The question is a scan of the literal words for
//! a `{` followed by a digit — no allocation, no capture, nothing kept.
//!
//! # Why the last stage is not captured
//!
//! A captured stage writes to a pipe, and every stage but the last **already** writes to a pipe:
//! standing between them costs nothing, changes no descriptor's nature and is invisible. The last
//! stage writes to the terminal, and interposing there would turn `isatty` false — colours off,
//! pagers not paging, progress bars silent — which is the whole reason `keep` exists as an opt-in
//! rather than a default. So the last stage runs exactly as it always has.
//!
//! That is also why a coordinate reaching *back past this pipeline* is not here yet: it needs the
//! previous command's output, and the previous command was somebody's last stage.
//!
//! # A stage is fed from a file, not a pipe
//!
//! The captured text goes into a temporary file which becomes the next stage's stdin. A pipe would
//! deadlock: the parent cannot write a megabyte into one while the child it is writing to has not
//! started reading, and the child cannot start until the parent stops writing. `tempfile` is a
//! dependency of this crate for the same reason in `direnv`, where the comment says so.

use super::{eval_command, pipeline_status, status_of, wait_for_status};
use crate::env::Environment;
use crate::env::builtins::run_exit_trap;
use crate::exec::compound::flush_stdout;
use crate::exec::streams::{self, Streams};
use nix::unistd::{ForkResult, close, dup2, fork, pipe};
use oslo_base::ast::{Command, Pipeline};
use oslo_base::error::{Result, ShellError};
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, IntoRawFd};

/// Whether any stage of this pipeline addresses a stream.
///
/// The gate. Answering `false` is the common case and must stay cheap.
pub(super) fn uses_coordinates(pipeline: &Pipeline) -> bool {
    pipeline
        .commands
        .iter()
        .any(streams::command_uses_coordinates)
}

/// Run the stages one at a time, threading each one's output into the next.
pub(super) fn run(env: &mut Environment, pipeline: &Pipeline) -> Result<i32> {
    let mut streams = Streams::for_this_pipeline();

    // **One command is not a pipeline, and must not become one.** `super::run_byte_stages` runs a
    // lone command with `eval_command` in *this* shell; forking it here instead made every builtin
    // that changes the shell change a child that then exits — `declare -a b="(y{1,2})"` left `b`
    // unset, because the word merely *looked* like it held a coordinate and that was enough to
    // take this path. There is no upstream stage to wait for either, so there is nothing the
    // sequential machinery below would do for it.
    if let [only] = pipeline.commands.as_slice() {
        let command = rewritten(only, &streams);
        let status = eval_command(env, &command)?;
        env.set_pipeline_status(vec![status]);
        return Ok(status);
    }

    let mut statuses = Vec::with_capacity(pipeline.commands.len());
    let mut upstream: Option<String> = None;

    for (at, command) in pipeline.commands.iter().enumerate() {
        let last = at + 1 == pipeline.commands.len();
        let command = rewritten(command, &streams);

        // The last stage is never captured — its stdout is the terminal's, exactly as it always
        // was. See the module docs for why that is not a compromise.
        let (status, kept) = run_stage(env, &command, upstream.as_deref(), !last)?;
        statuses.push(status);
        if last {
            break;
        }
        // Both halves of the stage, pushed together so `{0}` and `{%0}` name the same one. The
        // words are taken from the *rewritten* command, so a stage that itself read a coordinate
        // reports what it actually ran rather than what was typed.
        streams.push_command(words_of(&command));
        streams.push_stage(kept.clone());
        upstream = Some(kept);
    }

    // The same rule the concurrent path uses, and taken from the same place: with `pipefail` the
    // status is the rightmost stage that *failed*, not the rightmost stage. Computing it here
    // instead was a silent lie — `set -o pipefail; false | echo {0:0}` reported 0.
    let status = pipeline_status(env, &statuses);
    env.set_pipeline_status(statuses);
    Ok(status)
}

/// A copy of the command with its coordinates replaced.
///
/// Cloned rather than rewritten in place: the pipeline belongs to the syntax tree, which a loop may
/// run again, and a stage that rewrote itself would answer the second time around with the first
/// time's text.
fn rewritten(command: &Command, streams: &Streams) -> Command {
    let mut command = command.clone();
    streams::rewrite_command(&mut command, streams);
    command
}

/// A stage as the words it was written with, for `{%…}` to address.
///
/// A simple command gives its words one by one, which is the case the feature is for and the only
/// one where a word dimension means anything. Anything else — a group, a loop — is one value: its
/// rendered text, so `{%0}` still answers with something true and `{%0:1}` reads nothing rather
/// than slicing a construct into pieces that were never arguments.
///
/// The rendering is [`super::describe`], which the job table already uses to label a job. It is
/// deliberately approximate about quoting, and that is the right trade here too: `{%0:1}` naming
/// the argument as typed is the whole ask, and reproducing the grammar exactly would mean a second
/// unrunnable copy of it.
fn words_of(command: &Command) -> Vec<String> {
    match command {
        Command::Simple(simple) => simple
            .words
            .iter()
            .map(super::describe::describe_word)
            .collect(),
        other => vec![super::describe::describe_command(other)],
    }
}

/// Run one stage: feed it `input`, and keep what it prints when `keep` is set.
///
/// Answers the status and, when kept, the text.
fn run_stage(
    env: &mut Environment,
    command: &Command,
    input: Option<&str>,
    keep: bool,
) -> Result<(i32, String)> {
    let stdin = input.map(as_a_file).transpose()?;
    let capture = keep
        .then(pipe)
        .transpose()
        .map_err(|e| ShellError::ExecutionError(format!("Pipe failed: {e}")))?;
    run_in_child(env, command, stdin.as_ref(), capture)
}

/// The captured text as a file positioned at its start, ready to be a child's stdin.
fn as_a_file(text: &str) -> Result<std::fs::File> {
    let mut file = tempfile::tempfile()
        .map_err(|e| ShellError::ExecutionError(format!("scratch file failed: {e}")))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
        .map_err(|e| ShellError::ExecutionError(format!("scratch file failed: {e}")))?;
    Ok(file)
}

/// Fork, rearrange the child's descriptors, run the command in it.
fn run_in_child(
    env: &mut Environment,
    command: &Command,
    stdin: Option<&std::fs::File>,
    capture: Option<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)>,
) -> Result<(i32, String)> {
    // Anything already buffered belongs to the parent, not to what this stage prints.
    flush_stdout();
    let stdin_fd = stdin.map(|file| file.as_raw_fd());

    // Safety: the child only rearranges descriptors it owns and never returns to this function.
    match oslo_base::forksafe::quiesced(|| unsafe { fork() }) {
        Ok(ForkResult::Child) => {
            crate::exec::job::reset_signals_for_child();
            if let Some(fd) = stdin_fd {
                let _ = dup2(fd, 0);
            }
            if let Some((reader, writer)) = capture {
                let _ = close(reader.into_raw_fd());
                let _ = dup2(writer.as_raw_fd(), 1);
                let _ = close(writer.into_raw_fd());
            }
            env.enter_subshell();
            let status = status_of(eval_command(env, command));
            // Without this the capture would close on an unflushed partial line.
            flush_stdout();
            std::process::exit(run_exit_trap(env, status));
        }
        Ok(ForkResult::Parent { child }) => {
            let text = match capture {
                Some((reader, writer)) => {
                    let _ = close(writer.into_raw_fd());
                    let out = read_bounded(std::fs::File::from(reader));
                    String::from_utf8_lossy(&out).into_owned()
                }
                None => String::new(),
            };
            Ok((wait_for_status(child), text))
        }
        Err(e) => Err(ShellError::ExecutionError(format!("Fork failed: {e}"))),
    }
}

/// Read at most [`streams::STREAM_MAX`] bytes, then let go.
///
/// **Bounded during the read, not after it.** Reading to EOF and truncating afterwards is the same
/// answer for a file and a catastrophe for a tap that never closes: `yes | echo {0:0}` hung for
/// ever, and so did `yes | head -3 | echo {0:0}` — capturing the first stage to EOF defeats `head`'s
/// early exit, because the thing draining `yes` is no longer `head`.
///
/// Dropping the reader at the cap closes it, and the next write the producer attempts kills it with
/// `SIGPIPE` — which is exactly what `head` does to `yes` in an ordinary pipeline. So a stage that
/// produces without end is stopped by the mechanism that always stopped it.
fn read_bounded(mut file: std::fs::File) -> Vec<u8> {
    use std::io::Read;
    let mut out = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    while out.len() < streams::STREAM_MAX {
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            // **A signal is not the end of the stream.** oslo's handlers carry no `SA_RESTART`
            // (`term/resize.rs`), so an ordinary window resize interrupts this read — and folding
            // that into the `Ok(0)` arm silently truncated the capture. `cat hosts.txt | ssh
            // {0:0} uptime` then ran against no host at all, and reported success.
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    out.truncate(streams::STREAM_MAX);
    // A shell word is a C string and cannot carry a NUL, so a coordinate reading one would build an
    // argv that dies at exec. Dropped the way command substitution drops them.
    out.retain(|&b| b != 0);
    // Dropped here, deliberately: closing the read end is what stops an endless producer.
    drop(file);
    out
}
