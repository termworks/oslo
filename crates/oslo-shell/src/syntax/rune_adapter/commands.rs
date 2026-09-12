//! Command and compound-command conversion.

use super::redirects::{convert_redirects, redirects_of};
use super::{convert_command_list, unsupported};
use crate::syntax::lower::cond;
use crate::syntax::lower::{
    convert_braced_words_from_str, convert_words_from_str, single_word_from_str,
};
use oslo_base::ast as oslo_ast;
use oslo_base::error::{Result, ShellError};
use rune::ast::{Command, CommandList, ForCommand, IfCommand, SimpleCommand, Word};
use rune::{Node, SyntaxKind, Tree};

/// The text a node covers, exactly as written.
pub(super) fn text_of<'t>(tree: &'t Tree, node: &Node) -> &'t str {
    tree.source().slice(node.span())
}

fn word_text<'t>(tree: &'t Tree, word: Word<'_>) -> &'t str {
    tree.source().slice(word.span())
}

/// The words a `Word` node stands for, brace expansion and all.
///
/// Almost every word is re-lexed from its source text, because that is what oslo's evaluator wants
/// and what its own lexer already knows how to read. A process substitution is the exception: it
/// is not a word-level construct at all, so the word lexer would give up on it and hand back a
/// literal `<(cmd)` — which `cat <(echo hi)` would then pass as a filename that does not exist.
pub(super) fn words_of(tree: &Tree, node: &Node) -> Result<Vec<oslo_ast::Word>> {
    if !holds_a_process_substitution(node) {
        return convert_braced_words_from_str(text_of(tree, node));
    }
    Ok(vec![process_substitution_word(tree, node)?])
}

/// One word, for the positions that take exactly one — a redirect target, a `case` word.
///
/// Brace expansion does not apply in those places, so this is the un-braced counterpart of
/// [`words_of`]. `cmd < <(gen)` still has to reach the process substitution, because the target of
/// a redirection is a filename and that is precisely what a process substitution produces.
pub(super) fn word_of(tree: &Tree, node: &Node) -> Result<oslo_ast::Word> {
    if !holds_a_process_substitution(node) {
        return single_word_from_str(text_of(tree, node));
    }
    process_substitution_word(tree, node)
}

fn holds_a_process_substitution(node: &Node) -> bool {
    node.nodes()
        .any(|child| child.kind() == SyntaxKind::ProcessSubstitution)
}

fn process_substitution_word(tree: &Tree, node: &Node) -> Result<oslo_ast::Word> {
    let mut parts = Vec::new();
    for child in node.children() {
        match child {
            rune::Element::Node(inner) if inner.kind() == SyntaxKind::ProcessSubstitution => {
                let text = text_of(tree, inner);
                // The body is carried as text and re-parsed when it runs, exactly as `$(cmd)` is.
                let command = text
                    .strip_prefix("<(")
                    .or_else(|| text.strip_prefix(">("))
                    .and_then(|rest| rest.strip_suffix(')'))
                    .unwrap_or(text);
                parts.push(oslo_ast::WordPart::ProcessSubstitution {
                    reads_from_command: text.starts_with("<("),
                    command: command.to_string(),
                });
            }
            rune::Element::Node(inner) => {
                for word in convert_words_from_str(text_of(tree, inner))? {
                    parts.extend(word.parts);
                }
            }
            rune::Element::Token(token) => {
                for word in convert_words_from_str(token.text(tree.source()))? {
                    parts.extend(word.parts);
                }
            }
        }
    }
    Ok(oslo_ast::Word { parts })
}

pub(super) fn convert_command(tree: &Tree, command: Command<'_>) -> Result<oslo_ast::Command> {
    match command {
        Command::Simple(simple) => Ok(oslo_ast::Command::Simple(convert_simple(tree, simple)?)),
        Command::Function(function) => {
            let name = function
                .name()
                .map(|word| word_text(tree, word).to_string())
                .ok_or_else(|| ShellError::SyntaxError("a function with no name".to_string()))?;
            let body = match function.body() {
                Some(body) => convert_command(tree, body)?,
                None => {
                    return Err(ShellError::SyntaxError(format!(
                        "the function {name} has no body"
                    )));
                }
            };
            Ok(oslo_ast::Command::FunctionDef {
                name,
                body: Box::new(body),
            })
        }
        Command::Conditional(conditional) => {
            let list = convert_cond(tree, conditional.syntax())?;
            Ok(cond::as_command(list))
        }
        other => {
            let kind = convert_compound(tree, other)?;
            Ok(oslo_ast::Command::Compound {
                kind,
                // Redirections attached to the whole compound, e.g. `while ...; done > log`.
                redirections: convert_redirects(tree, redirects_of(other.syntax()))?,
            })
        }
    }
}

fn convert_compound(tree: &Tree, command: Command<'_>) -> Result<oslo_ast::CompoundCommand> {
    Ok(match command {
        Command::If(conditional) => convert_if(tree, conditional)?,
        Command::Loop(looping) => {
            let condition = list_at(tree, looping.condition())?;
            let body = list_at(tree, looping.body())?;
            match looping.is_until() {
                true => oslo_ast::CompoundCommand::Until { condition, body },
                false => oslo_ast::CompoundCommand::While { condition, body },
            }
        }
        Command::For(looping) => convert_for(tree, looping)?,
        Command::Case(case) => {
            let word = single_word_from_str(
                case.word()
                    .map(|word| word_text(tree, word))
                    .unwrap_or_default(),
            )?;
            let mut items = Vec::new();
            for item in case.items() {
                let mut patterns = Vec::new();
                for pattern in item.patterns() {
                    patterns.extend(convert_words_from_str(word_text(tree, pattern))?);
                }
                // The terminator is part of the program, not punctuation: `;&` and `;;&` select
                // different branches from `;;`, and collapsing all three made a fallthrough chain
                // run one branch.
                let post_action = match item.terminator() {
                    Some(SyntaxKind::SemiAmp) => oslo_ast::CaseAction::FallThrough,
                    Some(SyntaxKind::SemiSemiAmp) => oslo_ast::CaseAction::ContinueMatching,
                    _ => oslo_ast::CaseAction::ExitCase,
                };
                items.push(oslo_ast::CaseItem {
                    patterns,
                    body: list_at(tree, item.body())?,
                    post_action,
                });
            }
            oslo_ast::CompoundCommand::Case { word, items }
        }
        Command::Subshell(subshell) => {
            oslo_ast::CompoundCommand::Subshell(list_at(tree, subshell.body())?)
        }
        Command::Group(group) => oslo_ast::CompoundCommand::Group(list_at(tree, group.body())?),
        // The expression text is carried through unparsed on purpose: parameters and command
        // substitutions inside it are expanded when the command runs, not now.
        Command::Arithmetic(arithmetic) => {
            oslo_ast::CompoundCommand::Arithmetic(arithmetic.expression(tree.source()).to_string())
        }
        Command::Simple(_) | Command::Function(_) | Command::Conditional(_) => {
            return Err(ShellError::SyntaxError(
                "not a compound command".to_string(),
            ));
        }
    })
}

fn convert_if(tree: &Tree, conditional: IfCommand<'_>) -> Result<oslo_ast::CompoundCommand> {
    let mut elif_branches = Vec::new();
    for clause in conditional.elif_clauses() {
        elif_branches.push((
            list_at(tree, clause.condition())?,
            list_at(tree, clause.then_branch())?,
        ));
    }
    let else_branch = match conditional.else_branch() {
        Some(list) => Some(convert_command_list(tree, list)?),
        None => None,
    };
    Ok(oslo_ast::CompoundCommand::If {
        condition: list_at(tree, conditional.condition())?,
        then_branch: list_at(tree, conditional.then_branch())?,
        elif_branches,
        else_branch,
    })
}

fn convert_for(tree: &Tree, looping: ForCommand<'_>) -> Result<oslo_ast::CompoundCommand> {
    if looping.kind() == SyntaxKind::SelectCommand {
        return Err(unsupported("select"));
    }
    let body = list_at(tree, looping.body())?;
    if looping.kind() == SyntaxKind::ArithForCommand {
        let (init, cond, step) = arithmetic_for_sections(tree, looping.syntax());
        return Ok(oslo_ast::CompoundCommand::ArithmeticFor {
            init,
            cond,
            step,
            body,
        });
    }
    let var_name = looping
        .variable()
        .map(|word| word_text(tree, word).to_string())
        .ok_or_else(|| ShellError::SyntaxError("`for` with no variable".to_string()))?;

    // `for i; do` iterates the positional parameters, which is not the same as iterating nothing —
    // so an absent `in` has to stay absent rather than becoming an empty list.
    let has_in = looping
        .syntax()
        .tokens()
        .any(|token| token.kind() == SyntaxKind::In);
    let items = match has_in {
        false => None,
        true => {
            let mut converted = Vec::new();
            for word in looping.items() {
                converted.extend(words_of(tree, word.syntax())?);
            }
            Some(converted)
        }
    };
    Ok(oslo_ast::CompoundCommand::For {
        var_name,
        items,
        body,
    })
}

/// The three sections of a `for ((init; cond; step))` head, as raw text.
///
/// A section holding nothing but space is *absent*, not "the expression 0": evaluating the empty
/// middle of `for (( ; ; ))` as a condition would end the loop before its first iteration instead
/// of running it forever.
fn arithmetic_for_sections(
    tree: &Tree,
    node: &Node,
) -> (Option<String>, Option<String>, Option<String>) {
    let text = text_of(tree, node);
    let inner = match text.find("((") {
        Some(at) => &text[at + 2..],
        None => return (None, None, None),
    };
    let inner = match inner.find("))") {
        Some(at) => &inner[..at],
        None => inner,
    };
    let mut sections = inner
        .splitn(3, ';')
        .map(|section| Some(section.to_string()).filter(|text| !text.trim().is_empty()));
    let init = sections.next().flatten();
    let cond = sections.next().flatten();
    let step = sections.next().flatten();
    (init, cond, step)
}

fn list_at(tree: &Tree, list: Option<CommandList<'_>>) -> Result<oslo_ast::CommandList> {
    match list {
        Some(list) => convert_command_list(tree, list),
        None => Ok(oslo_ast::CommandList::default()),
    }
}

fn convert_simple(tree: &Tree, simple: SimpleCommand<'_>) -> Result<oslo_ast::SimpleCommand> {
    let mut assignments = Vec::new();
    let mut words = Vec::new();
    let mut redirections = Vec::new();

    for child in simple.syntax().nodes() {
        match child.kind() {
            // **Only a *prefix* `name=value` is an assignment.** After the command name it is an
            // ordinary argument — `alias g='echo hi'`, `env FOO=bar`, and `declare -a w=(a b)`,
            // where the builtin parses the array itself. rune records the same shape either way,
            // because it is the same shape; which of the two it means is decided by where it sits.
            //
            // The word is built from the assignment's own source text rather than by rejoining a
            // name and a value: the rejoined text would be a bare literal, so its quotes would
            // survive expansion and the value would then be field-split on its own spaces.
            SyntaxKind::Assignment if words.is_empty() => {
                assignments.push(convert_assignment(tree, child)?);
            }
            SyntaxKind::Assignment => {
                words.extend(words_of(tree, child)?);
            }
            SyntaxKind::Redirect => {
                if let Some(prefix) = child
                    .token(SyntaxKind::Text)
                    .filter(|token| token.text(tree.source()).parse::<i32>().is_err())
                {
                    words.extend(convert_words_from_str(prefix.text(tree.source()))?);
                }
                redirections.extend(super::redirects::convert_redirect(tree, child)?);
            }
            SyntaxKind::Word => {
                words.extend(words_of(tree, child)?);
            }
            _ => {}
        }
    }

    Ok(oslo_ast::SimpleCommand {
        assignments,
        words,
        redirections,
    })
}

/// Convert one `name=value`, `name[i]=value`, `name=(…)` or any of their `+=` forms.
///
/// The shape is preserved rather than flattened. Both halves used to be turned into text:
/// `a=(1 2 3)` kept only the first word of `(1 2 3)`, so `echo "$a"` printed the source
/// parentheses, and `a[1]=x` became a variable *literally named* `a[1]`.
fn convert_assignment(tree: &Tree, node: &Node) -> Result<oslo_ast::Assignment> {
    let name = node
        .tokens()
        .find(|token| token.kind() == SyntaxKind::Text)
        .map(|token| token.text(tree.source()).to_string())
        .unwrap_or_default();
    let append = node.token(SyntaxKind::PlusEqual).is_some();

    // `name[i]=value`. rune keeps the subscript in the name token's text, because the lexer has no
    // reason to split a run of ordinary characters; the shape is recovered here.
    let target = match name.split_once('[') {
        Some((name, rest)) => oslo_ast::AssignmentTarget::Element {
            name: name.to_string(),
            index: single_word_from_str(rest.trim_end_matches(']'))?,
        },
        None => oslo_ast::AssignmentTarget::Name(name),
    };

    let value = match node.node(SyntaxKind::ArrayValue) {
        Some(array) => {
            let mut elements = Vec::new();
            for element in array.nodes().filter(|node| node.kind() == SyntaxKind::Word) {
                elements.extend(convert_array_element(tree, text_of(tree, element))?);
            }
            oslo_ast::AssignmentValue::Array(elements)
        }
        None => oslo_ast::AssignmentValue::Scalar(single_word_from_str(
            node.node(SyntaxKind::Word)
                .map(|word| text_of(tree, word))
                .unwrap_or_default(),
        )?),
    };

    Ok(oslo_ast::Assignment {
        target,
        value,
        append,
    })
}

/// One element of an array literal.
///
/// An unindexed element may still expand to several words — `a=($list)` and `a=(*.c)` both do — so
/// it becomes one [`oslo_ast::ArrayElement`] per word rather than being forced into one. An
/// *indexed* element (`[3]=x`) is a single value by construction.
fn convert_array_element(tree: &Tree, text: &str) -> Result<Vec<oslo_ast::ArrayElement>> {
    let _ = tree;
    if let Some(rest) = text.strip_prefix('[')
        && let Some((index, value)) = rest.split_once("]=")
    {
        return Ok(vec![oslo_ast::ArrayElement {
            index: Some(single_word_from_str(index)?),
            value: single_word_from_str(value)?,
        }]);
    }
    Ok(convert_braced_words_from_str(text)?
        .into_iter()
        .map(|value| oslo_ast::ArrayElement { index: None, value })
        .collect())
}

/// Lower a `[[ ... ]]` expression onto and-or lists and calls to the `[[` builtin.
pub(super) fn convert_cond(tree: &Tree, node: &Node) -> Result<oslo_ast::AndOrList> {
    match node.kind() {
        SyntaxKind::CondCommand | SyntaxKind::CondGroup => {
            let inner = node
                .nodes()
                .find(|child| child.kind() != SyntaxKind::Word)
                .ok_or_else(|| ShellError::SyntaxError("an empty `[[ ]]` test".to_string()))?;
            convert_cond(tree, inner)
        }
        SyntaxKind::CondOr | SyntaxKind::CondAnd => {
            let op = match node.kind() == SyntaxKind::CondAnd {
                true => oslo_ast::AndOrOp::And,
                false => oslo_ast::AndOrOp::Or,
            };
            let mut branches = node
                .nodes()
                .filter(|child| child.kind() != SyntaxKind::Word);
            let first = branches
                .next()
                .ok_or_else(|| ShellError::SyntaxError("an empty `[[ ]]` test".to_string()))?;
            let mut list = convert_cond(tree, first)?;
            for branch in branches {
                list = cond::join(list, op, convert_cond(tree, branch)?);
            }
            Ok(list)
        }
        SyntaxKind::CondNot => {
            let inner = node
                .nodes()
                .find(|child| child.kind() != SyntaxKind::Word)
                .ok_or_else(|| ShellError::SyntaxError("`!` with nothing to negate".to_string()))?;
            Ok(cond::negate(convert_cond(tree, inner)?))
        }
        SyntaxKind::CondUnary => {
            let mut words = node
                .nodes()
                .filter(|child| child.kind() == SyntaxKind::Word);
            let op = words.next().map(|word| text_of(tree, word)).unwrap_or("-n");
            let operand = words.next().map(|word| text_of(tree, word)).unwrap_or("");
            cond::unary(op, operand)
        }
        SyntaxKind::CondBinary => {
            let mut operands = node
                .nodes()
                .filter(|child| child.kind() == SyntaxKind::CondWord);
            let left = operands
                .next()
                .map(|word| text_of(tree, word))
                .unwrap_or("");
            let right = operands
                .next()
                .map(|word| text_of(tree, word))
                .unwrap_or("");
            let op = node
                .tokens()
                .find(|token| !token.kind().is_trivia())
                .map(|token| token.text(tree.source()))
                .unwrap_or("=");
            cond::binary(left, op, right)
        }
        SyntaxKind::CondWord => cond::bare(text_of(tree, node)),
        _ => Err(ShellError::SyntaxError(format!(
            "this is not part of a `[[ ]]` test: {:?}",
            node.kind()
        ))),
    }
}
