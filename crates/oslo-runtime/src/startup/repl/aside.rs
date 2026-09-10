//! The pieces of the loop that are not the loop: what it says to the terminal, and what it makes
//! of a Lua line.
//!
//! Split out when `repl.rs` crossed the 600-line limit. Grouped by subject rather than by the
//! order the loop happens to call them in — the terminal-facing helpers together, the Lua-line
//! evaluator with them because it is the other thing the loop delegates whole rather than inlines.

use crate::lua::LuaEngine;
use oslo_base::error::ShellError;
use oslo_shell::Environment;
use std::sync::{Arc, Mutex};

pub(crate) fn announce(sequence: &str) {
    if sequence.is_empty() {
        return;
    }
    print!("{sequence}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

/// The title to show while `text` runs: the command, shortened to its first word and trimmed.
///
/// The first word rather than the whole line, because a title bar is narrow and `cargo` in a tab
/// is more use than the first forty characters of a `for` loop.
pub(crate) fn title_for_command(text: &str) -> String {
    let first = text.split_whitespace().next().unwrap_or("");
    if first.is_empty() {
        current_directory()
    } else {
        format!("{first} — {}", oslo_ui::prompt::tilde(&current_directory()))
    }
}

/// Where the shell is now, for the `cd` hook to compare against.
pub(crate) fn cwd() -> String {
    current_directory()
}

pub(crate) fn current_directory() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Run one Lua line typed at the prompt.
///
/// A chunk that merely printed something has not run a command, so `$?` stays where it was;
/// `oslo.proc.exit(n)` is how a script chooses a status, and it ends the shell rather than setting one.
pub(crate) fn run_lua_line(
    lua: &LuaEngine,
    text: &str,
    last_status: i32,
) -> Result<i32, ShellError> {
    if let Some(status) = leaving(text) {
        return Err(ShellError::Exit(status));
    }
    match lua.eval_script(&as_a_prompt_line(lua, text)) {
        Ok(()) => Ok(last_status),
        Err(ShellError::Lua(e)) if e.exit.is_some() => Err(ShellError::Exit(e.exit.unwrap_or(0))),
        Err(e) => Err(e),
    }
}

/// `exit`, and `exit 3`, typed at a Lua prompt.
///
/// **Because it is a shell prompt before it is a Lua one.** `exit` is a word every shell answers
/// and Lua has no meaning for at all: as an expression it is an unset global, so the prompt read
/// it, printed `nil`, and stayed open. There is no way to leave a Lua prompt that a shell user
/// would guess — `os.exit()` is the Lua answer and nobody arrives knowing it.
///
/// Only the bare word, so a script that has a variable called `exit` is untouched by this: `exit`
/// alone leaves, `exit(0)` and `print(exit)` are Lua.
fn leaving(text: &str) -> Option<i32> {
    let mut words = text.split_whitespace();
    let first = words.next()?;
    if !matches!(first, "exit" | "quit") {
        return None;
    }
    match words.next() {
        None => Some(0),
        Some(code) if words.next().is_none() => code.parse().ok(),
        Some(_) => None,
    }
}

/// The source to actually run for one typed line.
///
/// **A prompt is mostly expressions, and Lua chunks are statements.** `1 + 1`, `x * 2`, `os.time`
/// and `{1,2}` are every one of them a syntax error as a chunk — "expression is not a statement" —
/// so the Lua prompt could evaluate nothing and print nothing. Every Lua REPL solves it the same
/// way and so does this: try the line as the tail of a `return` first, and if that parses, run it
/// wrapped so whatever it produced is printed.
///
/// `print` does the printing rather than Rust, because that is what makes `__tostring` work and
/// what lets a config replace `print` and have the prompt obey it — the same property the
/// reference interpreter has.
///
/// **The line goes on a line of its own** inside the wrapper. Written inline, a trailing `--`
/// comment would swallow the closing bracket that follows it.
fn as_a_prompt_line(lua: &LuaEngine, text: &str) -> String {
    // A statement stays a statement: `x = 5` is not an expression list and must not be wrapped.
    if !lua.compiles(&format!("return\n{text}\n")) {
        return text.to_string();
    }
    format!(
        "local __oslo_answer = table.pack(\n{text}\n)\n\
         if __oslo_answer.n > 0 then print(table.unpack(__oslo_answer, 1, __oslo_answer.n)) end\n"
    )
}

/// Whether a `pre-exit` handler said no.
///
/// **The one hook that can refuse something asked for directly.** `exit` and Ctrl-D are a keystroke
/// away from the command above them, and the shell they close is often the last pane of a
/// multiplexer — where the old answer was to have the multiplexer launch another shell, because a
/// shell could not decline to die. `reason` is `"exit"` or `"eof"`, so a handler can ask about the
/// accident and stay out of the way of the deliberate one.
///
/// **Only where somebody can change their mind.** A shell whose input is a file or a pipe reaches
/// end-of-file because the input genuinely ended: refusing there is not a second chance, it is a
/// loop that reads the same end-of-file for ever. Asked of stdin rather than of `$-`, because that
/// is the thing that runs out.
///
/// `false` when nothing is attached — one relaxed load — so a shell with no such hook pays nothing.
/// A handler that fails answers nothing and the shell leaves, which is the right way for this one
/// to break: a config with a mistake in it must not be able to trap you in a terminal.
pub(crate) fn exit_refused(reason: &str, status: i32) -> bool {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return false;
    }
    matches!(
        oslo_base::hooks::answer_hook_with(
            oslo_base::hooks::at::PRE_EXIT,
            vec![oslo_base::hooks::fields(&[
                ("reason", oslo_base::value::Value::str(reason)),
                ("status", oslo_base::value::Value::int(i64::from(status))),
            ])],
        ),
        Some(oslo_base::value::Value::Bool(false))
    )
}

/// `$IGNOREEOF`: how many end-of-file characters to ignore before ending the shell.
///
/// `None` means the variable is unset and Ctrl-D exits immediately, as it always has. bash's
/// documented fallback for a value that is not a number is 10.
pub(crate) fn ignore_eof_limit(env_struct: &Arc<Mutex<Environment>>) -> Option<usize> {
    let guard = env_struct.lock().unwrap_or_else(|held| held.into_inner());
    if let Some(raw) = guard.get_var("IGNOREEOF") {
        return Some(raw.trim().parse::<usize>().unwrap_or(10));
    }
    // `set -o ignoreeof` is the option spelling of the same thing, and bash treats it as
    // `IGNOREEOF=10`. It used to be accepted and ignored, so a shell that had been told not to
    // exit on Ctrl-D exited on Ctrl-D.
    guard
        .option(oslo_shell::env::options::ShellOption::IgnoreEof)
        .then_some(10)
}

thread_local! {
    /// How long the command before this prompt took, for the right prompt to mention.
    ///
    /// Thread-local rather than threaded through `read_command`: the duration is a property of the
    /// session's last command, and every caller that wants it is on the REPL's own thread.
    static LAST_DURATION: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

/// How long the last command took, if one has run.
pub(crate) fn last_command_duration() -> Option<std::time::Duration> {
    LAST_DURATION.get()
}

pub(crate) fn note_command_duration(elapsed: std::time::Duration) {
    LAST_DURATION.set(Some(elapsed));
}

/// Ask the terminal again what it can do, when the line just run was one that reset it.
///
/// **Capabilities are settled once, by a 100 ms exchange before the first prompt.** A terminal that
/// missed that window — a busy machine, an emulator still starting — kept the degraded answer for
/// the session, and `reset` could not help: the stale answer is the shell's own memory rather than
/// the terminal's state, so resetting the terminal changed nothing about what the shell believed.
///
/// Here, because this is both the moment the old answer stops being trustworthy and the moment
/// somebody is plainly trying to put things right.
pub fn renegotiate_if_reset(line: &str) {
    if crate::startup::terminal::resets_the_terminal(line) {
        crate::startup::terminal::renegotiate();
    }
}
