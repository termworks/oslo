//! What a stage's columns will be, worked out **before it runs**.
//!
//! ```text
//! ls | cols nmae
//!      ────────
//!      ls answers name size size_human is_dir modified mode, exactly
//!      `nmae` is not among them ──▶ refused, and `ls` never runs
//! ```
//!
//! # Finishing oslo's own idea rather than importing nushell's
//!
//! `data::plan` decides which channel every edge carries before any stage starts, and everything
//! good about the structured half follows from that. But the declaration it reads stops at
//! `Shape` — *takes rows, gives rows* — and says nothing about **which
//! columns**. So a mistyped column name was caught by `tools::unknown_column` scanning the rows that
//! were actually produced, which is to say *after the producer ran*. For `ls` that costs nothing;
//! for a tool a config registered it means a side effect has already happened.
//!
//! This is the same question one level down: the pipe already knows what shape crosses an edge, and
//! now it knows what is in it.
//!
//! # `Unknown` is an answer, not a failure
//!
//! Most streams are knowable and some are not. `from json` learns its columns from the document,
//! `map` from whatever Lua returned, `headers` from row one. Those are `Columns::Unknown`, and a
//! pipeline is `Unknown` from the first such stage onwards.
//!
//! **Nothing may be refused on an `Unknown`.** That is the rule the whole design rests on: a
//! plan-time check that guesses wrong turns a working pipeline into an error, which is strictly
//! worse than the runtime check it replaces. `unknown_column` still exists and still catches
//! everything this cannot see.
//!
//! # How much is actually knowable
//!
//! More than it looks. Twenty-five of the forty verbs answer exactly, and the head of almost every
//! real pipeline is one of them — the three producers, plus `parse`, whose columns are sitting in a
//! literal operand:
//!
//! ```text
//! cat /etc/passwd | parse '{user}:{x}:{uid}:{rest}' | cols user uid
//!                         └── the columns are right here ──┘
//! ```

use super::path::Path;

/// What is known about a stream's columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Columns {
    /// Exactly these, in this order.
    Known(Vec<String>),
    /// Not knowable until the data arrives.
    Unknown,
}

impl Columns {
    /// A known set, from names.
    pub fn known<S: AsRef<str>>(names: impl IntoIterator<Item = S>) -> Columns {
        Columns::Known(names.into_iter().map(|n| n.as_ref().to_string()).collect())
    }

    /// The names, or `None` when nothing is known.
    pub fn names(&self) -> Option<&[String]> {
        match self {
            Columns::Known(names) => Some(names),
            Columns::Unknown => None,
        }
    }

    /// Whether `wanted` could name something in this stream.
    ///
    /// **Generous on purpose**, because the cost of a false refusal is a working pipeline that stops
    /// working and the cost of a false acceptance is only the runtime check doing its job:
    ///
    /// * `Unknown` accepts everything — nothing is known, so nothing can be refused.
    /// * A path is judged by its **first step only**: `metadata.name` is accepted when the stream
    ///   has `metadata`, because whether that cell is a record is a question about data.
    /// * An optional first step (`a?.b`) accepts, since it said the absence was expected.
    /// * An exact column of that literal name always accepts, which is how a column genuinely
    ///   called `a.b` keeps working.
    pub fn accepts(&self, wanted: &str) -> bool {
        let Columns::Known(names) = self else {
            return true;
        };
        if names.iter().any(|name| name == wanted) {
            return true;
        }
        let path = Path::parse(wanted);
        match path.first_step() {
            Some((step, optional)) => optional || names.iter().any(|name| name == step),
            None => true,
        }
    }
}

/// What `name` answers, given its words and what reached it.
///
/// `argv` is the whole command including its name, as `tools::run_tool` takes it, so operands start
/// at one.
pub fn through(name: &str, argv: &[String], input: &Columns) -> Columns {
    let operand = |at: usize| argv.get(at).map(String::as_str);
    // **A tool a config registered, which said what it produces.** Asked first, so a config can
    // declare columns for a name the shell has never heard of — and, since `run_tool` looks in the
    // config's table before its own, for one it has.
    if let Some(columns) = super::tool::columns_of(name) {
        return Columns::Known(columns);
    }
    match name {
        // Producers know their own, and say so beside the code that fills them.
        "df" => Columns::known(super::tools::df::COLUMNS),
        "ps" => Columns::known(super::tools::system::PS_COLUMNS),
        "ls" => Columns::known(super::tools::system::LS_COLUMNS),
        "history" => Columns::known(super::tools::past::COLUMNS),

        // Bytes into rows.
        "lines" => Columns::known(["line"]),
        "parse" => parsed(argv),
        // The document or the header decides, and neither is here yet.
        "from" | "detect-columns" => Columns::Unknown,

        // The verbs that keep the shape they were given.
        "where" | "sort-by" | "reverse" | "first" | "final" | "skip" | "every" | "distinct"
        | "compact" => input.clone(),

        // The verbs that name their own columns.
        "cols" => Columns::Known(argv[1..].to_vec()),
        "get" => match operand(1) {
            Some(column) => Columns::known([column]),
            None => Columns::Unknown,
        },
        "length" => Columns::known(["length"]),
        "reduce" => Columns::known(["reduced"]),
        "describe" => Columns::known(["column", "type", "filled", "rows"]),
        "stats" => Columns::known(["field", "count", "min", "max", "sum", "mean"]),
        "group-by" => match operand(1) {
            Some(column) => Columns::known([column, "count", "rows"]),
            None => Columns::Unknown,
        },
        "histogram" => match operand(1) {
            Some(column) => Columns::known([column, "count", "bar"]),
            None => Columns::Unknown,
        },
        // `count` notices what it was handed, exactly as the verb does: a grouped stream keeps its
        // own columns and loses the rows it was carrying.
        "count" => match input.names() {
            Some(names) if names.iter().any(|n| n == "count") => {
                Columns::Known(names.iter().filter(|n| *n != "rows").cloned().collect())
            }
            _ => Columns::known(["count"]),
        },

        // The verbs that edit the set they were given.
        // **A nested write changes nothing about the top-level set.** `insert a.b` puts a field
        // inside the column `a`, and `reject a.b` takes one out of it — neither adds nor removes a
        // column of the stream, so a path of more than one step leaves the set exactly as it was.
        // Saying otherwise would invent a column called `a.b` that the rows do not have, and the
        // planner would then refuse the very next stage for naming what it just promised.
        "reject" => match input.names() {
            Some(names) => Columns::Known(
                names
                    .iter()
                    .filter(|name| !argv[1..].iter().any(|wanted| wanted == *name))
                    .cloned()
                    .collect(),
            ),
            None => Columns::Unknown,
        },
        "rename" => match (input.names(), operand(1), operand(2)) {
            // In place, because a record's order decides what the drawn table shows.
            (Some(names), Some(from), Some(to)) => Columns::Known(
                names
                    .iter()
                    .map(|name| match name == from {
                        true => to.to_string(),
                        false => name.clone(),
                    })
                    .collect(),
            ),
            _ => Columns::Unknown,
        },
        // **A nested write adds no column.** `insert state.tag …` puts a field inside the column
        // `state`; the stream still has exactly the columns it had. Claiming otherwise would
        // promise a column called `state.tag` that no row carries, and the planner would then
        // refuse the next stage for naming what this one had just announced.
        "insert" | "update" | "upsert" | "default" => match (input.names(), operand(1)) {
            (Some(names), Some(column)) => {
                let mut out = names.to_vec();
                let writes_a_column =
                    names.iter().any(|name| name == column) || !column.contains('.');
                if writes_a_column && !out.iter().any(|name| name == column) {
                    out.push(column.to_string());
                }
                Columns::Known(out)
            }
            _ => Columns::Unknown,
        },
        "enumerate" => match input.names() {
            // The index leads, as the verb puts it.
            Some(names) => Columns::Known(
                std::iter::once("index".to_string())
                    .chain(names.iter().cloned())
                    .collect(),
            ),
            None => Columns::Unknown,
        },

        // The Lua decides, the nesting decides, row one decides, or the other stream decides.
        "map" | "flatten" | "headers" | "lookup" | "append" | "merge" => Columns::Unknown,

        // `each` produces no rows and `to` produces bytes; neither has a column set to pass on.
        _ => Columns::Unknown,
    }
}

/// The columns a `parse` pattern names, read out of the literal operand.
///
/// Both spellings say what they produce, which is what makes `parse` the one bridge into structure
/// whose output is knowable before a byte of input arrives.
fn parsed(argv: &[String]) -> Columns {
    let by_regex = argv.get(1).is_some_and(|word| word == "--regex");
    let Some(pattern) = argv.get(if by_regex { 2 } else { 1 }) else {
        return Columns::Unknown;
    };
    match by_regex {
        // `(?<name>…)`, which is what `bridge::parse_regex` captures by.
        true => match regex::Regex::new(pattern) {
            Ok(compiled) => {
                let names: Vec<&str> = compiled.capture_names().flatten().collect();
                match names.is_empty() {
                    true => Columns::Unknown,
                    false => Columns::known(names),
                }
            }
            // It will be refused when it runs; saying nothing here is not this function's job.
            Err(_) => Columns::Unknown,
        },
        false => {
            let holes = holes_of(pattern);
            match holes.is_empty() {
                true => Columns::Unknown,
                false => Columns::Known(holes),
            }
        }
    }
}

/// The `{name}` holes of a pattern, in order.
fn holes_of(pattern: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '{' {
            continue;
        }
        let mut name = String::new();
        for c in chars.by_ref() {
            if c == '}' {
                break;
            }
            name.push(c);
        }
        if !name.is_empty() {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
#[path = "columns/tests.rs"]
mod tests;
