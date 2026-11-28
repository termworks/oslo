//! Whether a command could carry a coordinate anywhere.
//!
//! The gate for the whole feature: a pipeline that answers `false` runs down the path it always
//! did, forked concurrently, with nothing captured and nothing to pay for. That is the common case
//! and the reason this file exists at all — the question has to be cheap, and it has to be asked
//! before anything is captured.
//!
//! **It looks everywhere [`super::rewrite_command`] writes, and nowhere else.** The two walks
//! mirror each other, and both directions of a mismatch are a bug:
//!
//! * A gate that reads *less* leaves a command on the concurrent path where the rewriter never
//!   runs — `cat f | cat > {0:0}` wrote to a file called `{0:0}`, silently.
//! * A gate that reads *more* pulls a pipeline off the concurrent path to rewrite nothing, paying
//!   the sequential cost for no reason.
//!
//! Kept beside the rewriter rather than inside it so the mirroring is visible as two files with the
//! same shape, and a test holds them together.

use super::{holds_a_quoted_coordinate, looks_like_a_coordinate, only_literal};
use oslo_base::ast::{AssignmentValue, Command, CommandList, CompoundCommand, Redirection, Word};

/// Whether a command could carry a coordinate anywhere. See the module docs.
pub fn command_uses_coordinates(command: &Command) -> bool {
    match command {
        // Array values but not scalars and not subscripts, mirroring `rewrite_command` exactly.
        Command::Simple(simple) => {
            any_word_but_a_regex(&simple.words)
                || simple
                    .assignments
                    .iter()
                    .any(|assignment| match &assignment.value {
                        AssignmentValue::Scalar(_) => false,
                        AssignmentValue::Array(elements) => {
                            elements.iter().any(|element| is_one(&element.value))
                        }
                    })
                || any_redirection(&simple.redirections)
        }
        Command::Compound { kind, redirections } => {
            any_compound(kind) || any_redirection(redirections)
        }
        // Not rewritten, so not a reason to leave the concurrent path.
        Command::FunctionDef { .. } => false,
    }
}

fn is_one(word: &Word) -> bool {
    only_literal(word).is_some_and(looks_like_a_coordinate) || holds_a_quoted_coordinate(word)
}

fn any_word(words: &[Word]) -> bool {
    words.iter().any(is_one)
}

/// As [`any_word`], but blind to a `[[ … =~ … ]]` regex — which the rewriter will not touch, so
/// claiming it here would take a pipeline off the concurrent path to do nothing.
fn any_word_but_a_regex(words: &[Word]) -> bool {
    let regex = super::regex_operand_of_a_conditional(words);
    words
        .iter()
        .enumerate()
        .any(|(at, word)| Some(at) != regex && is_one(word))
}

fn any_redirection(redirections: &[Redirection]) -> bool {
    redirections.iter().any(|redirection| {
        is_one(&redirection.target) || redirection.heredoc_content.as_ref().is_some_and(is_one)
    })
}

fn any_compound(kind: &CompoundCommand) -> bool {
    match kind {
        CompoundCommand::If {
            condition,
            then_branch,
            elif_branches,
            else_branch,
        } => {
            any_list(condition)
                || any_list(then_branch)
                || elif_branches
                    .iter()
                    .any(|(condition, body)| any_list(condition) || any_list(body))
                || else_branch.as_ref().is_some_and(any_list)
        }
        CompoundCommand::While { condition, body } | CompoundCommand::Until { condition, body } => {
            any_list(condition) || any_list(body)
        }
        CompoundCommand::For { items, body, .. } | CompoundCommand::Select { items, body, .. } => {
            items.as_ref().is_some_and(|items| any_word(items)) || any_list(body)
        }
        CompoundCommand::Coproc { body, .. } => command_uses_coordinates(body),
        CompoundCommand::Case { word, items } => {
            is_one(word)
                || items
                    .iter()
                    .any(|item| any_word(&item.patterns) || any_list(&item.body))
        }
        CompoundCommand::ArithmeticFor { body, .. } => any_list(body),
        CompoundCommand::Subshell(list) | CompoundCommand::Group(list) => any_list(list),
        CompoundCommand::Arithmetic(_) => false,
    }
}

fn any_list(list: &CommandList) -> bool {
    list.items.iter().any(|item| {
        any_pipeline(&item.and_or.first)
            || item
                .and_or
                .rest
                .iter()
                .any(|(_, pipeline)| any_pipeline(pipeline))
    })
}

/// Mirrors `rewrite_pipeline`, including where it stops: a nested pipeline with stages of its own
/// is not this stage's business, so finding a coordinate in one must not open the gate here.
fn any_pipeline(pipeline: &oslo_base::ast::Pipeline) -> bool {
    pipeline.commands.len() == 1 && pipeline.commands.iter().any(command_uses_coordinates)
}
