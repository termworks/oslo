//! Bridge from `rune`'s syntax tree to oslo's own [`oslo_base::ast`].
//!
//! rune produces a lossless tree over the source text; oslo's evaluator works on its own simpler
//! AST. This module is the translation, and it is the *only* path from source text to a runnable
//! program: `main`, `eval`, `source`, command substitution and the Lua `oslo.proc.exec` binding
//! all come through here, and an error raised here is what the user sees.
//!
//! That makes fidelity here load-bearing: anything this module drops is silently unobservable at
//! runtime. Anything it cannot represent must therefore be *rejected by name*, never approximated.
//!
//! # What rune hands over
//!
//! Words arrive as source text and are re-lexed by [`super::lower::words`], exactly as they were
//! under the previous parser — that machinery is about what shell means by a word, not about who
//! parsed it, and it is shared with the runtime. What changed is everything above the word: the
//! tree is rune's, the spans are real, and a script with several mistakes in it reports several.
//!
//! # Known gaps
//!
//! - Here-document bodies are carried through verbatim; whether an unquoted body expands is
//!   decided from the delimiter's quoting, which rune records.
//! - `coproc` is refused by name rather than approximated: it has no representation in oslo's AST
//!   and cannot get one until there are arrays to publish the descriptors in and job control to
//!   manage the child.
//! - `time -p` is carried as plain `time`: [`oslo_base::ast::Pipeline`] records *that* a pipeline
//!   is timed, not which of bash's two report formats was asked for.
//! - `[[ ... ]]` becomes calls to the `[[` builtin, with `&&`, `||` and `!` expressed using the
//!   shell's own and-or lists and pipeline negation; see [`super::lower::cond`].

mod commands;
mod redirects;

use oslo_base::ast as oslo_ast;
use oslo_base::error::{Result, ShellError};
use rune::ast::{AndOrList, Command, CommandList, ListItem, Pipeline, Script};
use rune::{SyntaxKind, Tree};

/// Parse `script` and lower it into something oslo can run.
pub fn parse_bash_script(script: &str) -> Result<oslo_ast::CommandList> {
    // Before the tree is walked, not after: the lowering and the evaluator are both recursive, so
    // absurdly nested input would overflow the stack and abort the process before any error of
    // ours could be produced. See [`oslo_base::nesting`]. rune's own parser is bounded and does
    // not need the guard; what comes after it does.
    oslo_base::nesting::check_nesting(script)?;

    let parsed = rune::parse(script);
    // **The first error is the one to report, and rune orders them by position.** It finds every
    // mistake in the file, which is what a checker wants; a shell about to run the script wants
    // the earliest one, because that is where the program stopped making sense.
    // The position is deliberately left out of the text: oslo prefixes its own `file: line N:`,
    // and a script run a statement at a time would have this one counting from the wrong place.
    let deferred = |error: &&rune::Error| {
        inside_closed_substitution(parsed.tree(), parsed.tree().root(), error.span.start)
    };
    if let Some(error) = parsed.errors().iter().find(|error| !deferred(error)) {
        return Err(ShellError::SyntaxError(error.message.clone()));
    }
    convert_script(parsed.tree())
}

/// Whether an offset falls inside a command substitution that was properly closed.
///
/// **What is inside `$( )` is not parsed until the substitution runs.** The body is carried
/// through as text and re-parsed at expansion time, so `echo $(if)` is a command substitution that
/// fails when it is expanded, not a script that will not parse: bash reports it and leaves a
/// non-interactive shell with 127, where refusing the whole file gives 2 and never runs the line.
///
/// Only a *closed* one defers. `echo $(ls` ran out of input looking for the `)`, which makes the
/// outer script unfinished rather than the inner one wrong, and that is a parse error in any shell.
fn inside_closed_substitution(tree: &Tree, node: &rune::Node, at: u32) -> bool {
    for child in node.nodes() {
        if !child.span().contains(at) {
            continue;
        }
        let substitution = matches!(
            child.kind(),
            SyntaxKind::CommandSubstitution | SyntaxKind::ProcessSubstitution
        );
        if substitution && closed(tree, child) {
            return true;
        }
        if inside_closed_substitution(tree, child, at) {
            return true;
        }
    }
    false
}

/// Whether a substitution reached its closing `)` or backtick.
///
/// Read from the text rather than from the last token: when the body does not parse, recovery may
/// have swallowed the `)` into whatever went wrong in there, and it is still the character that
/// closed the substitution.
fn closed(tree: &Tree, node: &rune::Node) -> bool {
    let text = tree.source().slice(node.span());
    match text.starts_with('`') {
        true => text.len() >= 2 && text.ends_with('`'),
        false => text.ends_with(')'),
    }
}

fn convert_script(tree: &Tree) -> Result<oslo_ast::CommandList> {
    match Script::of(tree).commands() {
        Some(list) => convert_command_list(tree, list),
        None => Ok(oslo_ast::CommandList::default()),
    }
}

pub(super) fn convert_command_list(
    tree: &Tree,
    list: CommandList<'_>,
) -> Result<oslo_ast::CommandList> {
    let mut items = Vec::new();
    let mut previous_unterminated = false;
    for item in list.items() {
        if previous_unterminated {
            return Err(ShellError::SyntaxError(
                "commands must be separated by `;`, `&`, or a newline".to_string(),
            ));
        }
        previous_unterminated = item.terminator().is_none();
        items.push(convert_list_item(tree, item)?);
    }
    Ok(oslo_ast::CommandList { items })
}

/// Convert one list item, preserving its separator.
///
/// The separator is what distinguishes `sleep 10 &` from `sleep 10;` — dropping it silently turns
/// every background job into a foreground one.
fn convert_list_item(tree: &Tree, item: ListItem<'_>) -> Result<oslo_ast::ListItem> {
    // The tree carries real byte offsets, so the line is a lookup rather than something the
    // parser had to be asked to record. It is what lets `$LINENO` name a real line.
    let (line, _) = tree.source().line_col(item.span().start);
    let and_or = convert_and_or(tree, item)?;
    let op = match item.is_background() {
        true => oslo_ast::ListOp::Background,
        false => oslo_ast::ListOp::Sequential,
    };
    Ok(oslo_ast::ListItem { and_or, op, line })
}

/// The and-or list inside a statement, whether or not one was written.
///
/// rune only builds an `AndOrList` node when there is an operator to justify it, so a lone command
/// arrives with nothing around it and becomes a chain of one.
fn convert_and_or(tree: &Tree, item: ListItem<'_>) -> Result<oslo_ast::AndOrList> {
    if let Some(list) = item.and_or() {
        return convert_and_or_node(tree, list);
    }
    Ok(oslo_ast::AndOrList {
        first: convert_pipeline(tree, item)?,
        rest: Vec::new(),
    })
}

/// Each branch of `a && b || c`, with the operator that reached it.
///
/// The children are walked directly rather than through rune's own `branches`, because a branch
/// may be a *pipeline* rather than a command and only a command casts. `env | grep x || true` has
/// a pipeline on the left, and reading it as "no branches" lost the whole line.
fn convert_and_or_node(tree: &Tree, list: AndOrList<'_>) -> Result<oslo_ast::AndOrList> {
    let mut first = None;
    let mut rest = Vec::new();
    let mut operator = None;

    for child in list.syntax().children() {
        match child {
            rune::Element::Token(token) => match token.kind() {
                SyntaxKind::AndAnd => operator = Some(oslo_ast::AndOrOp::And),
                SyntaxKind::PipePipe => operator = Some(oslo_ast::AndOrOp::Or),
                _ => {}
            },
            rune::Element::Node(node) => {
                let Some(pipeline) = branch_pipeline(tree, node)? else {
                    continue;
                };
                match operator.take() {
                    None => first = Some(pipeline),
                    Some(op) => rest.push((op, pipeline)),
                }
            }
        }
    }

    match first {
        Some(first) => Ok(oslo_ast::AndOrList { first, rest }),
        None => Err(ShellError::SyntaxError(
            "an and-or list with nothing in it".to_string(),
        )),
    }
}

/// One branch, which is a pipeline if it is written as one and a command otherwise.
fn branch_pipeline(tree: &Tree, node: &rune::Node) -> Result<Option<oslo_ast::Pipeline>> {
    if let Some(pipeline) = Pipeline::cast(node) {
        return Ok(Some(convert_pipeline_node(tree, pipeline)?));
    }
    match Command::cast(node) {
        Some(command) => Ok(Some(pipeline_around(tree, command)?)),
        None => Ok(None),
    }
}

/// The pipeline inside a statement, whether or not one was written.
fn convert_pipeline(tree: &Tree, item: ListItem<'_>) -> Result<oslo_ast::Pipeline> {
    match item.pipeline() {
        Some(pipeline) => convert_pipeline_node(tree, pipeline),
        None => match item.command() {
            Some(command) => pipeline_around(tree, command),
            None => Ok(oslo_ast::Pipeline {
                negated: false,
                timed: false,
                commands: Vec::new(),
            }),
        },
    }
}

/// Wrap one command as a pipeline of one, unless it is itself a pipeline.
fn pipeline_around(tree: &Tree, command: Command<'_>) -> Result<oslo_ast::Pipeline> {
    if let Some(pipeline) = Pipeline::cast(command.syntax()) {
        return convert_pipeline_node(tree, pipeline);
    }
    Ok(oslo_ast::Pipeline {
        negated: false,
        timed: false,
        commands: vec![commands::convert_command(tree, command)?],
    })
}

fn convert_pipeline_node(tree: &Tree, pipeline: Pipeline<'_>) -> Result<oslo_ast::Pipeline> {
    let mut converted = Vec::new();
    for command in pipeline.commands() {
        converted.push(commands::convert_command(tree, command)?);
    }
    Ok(oslo_ast::Pipeline {
        negated: pipeline.is_negated(),
        // The keyword is recorded, not the report format: `-p` is flattened into the same flag.
        timed: pipeline.is_timed(),
        commands: converted,
    })
}

/// A construct oslo parses but cannot yet execute.
///
/// Reported as a syntax error so it reaches the user with a non-zero status and the name of the
/// thing that is missing, rather than being approximated into something that runs.
pub(super) fn unsupported(construct: &str) -> ShellError {
    ShellError::SyntaxError(format!("{construct} is not supported yet"))
}
