//! Command and compound-command conversion.

use super::convert_command_list;
use super::redirects::{convert_redirects, redirects_of};
use crate::lexer::TildeMode;
use crate::syntax::lower::cond;
use crate::syntax::lower::{
    convert_braced_words_from_str, convert_words_from_str, single_word_from_str,
    single_word_with_tilde,
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

/// A list of words being converted, with the unbraced form kept once brace expansion changes one.
#[derive(Default)]
struct WordList {
    words: Vec<oslo_ast::Word>,
    unbraced: Option<Vec<oslo_ast::Word>>,
}

impl WordList {
    /// Add the words a `Word` node stands for.
    fn push_node(&mut self, tree: &Tree, node: &Node) -> Result<()> {
        let expanded = words_of(tree, node)?;
        let text = text_of(tree, node);
        if text.contains('{') && !holds_a_process_substitution(node) {
            let plain = convert_words_from_str(text)?;
            if plain != expanded {
                if self.unbraced.is_none() {
                    self.unbraced = Some(self.words.clone());
                }
                if let Some(unbraced) = &mut self.unbraced {
                    unbraced.extend(plain);
                }
                self.words.extend(expanded);
                return Ok(());
            }
        }
        self.push(expanded)
    }

    /// Add words that brace expansion has no part in.
    fn push(&mut self, words: Vec<oslo_ast::Word>) -> Result<()> {
        if let Some(unbraced) = &mut self.unbraced {
            unbraced.extend(words.iter().cloned());
        }
        self.words.extend(words);
        Ok(())
    }
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

/// Whether `node` holds a `<( )` of its own, directly or in the value word of an assignment.
pub(super) fn holds_a_process_substitution(node: &Node) -> bool {
    node.nodes().any(|child| {
        child.kind() == SyntaxKind::ProcessSubstitution
            || (child.kind() == SyntaxKind::Word && holds_a_process_substitution(child))
    })
}

pub(super) fn process_substitution_word(tree: &Tree, node: &Node) -> Result<oslo_ast::Word> {
    process_substitution_word_from(tree, node, node.span().start as usize)
}

/// [`process_substitution_word`] for the part of `node` from byte `from` of the source on: the
/// value of an `[index]=value` array element.
pub(super) fn process_substitution_word_from(
    tree: &Tree,
    node: &Node,
    from: usize,
) -> Result<oslo_ast::Word> {
    let mut parts = Vec::new();
    for child in node.children() {
        let span = match child {
            rune::Element::Node(inner) => inner.span(),
            rune::Element::Token(token) => token.span(),
        };
        let (start, end) = (span.start as usize, span.end as usize);
        if end <= from {
            continue;
        }
        if start < from {
            let text = &tree.source().text()[from..end];
            for word in convert_words_from_str(text)? {
                parts.extend(word.parts);
            }
            continue;
        }
        match child {
            rune::Element::Node(inner)
                if inner.kind() == SyntaxKind::Word && holds_a_process_substitution(inner) =>
            {
                parts.extend(process_substitution_word(tree, inner)?.parts);
            }
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
        // `coproc [NAME] command`: the array a coprocess publishes into is `COPROC` unless named.
        Command::Coproc(coproc) => {
            let name = coproc
                .name()
                .map(|word| word_text(tree, word).to_string())
                .unwrap_or_else(|| "COPROC".to_string());
            let body = coproc.command().ok_or_else(|| {
                ShellError::SyntaxError("coproc: a command to run was expected".to_string())
            })?;
            Ok(oslo_ast::Command::Compound {
                kind: oslo_ast::CompoundCommand::Coproc {
                    name,
                    body: Box::new(convert_command(tree, body)?),
                },
                redirections: Vec::new(),
            })
        }
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
            let start = function.span().start as usize;
            let end = super::redirects::extent_with_heredocs(tree, function.syntax()) as usize;
            let source = tree
                .source()
                .text()
                .get(start..end)
                .map(std::sync::Arc::from);
            Ok(oslo_ast::Command::FunctionDef {
                name,
                body: Box::new(body),
                source: oslo_ast::FunctionSource(source),
            })
        }
        Command::Conditional(conditional) => {
            let list = convert_cond(tree, conditional.syntax())?;
            let mut command = cond::as_command(list);
            // `[[ … ]] 2>/dev/null` redirects the whole test.
            if let oslo_ast::Command::Compound { redirections, .. } = &mut command {
                *redirections = convert_redirects(tree, redirects_of(conditional.syntax()))?;
            }
            Ok(command)
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
        Command::Simple(_)
        | Command::Function(_)
        | Command::Conditional(_)
        | Command::Coproc(_) => {
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
    let body = list_at(tree, looping.body())?;
    if looping.kind() == SyntaxKind::ArithForCommand {
        let (init, cond, step) = arithmetic_for_sections(looping.arithmetic(tree.source()));
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
    let (items, unbraced) = match has_in {
        false => (None, None),
        true => {
            let mut converted = WordList::default();
            for word in looping.items() {
                converted.push_node(tree, word.syntax())?;
            }
            (Some(converted.words), converted.unbraced)
        }
    };
    if looping.kind() == SyntaxKind::SelectCommand {
        return Ok(oslo_ast::CompoundCommand::Select {
            var_name,
            items,
            body,
        });
    }
    Ok(oslo_ast::CompoundCommand::For {
        var_name,
        items,
        unbraced,
        body,
    })
}

/// The three sections of a `for ((init; cond; step))` head, as raw text.
///
/// A section holding nothing but space is *absent*, not "the expression 0": evaluating the empty
/// middle of `for (( ; ; ))` as a condition would end the loop before its first iteration instead
/// of running it forever.
fn arithmetic_for_sections(
    header: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    let Some(inner) = header else {
        return (None, None, None);
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
    let mut words = WordList::default();
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
            SyntaxKind::Assignment if words.words.is_empty() => {
                assignments.push(convert_assignment(tree, child)?);
            }
            SyntaxKind::Assignment | SyntaxKind::Word => words.push_node(tree, child)?,
            SyntaxKind::Redirect => {
                // A `{name}` prefix belongs to the redirection; any other non-numeric one is a word.
                let variable = rune::ast::Redirect::cast(child)
                    .and_then(|redirect| redirect.variable(tree.source()));
                if variable.is_none()
                    && let Some(prefix) = child
                        .token(SyntaxKind::Text)
                        .filter(|token| token.text(tree.source()).parse::<i32>().is_err())
                {
                    words.push(convert_words_from_str(prefix.text(tree.source()))?)?;
                }
                redirections.extend(super::redirects::convert_redirect(tree, child)?);
            }
            _ => {}
        }
    }

    Ok(oslo_ast::SimpleCommand {
        assignments,
        words: words.words,
        redirections,
        unbraced: words.unbraced,
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

    // `name[i]=value`, whose subscript rune gives a node of its own: `a[$i]=x`, `a[ $i + 1 ]=x`.
    // The subscript is one word however many blanks it holds, and keeps them: `m[ a ]` is the key
    // ` a `.
    let target = match node.node(SyntaxKind::Subscript) {
        Some(subscript) => {
            let text = text_of(tree, subscript);
            let inner = text.strip_prefix('[').unwrap_or(text);
            let inner = inner.strip_suffix(']').unwrap_or(inner);
            oslo_ast::AssignmentTarget::Element {
                name,
                index: crate::lexer::parse_single_word(inner)?,
            }
        }
        None => oslo_ast::AssignmentTarget::Name(name),
    };

    let value = match node.node(SyntaxKind::ArrayValue) {
        Some(array) => {
            oslo_ast::AssignmentValue::Array(super::literal::array_elements(tree, array)?)
        }
        None => match node.node(SyntaxKind::Word) {
            Some(word) if holds_a_process_substitution(word) => {
                oslo_ast::AssignmentValue::Scalar(process_substitution_word(tree, word)?)
            }
            word => oslo_ast::AssignmentValue::Scalar(single_word_with_tilde(
                word.map(|word| text_of(tree, word)).unwrap_or_default(),
                TildeMode::Value,
            )?),
        },
    };

    Ok(oslo_ast::Assignment {
        target,
        value,
        append,
    })
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
