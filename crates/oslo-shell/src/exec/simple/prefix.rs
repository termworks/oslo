//! The assignments in front of a command word: `x=1 y=$x cmd`.

use super::{escape, heredocs_expanded, run_command_word, trace};
use crate::env::Environment;
use crate::expand::expand_word_to_string;
use oslo_base::ast::{Assignment, AssignmentTarget, AssignmentValue, SimpleCommand};
use oslo_base::error::Result;

/// The command the assignments are in front of, its words already expanded.
pub(super) struct Command<'a> {
    pub(super) name: &'a str,
    pub(super) words: &'a [String],
    pub(super) escape: escape::Escape,
}

/// Run `command` with its prefix assignments in a scope of their own.
///
/// The words were expanded before any assignment, and each assignment is expanded after the one
/// before it is made, so `x=1 y=$x cmd` gives `y` the new `x`. The scope is undone when the command
/// ends, except after a special builtin in POSIX mode, where the assignments persist as in bash.
/// `keyword` holds the `set -k` assignments found among the arguments.
pub(super) fn run(
    env: &mut Environment,
    simple: &SimpleCommand,
    keyword: Vec<(String, String)>,
    command: Command<'_>,
) -> Result<i32> {
    // A here-document is expanded without the assignments in scope, as bash and dash do.
    let redirections = heredocs_expanded(env, &simple.redirections)?;
    let special = persists(env, &command);
    env.push_temp_scope();
    let traced = match assign_all(env, &simple.assignments, keyword, special) {
        Ok(traced) => traced,
        Err(e) => {
            env.pop_scope();
            return Err(e);
        }
    };
    trace::trace_command(env, &traced, command.words);
    let result = run_command_word(
        env,
        command.name,
        command.words,
        &redirections,
        command.escape,
    );
    match special {
        true => env.keep_temp_scope(),
        false => env.pop_scope(),
    }
    result
}

/// Whether the command is a special builtin run in POSIX mode, after which the assignments stay.
fn persists(env: &Environment, command: &Command<'_>) -> bool {
    env.posix()
        && !command.escape.skips_builtin()
        && crate::env::scope::is_special_builtin(command.name)
        && env.is_builtin(command.name)
}

/// Make each assignment in turn, answering how `set -x` shows them.
fn assign_all(
    env: &mut Environment,
    assignments: &[Assignment],
    keyword: Vec<(String, String)>,
    special: bool,
) -> Result<Vec<String>> {
    let mut traced = Vec::new();
    for assign in assignments {
        let name = assign.name();
        let Some(value) = value_of(env, assign)? else {
            // POSIX mode abandons the line over it, and ends the shell before a special builtin.
            if special {
                super::posix::assignment_failure(env, name)?;
            } else if env.posix() {
                return Err(oslo_base::error::ShellError::Abandoned);
            }
            continue;
        };
        traced.push(trace::assignment_text(name, false, &value));
        if !env.set_local_exported_var(name, &value) && special {
            super::posix::assignment_failure(env, name)?;
        }
    }
    for (name, value) in keyword {
        traced.push(trace::assignment_text(&name, false, &value));
        env.set_local_exported_var(&name, &value);
    }
    Ok(traced)
}

/// The value an assignment gives its name, or `None` for an array element, which bash reports
/// as not a valid identifier and skips.
fn value_of(env: &mut Environment, assign: &Assignment) -> Result<Option<String>> {
    let written = match (&assign.target, &assign.value) {
        (AssignmentTarget::Element { name, index }, _) => {
            let target = format!(
                "{name}[{}]",
                crate::env::builtins::control::format_word(index)
            );
            eprintln!("{}`{target}': not a valid identifier", env.origin());
            return Ok(None);
        }
        (_, AssignmentValue::Scalar(word)) => expand_word_to_string(env, word)?,
        // `b=(1 2) cmd` hands the command the text `(1 2)`, as bash does.
        (_, AssignmentValue::Array(elements)) => {
            let mut parts = Vec::with_capacity(elements.len());
            for element in elements {
                let value = expand_word_to_string(env, &element.value)?;
                parts.push(match &element.index {
                    Some(index) => format!("[{}]={value}", expand_word_to_string(env, index)?),
                    None => value,
                });
            }
            format!("({})", parts.join(" "))
        }
    };
    Ok(Some(match assign.append {
        true => format!(
            "{}{written}",
            env.get_var(assign.name()).unwrap_or_default()
        ),
        false => written,
    }))
}
