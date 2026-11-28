//! `set -x`: the execution trace.
//!
//! One line per traced step, on stderr, after expansion and before the step runs, in bash's
//! format: the expanded `$PS4` with its first character repeated once per level of `eval`,
//! `source`, trap or command substitution, then the words quoted the way bash quotes them.
//! Each line is written with one `write(2)`, so concurrent pipeline stages cannot tear it.

use crate::env::Environment;
use crate::env::options::ShellOption;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

mod declared;
mod quote;
#[cfg(test)]
mod tests;

pub(crate) use declared::{flush as flush_deferred, trace_list};
pub use quote::{assignment_text, list_assignment_text, quote_word};

/// How many levels of `eval`, `source`, trap or command substitution enclose the command.
static LEVEL: AtomicUsize = AtomicUsize::new(0);

/// One more trace level for as long as the value lives.
pub struct Nested(());

impl Nested {
    pub fn enter() -> Self {
        LEVEL.fetch_add(1, Ordering::Relaxed);
        Nested(())
    }
}

impl Drop for Nested {
    fn drop(&mut self) {
        LEVEL.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One more trace level for the rest of this process: a command substitution's child.
pub fn nest() {
    LEVEL.fetch_add(1, Ordering::Relaxed);
}

/// Whether a trace line should be written now.
fn tracing(env: &Environment) -> bool {
    // A vetoed command is not traced: the trace prints every expanded argument.
    env.xtrace() && !oslo_base::quiet::active()
}

/// Trace a simple command: each assignment on its own line, then the words.
///
/// `assignments` are already rendered by [`assignment_text`]. `export` and `readonly` trace each
/// `NAME=value` operand as an assignment too, as bash does.
pub fn trace_command(env: &mut Environment, assignments: &[String], words: &[String]) {
    if !tracing(env) {
        return;
    }
    for assignment in assignments {
        trace_line(env, assignment);
    }
    let Some(name) = words.first() else {
        return;
    };
    if name == "[[" {
        let body = quote::conditional_text(&words[1..]);
        trace_line(env, &body);
        return;
    }
    // A declaration builtin given `name=(…)` traces each list first; see `declared`.
    let shapes = crate::env::builtins::declare::written_shapes();
    let reduced = shapes.and_then(|shapes| declared::reduced(words, &shapes));
    let words = reduced.as_deref().unwrap_or(words);
    let body: Vec<String> = words.iter().map(|word| quote_word(word)).collect();
    let mut lines = vec![body.join(" ")];
    if matches!(name.as_str(), "export" | "readonly") {
        for operand in &words[1..] {
            if let Some((lhs, value)) = operand.split_once('=')
                && crate::env::scope::is_valid_identifier(lhs)
            {
                lines.push(assignment_text(lhs, false, value));
            }
        }
    }
    if reduced.is_some() {
        return declared::defer(lines);
    }
    for line in lines {
        trace_line(env, &line);
    }
}

/// Trace one assignment that has just been made.
pub fn trace_assignment(env: &mut Environment, text: &str) {
    if tracing(env) {
        trace_line(env, text);
    }
}

/// Trace `(( expr ))`, with the expression as it was expanded.
pub fn trace_arithmetic(env: &mut Environment, expanded: &str) {
    if tracing(env) {
        trace_line(env, &format!("(( {expanded} ))"));
    }
}

/// Trace a compound command's header — `for x in …`, `case w in`, `select x in …` — whose
/// text is printed as it was written.
pub fn trace_header(env: &mut Environment, text: &str) {
    if tracing(env) {
        trace_line(env, text);
    }
}

/// Write `$PS4` and `body` as one line.
fn trace_line(env: &mut Environment, body: &str) {
    let mut line = prefix(env);
    line.push_str(body);
    line.push('\n');
    let _ = std::io::stderr().write_all(&oslo_base::lossless::restore_bytes(line.as_bytes()));
}

/// The trace prefix: `$PS4` expanded, its first character repeated once per level.
fn prefix(env: &mut Environment) -> String {
    let Some(ps4) = env.get_var("PS4").map(str::to_string) else {
        return String::new();
    };
    let expanded = expand_ps4(env, &ps4);
    let mut chars = expanded.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let levels = 1 + LEVEL.load(Ordering::Relaxed);
    let mut out: String = std::iter::repeat_n(first, levels).collect();
    out.push_str(chars.as_str());
    out
}

/// Expand `$PS4` as a prompt string — its backslash escapes, then its expansions — with the
/// trace off so its own substitutions are not traced.
fn expand_ps4(env: &mut Environment, ps4: &str) -> String {
    if !ps4.contains(['$', '`', '\\']) {
        return ps4.to_string();
    }
    let status = env.last_status;
    env.set_option(ShellOption::XTrace, false);
    let expanded = crate::expand::prompt::expand(env, ps4);
    env.set_option(ShellOption::XTrace, true);
    env.last_status = status;
    expanded.unwrap_or_else(|_| ps4.to_string())
}
