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
//! - `coproc` is not a keyword to rune, so the `coproc` submodule blanks its header out of the
//!   source and wraps the command that is left.
//! - `time -p` is carried as plain `time`: [`oslo_base::ast::Pipeline`] records *that* a pipeline
//!   is timed, not which of bash's two report formats was asked for.
//! - `[[ ... ]]` becomes calls to the `[[` builtin, with `&&`, `||` and `!` expressed using the
//!   shell's own and-or lists and pipeline negation; see [`super::lower::cond`].

mod commands;
mod coproc;
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

    match coproc::prepare(script) {
        Some((text, sites)) => coproc::with_sites(sites, || parse_prepared(&text)),
        None => parse_prepared(script),
    }
}

fn parse_prepared(script: &str) -> Result<oslo_ast::CommandList> {
    let parsed = rune::parse(script);
    // **The first error is the one to report, and rune orders them by position.** It finds every
    // mistake in the file, which is what a checker wants; a shell about to run the script wants
    // the earliest one, because that is where the program stopped making sense.
    // The position is deliberately left out of the text: oslo prefixes its own `file: line N:`,
    // and a script run a statement at a time would have this one counting from the wrong place.
    let deferred = |error: &&rune::Error| {
        inside_closed_substitution(parsed.tree(), parsed.tree().root(), error.span.start)
    };
    let (warnings, errors): (Vec<_>, Vec<_>) = parsed
        .errors()
        .iter()
        .filter(|error| !deferred(error))
        .partition(|error| error.severity == rune::Severity::Warning);
    if let Some(error) = errors.first() {
        return Err(ShellError::SyntaxError(error.message.clone()));
    }
    // bash reads an `extglob` group only with the option on, and refuses the line otherwise. A
    // program that names the option outside a comment may turn it on before the group runs, and
    // is left to the check made where the group is expanded.
    if !oslo_base::glob::walk::extglob()
        && opens_group(script)
        && !names_extglob(parsed.tree())
        && refused_group(parsed.tree(), parsed.tree().root())
    {
        return Err(ShellError::SyntaxError(
            "syntax error near unexpected token `('".to_string(),
        ));
    }
    // bash runs a here-document the input ends inside, and says so.
    for warning in warnings {
        let (line, _) = parsed.tree().source().line_col(warning.span.start);
        match warning.message.contains("here-document") {
            true => {
                eprintln!("oslo: warning: here-document at line {line} delimited by end-of-file")
            }
            false => eprintln!("oslo: warning: line {line}: {}", warning.message),
        }
    }
    convert_script(parsed.tree())
}

/// Whether the tree holds an `extglob` group outside the places bash reads one regardless.
///
/// A `[[ ]]` test takes patterns with the option off. A substitution's body is read again when it
/// runs, and arithmetic, a subscript and a `${…}` operand are not words that could hold a group.
fn refused_group(tree: &Tree, node: &rune::Node) -> bool {
    if matches!(
        node.kind(),
        SyntaxKind::CondCommand
            | SyntaxKind::CommandSubstitution
            | SyntaxKind::ProcessSubstitution
            | SyntaxKind::ParameterExpansion
            | SyntaxKind::ArithmeticExpansion
            | SyntaxKind::ArithCommand
            | SyntaxKind::ArithForCommand
            | SyntaxKind::Subscript
    ) {
        return false;
    }
    // Inside `"…"` a parenthesis is a character, so the quotes a word opens are followed.
    let mut quoted = false;
    node.children().any(|child| match child {
        rune::Element::Node(inner) => refused_group(tree, inner),
        rune::Element::Token(token) => {
            if token.kind() == SyntaxKind::DoubleQuote {
                quoted = !quoted;
            }
            !quoted && token.kind() == SyntaxKind::Text && opens_group(token.text(tree.source()))
        }
    })
}

/// Whether any word of the program, rather than a comment or a here-document, names `extglob`.
fn names_extglob(tree: &Tree) -> bool {
    let mut named = false;
    tree.root().visit_tokens(&mut |token| {
        named |= !matches!(
            token.kind(),
            SyntaxKind::Comment | SyntaxKind::HeredocText | SyntaxKind::HeredocEnd
        ) && token.text(tree.source()).contains("extglob");
    });
    named
}

/// Whether `text` holds an `extglob` operator with its `(`: `@(`, `!(`, `*(`, `+(` or `?(`.
fn opens_group(text: &str) -> bool {
    text.as_bytes()
        .windows(2)
        .any(|pair| matches!(pair, [b'?' | b'*' | b'+' | b'@' | b'!', b'(']))
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

/// The elements of the array literal `(body)`, read exactly as the value of `a=(body)` is.
///
/// This is what the declaration builtins receive: `declare -a a=([0]=x [2]=y)` passes them the
/// text `a=([0]=x [2]=y)` unexpanded, as bash does, and the literal is read here with the same
/// rules a bare assignment's is — indices, quoting, newlines and comments included.
pub fn array_literal(body: &str) -> Result<Vec<oslo_ast::ArrayElement>> {
    let text = format!("a=({body})");
    let bad = || ShellError::SyntaxError(format!("({body}): bad array value"));
    let parsed = rune::parse(&text);
    if !parsed.is_clean() {
        return Err(bad());
    }
    let tree = parsed.tree();
    let mut items = Script::of(tree).items();
    let (Some(item), None) = (items.next(), items.next()) else {
        return Err(bad());
    };
    let Some(Command::Simple(simple)) = item.command() else {
        return Err(bad());
    };
    let mut children = simple.syntax().nodes();
    let (Some(assignment), None) = (children.next(), children.next()) else {
        return Err(bad());
    };
    let Some(array) = assignment
        .node(SyntaxKind::ArrayValue)
        .filter(|array| array.span().end as usize == text.len())
    else {
        return Err(bad());
    };
    let mut elements = Vec::new();
    for element in array.nodes().filter(|node| node.kind() == SyntaxKind::Word) {
        elements.extend(commands::convert_array_element(
            tree,
            tree.source().slice(element.span()),
        )?);
    }
    Ok(elements)
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
