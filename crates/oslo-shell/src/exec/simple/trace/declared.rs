//! `set -x` for a declaration builtin given `name=(…)` operands, as bash traces it: each list on
//! its own line as it is assigned, `+ a=('1' '2')`, then the command with the operand reduced to
//! its name, `+ declare -a a`.

use super::{trace_line, tracing};
use crate::env::Environment;
use crate::env::builtins::arrays::LiteralItem;
use std::cell::RefCell;

thread_local! {
    /// The command's own trace lines, held back until its lists have been traced.
    static DEFERRED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The words with each `name=(…)` operand cut to its name, when `shapes` marks one; `None` when
/// the command traces as it is.
pub(super) fn reduced(words: &[String], shapes: &[bool]) -> Option<Vec<String>> {
    if !shapes.iter().any(|&compound| compound) {
        return None;
    }
    let reduce = |(at, word): (usize, &String)| match shapes.get(at) {
        Some(true) => {
            let name = word.split_once('=').map_or(word.as_str(), |(n, _)| n);
            name.strip_suffix('+').unwrap_or(name).to_string()
        }
        _ => word.clone(),
    };
    Some(words.iter().enumerate().map(reduce).collect())
}

/// Hold `lines` back until the builtin has traced its lists.
pub(super) fn defer(lines: Vec<String>) {
    DEFERRED.with(|deferred| *deferred.borrow_mut() = lines);
}

/// Write the lines [`defer`] held back, once the builtin is done.
pub(crate) fn flush(env: &mut Environment) {
    let lines = DEFERRED.with(|deferred| std::mem::take(&mut *deferred.borrow_mut()));
    for line in lines {
        trace_line(env, &line);
    }
}

/// `+ name=('…' …)`: one list a declaration builtin is assigning.
pub(crate) fn trace_list(env: &mut Environment, name: &str, append: bool, items: &[LiteralItem]) {
    if !tracing(env) {
        return;
    }
    let quoted = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let elements: Vec<String> = items
        .iter()
        .map(|item| match item {
            LiteralItem::Keyed { key, value, append } => {
                let op = if *append { "+=" } else { "=" };
                format!("[{}]{op}{}", quoted(key), quoted(value))
            }
            LiteralItem::Plain(value) => quoted(value),
        })
        .collect();
    let op = if append { "+=" } else { "=" };
    trace_line(env, &format!("{name}{op}({})", elements.join(" ")));
}
