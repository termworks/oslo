//! `oslo.ui.match` — scoring what was typed against a candidate.
//!
//! ```lua
//! oslo.ui.match("git checkout", "gco")           --> 142
//! oslo.ui.match_at("echo", "ec")                 --> { 1, 2 }
//! oslo.ui.rank(names, "gco", { limit = 10 })     --> { {text=…, score=…}, … }
//! ```
//!
//! Pure functions of two strings. A completion provider's `answer` runs inside the line editor,
//! where `oslo.run` and everything else holding the shell refuses — so a scorer that needs nothing
//! but its arguments is the only kind usable there. The alternative a config reaches for is a Lua
//! `string.find`, which ranks nothing: `max_items` then truncates whatever order the loop happened
//! to build.
//!
//! The scoring is the shell's own, so a Lua provider and the built-in finder agree about what `gco`
//! means.

use super::super::util::{list, ok, opt_text, put, record, text};
use oslo_base::value::{LuaError, Table, Value};
use oslo_ui::matching::{Fuzzy, fuzzy_score, positions};

/// A string value, or `None` for anything else.
fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// The preset an optional argument names, defaulting to `smart`.
fn mode(args: &[Value], n: usize, function: &str) -> Result<Fuzzy, LuaError> {
    match opt_text(args, n, function)? {
        None => Ok(Fuzzy::Smart),
        Some(name) => Fuzzy::parse(&name).ok_or_else(|| {
            LuaError::new(format!(
                "{function}: '{name}' is not off, tight, smart or loose"
            ))
        }),
    }
}

pub fn install(ui: &mut Table) {
    scoring(ui);
    ranking(ui);
}

fn scoring(ui: &mut Table) {
    // oslo.ui.match(candidate, typed, mode?) -> score, or nil when it does not match.
    //
    // Higher is better and the numbers are comparable only against each other; nothing is promised
    // about the scale, which is nucleo's and changes when it is tuned.
    put(ui, "match", |_, args| {
        let candidate = text(&args, 1, "oslo.ui.match")?;
        let typed = text(&args, 2, "oslo.ui.match")?;
        let fuzzy = mode(&args, 3, "oslo.ui.match")?;
        Ok(vec![
            fuzzy_score(&candidate, &typed, fuzzy)
                .map_or(Value::Nil, |score| Value::int(score as i64)),
        ])
    });

    // oslo.ui.match_at(candidate, typed) -> the byte offsets that matched.
    //
    // **1-based, because the caller's next move is `string.sub`.** `positions` counts from zero
    // like the rest of Rust, and handing that straight to Lua would mark one cell to the left.
    put(ui, "match_at", |_, args| {
        let candidate = text(&args, 1, "oslo.ui.match_at")?;
        let typed = text(&args, 2, "oslo.ui.match_at")?;
        ok(list(
            positions(&candidate, &typed)
                .into_iter()
                .map(|at| Value::int(at as i64 + 1)),
        ))
    });
}

fn ranking(ui: &mut Table) {
    // oslo.ui.rank(candidates, typed, opts?) -> { {text=, score=}, … }, best first.
    //
    // The non-matches are dropped rather than scored zero, so `#result` is the count worth showing
    // and `limit` cuts the list after the order exists rather than before.
    put(ui, "rank", |_, args| {
        let Some(Value::Table(candidates)) = args.first() else {
            return Err(LuaError::new(format!(
                "oslo.ui.rank: argument #1 must be a table, got {}",
                args.first().map_or("no value", Value::type_name)
            )));
        };
        let typed = text(&args, 2, "oslo.ui.rank")?;
        let (fuzzy, limit) = options(args.get(2))?;

        let mut scored: Vec<(String, i32)> = candidates
            .borrow()
            .sequence()
            .iter()
            .filter_map(as_text)
            .filter_map(|entry| fuzzy_score(&entry, &typed, fuzzy).map(|score| (entry, score)))
            .collect();

        // Descending by score, and ties broken by the order given: a provider that already sorted
        // by recency keeps that inside each score band rather than having it shuffled.
        scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        if let Some(limit) = limit {
            scored.truncate(limit);
        }
        ok(list(scored.into_iter().map(|(entry, score)| {
            record(vec![
                ("text", Value::str(entry)),
                ("score", Value::int(score as i64)),
            ])
        })))
    });
}

/// `{ mode = "smart", limit = n }`, both optional.
fn options(value: Option<&Value>) -> Result<(Fuzzy, Option<usize>), LuaError> {
    let Some(Value::Table(opts)) = value else {
        return Ok((Fuzzy::Smart, None));
    };
    let opts = opts.borrow();
    let fuzzy = match opts.get_str("mode") {
        Value::Nil => Fuzzy::Smart,
        named => {
            let named = as_text(&named).unwrap_or_default();
            Fuzzy::parse(&named).ok_or_else(|| {
                LuaError::new(format!(
                    "oslo.ui.rank: '{named}' is not off, tight, smart or loose"
                ))
            })?
        }
    };
    let limit = opts
        .get_str("limit")
        .as_number()
        .and_then(|n| n.as_int())
        .and_then(|n| (n > 0).then_some(n as usize));
    Ok((fuzzy, limit))
}

#[cfg(test)]
#[path = "find/tests.rs"]
mod tests;
