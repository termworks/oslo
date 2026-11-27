//! The command line a hook is about to see, as data rather than as a string.
//!
//! `pre-cmd` used to hand over `text` and nothing else, so a config that wanted to know what was
//! being run had to split it itself — and splitting a shell line with `gmatch("%S+")` is wrong the
//! moment somebody types `cat 'a b'`, `echo $HOME` or `a | b`. The shell parses the line a moment
//! later anyway; this hands the config the same answer instead of making it guess.
//!
//! # The shape
//!
//! A **flat list of the simple commands, in the order they were written**, each saying how it joins
//! the one before:
//!
//! ```lua
//! oslo.on.pre_cmd(function(c)
//!   #c.commands              -- how many commands the line runs
//!   c.commands[1].argv[1]    -- the first word of the first command
//!   c.commands[2].link       -- "|", "&&", "||", ";" or "&"
//! end)
//! ```
//!
//! Flat rather than nested, because the questions people ask are flat: *how many commands*, *what
//! is the second one*, *is anything piped*. A tree of lists of pipelines of commands answers those
//! only after the caller has walked it, and every caller would walk it the same way. The nesting is
//! not lost — it is in `link`, which is exactly the part of the tree that carries meaning.
//!
//! # What a word says
//!
//! Quoting is resolved and expansion is not, because expansion has not happened yet and inventing
//! an answer would be a lie: `cat 'a b'` gives one word `a b`, and `cat $HOME` gives the word
//! `$HOME`. A config testing `argv[2]` against the filesystem gets the path when it is written
//! plainly, and something it can see is a variable when it is not.

use oslo_base::ast::types::{Command, CommandList, ListOp, Word, WordPart};
use oslo_base::value::{Table, Value};

/// How a command joins the one before it.
const FIRST: &str = "first";

/// The parsed line, as a Lua list, or `None` when it does not parse.
///
/// A line that does not parse is not an error here — the shell is about to report it far better
/// than a hook could. The field is simply absent, and a handler that looks at `c.commands` before
/// checking it gets nil, which is the Lua way of saying the same thing.
pub fn commands_of(text: &str) -> Option<Value> {
    let list = oslo_shell::syntax::parse_bash_script(text).ok()?;
    let mut out = Table::new();
    for (at, (command, link)) in walk(&list).into_iter().enumerate() {
        out.set(Value::int(at as i64 + 1), describe(command, link));
    }
    Some(Value::table(out))
}

/// Every command in the list, in written order, with the operator that introduced it.
fn walk(list: &CommandList) -> Vec<(&Command, &'static str)> {
    let mut out = Vec::new();
    for (item_at, item) in list.items.iter().enumerate() {
        // What joins this *item* to the previous one is the previous item's operator: `a ; b` puts
        // the `;` on `a`, and the question a reader asks is how `b` arrived.
        let joined = match item_at {
            0 => FIRST,
            _ => match list.items[item_at - 1].op {
                ListOp::Background => "&",
                ListOp::Sequential | ListOp::Newline => ";",
            },
        };
        let mut link = joined;
        for (pipeline_at, pipeline) in std::iter::once(&item.and_or.first)
            .chain(item.and_or.rest.iter().map(|(_, p)| p))
            .enumerate()
        {
            if pipeline_at > 0 {
                link = match item.and_or.rest[pipeline_at - 1].0 {
                    oslo_base::ast::types::AndOrOp::And => "&&",
                    oslo_base::ast::types::AndOrOp::Or => "||",
                };
            }
            for (command_at, command) in pipeline.commands.iter().enumerate() {
                out.push((command, if command_at > 0 { "|" } else { link }));
            }
        }
    }
    out
}

/// One command: what it runs, and how it got here.
fn describe(command: &Command, link: &str) -> Value {
    let mut t = Table::new();
    t.set_str("link", Value::str(link));
    match command {
        Command::Simple(simple) => {
            t.set_str("kind", Value::str("simple"));
            let mut argv = Table::new();
            for (at, word) in simple.words.iter().enumerate() {
                argv.set(Value::int(at as i64 + 1), Value::str(flatten(word)));
            }
            t.set_str("argv", Value::table(argv));
            t.set(
                Value::str("redirects"),
                Value::int(simple.redirections.len() as i64),
            );
        }
        // A `for` loop or an `if` is one command to the line and has no argv of its own. Named
        // rather than skipped, so a count of `c.commands` matches what the line actually runs.
        Command::Compound { .. } => t.set_str("kind", Value::str("compound")),
        Command::FunctionDef { name, .. } => {
            t.set_str("kind", Value::str("function"));
            t.set_str("name", Value::str(name));
        }
    }
    Value::table(t)
}

/// A word as the text a config can use: quoting resolved, expansion left as written.
fn flatten(word: &Word) -> String {
    let mut out = String::new();
    for part in &word.parts {
        push(part, &mut out);
    }
    out
}

fn push(part: &WordPart, out: &mut String) {
    match part {
        WordPart::Literal(s) | WordPart::Escaped(s) | WordPart::SingleQuoted(s) => out.push_str(s),
        WordPart::DoubleQuoted(parts) => {
            for inner in parts {
                push(inner, out);
            }
        }
        // Left as it was written. The value is not known until expansion, and the honest answer to
        // "what is this word" before then is the thing the user typed.
        WordPart::Variable { name, .. } => {
            out.push('$');
            out.push_str(name);
        }
        WordPart::ArrayRef { name, .. } => {
            out.push_str("${");
            out.push_str(name);
            out.push_str("[…]}");
        }
        WordPart::CommandSubstitution(body) => {
            out.push_str("$(");
            out.push_str(body);
            out.push(')');
        }
        WordPart::FunctionSubstitution { body, reply } => {
            out.push_str(if *reply { "${|" } else { "${" });
            out.push_str(body);
            out.push('}');
        }
        WordPart::ProcessSubstitution { command, .. } => {
            out.push_str("<(");
            out.push_str(command);
            out.push(')');
        }
        WordPart::Arithmetic(body) => {
            out.push_str("$((");
            out.push_str(body);
            out.push_str("))");
        }
        WordPart::Tilde(user) => {
            out.push('~');
            out.push_str(user);
        }
    }
}

#[cfg(test)]
mod tests;
