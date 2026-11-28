//! Where the structured half stops and the byte half takes over.
//!
//! ```text
//! ls | first 2 | cat
//! └──── rows ────┘   └─ bytes: rendered, and handed over as this stage's standard input
//! ```
//!
//! # The mirror of the byte prefix, and it was the missing half
//!
//! [`super::run`] already lets an external command *lead*: `kubectl get pods -o json | from json |
//! where …` runs the prefix on the byte path and gives its output to the first tool. The other
//! direction had no answer at all. A tool followed by anything that is not a tool fell back for the
//! **whole** pipeline, and on that path the verbs are not commands:
//!
//! ```text
//! $ ls | first 2 | cat
//! oslo: first: command not found
//! $ echo $?
//! 0
//! ```
//!
//! Empty output, status 0 — and the structured `ls` had already run before the fallback re-ran the
//! line from the start, so a tool with a side effect performed it twice. That is the failure
//! `docs/known-gaps.md` opens by saying oslo does not have, so it does not get to stay.
//!
//! # Rendered as transport, never as a table
//!
//! What crosses is [`crate::data::render_transport`] — plain, complete, untruncated. The drawn
//! table with its box-drawing characters is for a person looking at a terminal; putting it on
//! another program's standard input is the exact failure the structured design exists to prevent.
//!
//! # A tool that prints, not just one that produces rows
//!
//! `to json` writes with `println!` and hands back nothing — it is a *bytes* producer, and that is
//! how `df | to json` puts JSON on the terminal. So carrying only the rows across the seam left
//! `ps | first 3 | to json | jq .` giving jq an empty input while the JSON went straight to the
//! shell's own stdout: the filter silently did nothing.
//!
//! Whether a byte suffix exists is knowable *before* any stage runs — it is the first stage that is
//! not a tool — so when there is one, the tool half's stdout is pointed at a scratch file for its
//! duration. What crosses is then everything the tools said, printed or produced.
//!
//! # A file, not a pipe
//!
//! The rows are written to a temporary file which becomes the suffix's standard input. A pipe would
//! deadlock: this process cannot write a megabyte of rendered rows into one before the child it is
//! writing to has started reading, and the child cannot start until the write returns. The same
//! reasoning, and the same `tempfile`, as `exec::pipeline::coordinates`.

use super::super::pipeline_status;
use crate::env::Environment;
use oslo_base::ast::{Command, Pipeline};
use oslo_base::error::{Result, ShellError};

/// Whether this half can run the stage at all.
///
/// A registered tool and nothing else. Anything the planner did not think was a tool — an external
/// command, a builtin, a compound — is where the bytes begin.
pub(super) fn runnable_here(env: &Environment, command: &Command) -> bool {
    match command {
        Command::Simple(simple) => super::simple_command_name(env, simple)
            .is_some_and(|name| crate::data::tool::lookup(&name).is_some()),
        _ => false,
    }
}

/// Where the byte suffix begins, if there is one.
///
/// Asked before anything runs, because the answer decides whether the tool half's own output has to
/// be captured. `None` when every stage from `start` is a tool, or when the very first one is not —
/// nothing has run then, and the byte path can still have the whole pipeline as it always did.
pub(super) fn byte_suffix_at(
    env: &Environment,
    pipeline: &Pipeline,
    start: usize,
) -> Option<usize> {
    let at = pipeline
        .commands
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, command)| !runnable_here(env, command))
        .map(|(i, _)| i)?;
    (at > start).then_some(at)
}

/// The tool half's stdout, pointed at a scratch file so a printing tool's output crosses the seam.
pub(super) struct Printed {
    saved: std::os::fd::RawFd,
    file: std::fs::File,
    /// Whether stdout has already been put back, so [`Drop`] does not close `saved` twice.
    restored: bool,
}

/// **Stdout goes back even when the stage that borrowed it did not finish.**
///
/// `finish` was the only route home, so any `?` between `start` and it — an expansion error in a
/// later stage, either `fallback` arm — left this process's stdout dup2'd onto a *deleted* scratch
/// file for the rest of the session. `ls | first ${nope?boom} | cat` did it: the prompt still drew,
/// and nothing said after it ever reached the terminal again, builtin or external alike.
///
/// This runs on the error paths, which is where the bug was. It does not run on a panic —
/// `panic = "abort"` skips destructors — and that is one more thing riding on the abort decision.
impl Drop for Printed {
    fn drop(&mut self) {
        self.restore();
    }
}

impl Printed {
    /// Point this process's stdout at a scratch file.
    pub(super) fn start() -> Result<Printed> {
        use std::os::fd::AsRawFd;
        let file = tempfile::tempfile()
            .map_err(|e| ShellError::ExecutionError(format!("scratch file failed: {e}")))?;
        let out = std::io::stdout().as_raw_fd();
        // The shell's own save policy: out of the script's 3..9 range, and `FD_CLOEXEC` so the
        // copy is not inherited by every program the pipeline runs. See `redirect::save_fd`.
        let saved = crate::exec::redirect::save_fd(out)
            .ok_or_else(|| ShellError::ExecutionError("dup: cannot save stdout".to_string()))?;
        let _ = nix::unistd::dup2(file.as_raw_fd(), out);
        Ok(Printed {
            saved,
            file,
            restored: false,
        })
    }

    /// Put this process's stdout back where it was. Idempotent, because both `finish` and `Drop`
    /// call it and closing `saved` twice would close whatever descriptor took its number.
    fn restore(&mut self) {
        use std::os::fd::AsRawFd;
        if self.restored {
            return;
        }
        self.restored = true;
        // The flush is the whole of the ordering: `println!` is line-buffered onto a descriptor
        // this is about to take away, and anything still in that buffer would be written to the
        // *restored* stdout afterwards — the tool half's output appearing after the suffix's, on
        // the terminal rather than in the pipe.
        crate::exec::compound::flush_stdout();
        let out = std::io::stdout().as_raw_fd();
        let _ = nix::unistd::dup2(self.saved, out);
        let _ = nix::unistd::close(self.saved);
    }

    /// Put stdout back and answer what was written to it.
    pub(super) fn finish(mut self) -> String {
        use std::io::{Read, Seek, SeekFrom};
        self.restore();

        let mut text = String::new();
        let _ = self.file.seek(SeekFrom::Start(0));
        let mut buffer = Vec::new();
        if self.file.read_to_end(&mut buffer).is_ok() {
            text = String::from_utf8_lossy(&buffer).into_owned();
        }
        text
    }
}

/// Render what the tools produced and run the rest of the pipeline on it.
///
/// `at` is the first stage this half could not run; everything from there is the byte suffix.
/// `printed` is whatever the tools wrote to stdout on the way.
pub(super) fn hand_over(
    env: &mut Environment,
    pipeline: &Pipeline,
    at: usize,
    rows: Option<Vec<crate::data::Record>>,
    printed: Option<Printed>,
    statuses: &mut Vec<i32>,
    fallback: fn(&mut Environment, &Pipeline) -> Result<i32>,
) -> Result<i32> {
    // What a printing tool said comes first: it was written before the rows were finished, and a
    // pipeline that both prints and produces should read in the order it happened.
    let mut text = printed.map(Printed::finish).unwrap_or_default();
    if let Some(rows) = rows
        && !rows.is_empty()
    {
        let rendered = crate::data::render_transport(&crate::data::Val::table(rows));
        if !rendered.is_empty() {
            // The newline a command's output ends with, which every reader of a line-oriented
            // stream expects and `render_transport` does not add.
            text.push_str(&rendered);
            text.push('\n');
        }
    }

    let suffix = Pipeline {
        commands: pipeline.commands[at..].to_vec(),
        negated: false,
        timed: false,
    };
    let status = feed(env, &suffix, &text, fallback)?;

    // One status per stage, the structured ones followed by the byte ones, so `PIPESTATUS` still
    // describes the pipeline that was written rather than the halves it was run in.
    statuses.extend(env.pipeline_status().iter().copied());
    let status = match statuses.len() > at {
        true => pipeline_status(env, statuses),
        // The byte path reported nothing to append; its own answer is all there is.
        false => status,
    };
    env.set_pipeline_status(statuses.clone());
    Ok(status)
}

/// Run `suffix` on the byte path with `text` as its standard input.
///
/// Standard input is put back however this returns, including on the error path — a shell left
/// reading from a temporary file would be a far worse failure than the one being reported.
fn feed(
    env: &mut Environment,
    suffix: &Pipeline,
    text: &str,
    fallback: fn(&mut Environment, &Pipeline) -> Result<i32>,
) -> Result<i32> {
    use std::io::{Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;

    let mut file = tempfile::tempfile()
        .map_err(|e| ShellError::ExecutionError(format!("scratch file failed: {e}")))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
        .map_err(|e| ShellError::ExecutionError(format!("scratch file failed: {e}")))?;

    let stdin = std::io::stdin().as_raw_fd();
    // Likewise: without `FD_CLOEXEC` this copy of the shell's stdin was readable by any program
    // the byte half of the pipeline went on to run.
    let saved = crate::exec::redirect::save_fd(stdin)
        .ok_or_else(|| ShellError::ExecutionError("dup: cannot save stdin".to_string()))?;
    let _ = nix::unistd::dup2(file.as_raw_fd(), stdin);

    let status = fallback(env, suffix);

    // SAFETY: `saved` is a descriptor this function created with `dup` and has not closed.
    let restore = unsafe { std::os::fd::BorrowedFd::borrow_raw(saved) };
    let _ = nix::unistd::dup2(restore.as_raw_fd(), stdin);
    let _ = nix::unistd::close(saved);
    status
}
