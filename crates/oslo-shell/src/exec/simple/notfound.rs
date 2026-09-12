//! The end of the command search, and saying why in a way that helps.
//!
//! Split from `simple` because reporting a word that would not run is a different job from running
//! one, and this half is where the shell has to be careful not to guess.

use super::*;

/// Where the shell looked, for the help line under a name it could not run.
///
/// **The four places, in the order it tried them.** "command not found" says the answer and not the
/// question, and the question is what a person needs when the name is one they are sure exists:
/// almost always it is on a `$PATH` this shell was not given, or it is a function in a file that
/// was never sourced.
const WHERE_IT_LOOKED: &str = "looked at aliases, functions, builtins and $PATH, in that order";

/// The end of the command search: nothing on `$PATH`, no function, no builtin.
///
/// Split out because two arms reach it — the ordinary one and `\cmd`, which skips the autoload
/// step between them — and a shell that said something different depending on which would be
/// reporting on its own internals rather than on the command.
pub(super) fn nothing_to_run(
    env: &mut Environment,
    cmd_name: &str,
    words: &[String],
    redirections: &[Redirection],
) -> Result<i32> {
    // Before giving up, ask the config. A distribution's package manager is the obvious handler —
    // "nvim is in package neovim", or install it and run it — and a handler that resolved the
    // situation answers with the status to report. Everyone else bolts this on as a shell
    // function; here it is a hook.
    if let Some(status) = oslo_base::hooks::ask_hook_here(
        oslo_base::hooks::at::COMMAND_NOT_FOUND,
        vec![oslo_base::value::Value::str(cmd_name)],
    ) {
        return Ok(status);
    }
    // Nobody handled it, so say what a person needs next: the name that was probably meant. Only
    // when the shell is interactive — a script's stderr is read by machines, and bash says exactly
    // "command not found" there.
    //
    // **Not for a word starting with `@`.** That position is reserved and `@name` does not expand
    // there, so there is no command it could have been meant to be: `@proj` was answered with
    // "did you mean gprof?", which is a guess about a word the shell has decided not to read.
    let hint = if env.interactive() && !cmd_name.starts_with('@') {
        let path = env.get_var("PATH").unwrap_or_default().to_string();
        oslo_ui::command_index::nearest(&path, cmd_name)
    } else {
        None
    };
    let reason = match structured_verb_reason(env, cmd_name) {
        Some(explained) => explained,
        None => match hint {
            Some(near) => format!("command not found; did you mean {near}?"),
            None => "command not found".to_string(),
        },
    };
    report_unrunnable(env, redirections, cmd_name, words, &reason, 127)
}

/// Why a *structured verb* is being reported as a missing command, when it is not missing at all.
///
/// **The old message named the wrong thing entirely.** A verb reaches here only when no edge of its
/// pipeline carried rows, so the shell looked the name up on `$PATH`, failed, and guessed:
/// `where: command not found; did you mean hexe?`. Every word of that is misleading — `where` exists,
/// `$PATH` was never where it lived, and `hexe` has nothing to do with anything.
///
/// The cause is usually one of two, and the second is invisible from the word that failed. An alias
/// expands **before** the planner sees the pipeline, so `alias lines=tokei` turns
/// `seq | lines | length` into `seq | tokei | length` — no rows anywhere, and the name reported
/// missing is `length`, three stages from the mistake. Naming the shadowing alias is the whole point:
/// the vocabulary is disjoint from POSIX and coreutils, not from names somebody has already taken.
fn structured_verb_reason(env: &Environment, cmd_name: &str) -> Option<String> {
    if oslo_base::vocab::kind_of(cmd_name) != Some("verb") {
        return None;
    }
    let mut shadowed: Vec<&str> = env
        .get_aliases()
        .keys()
        .filter(|name| oslo_base::vocab::kind_of(name) == Some("verb"))
        .map(String::as_str)
        .collect();
    shadowed.sort_unstable();

    let mut reason =
        "a structured verb, not a command: it runs only where an earlier stage produces rows"
            .to_string();
    // **Every one of them, and no claim about which.** The word that failed is not the word that
    // was aliased away — that one is gone by the time anything here can look — so naming a single
    // culprit would be a guess, which is the failing this message exists to stop making.
    if !shadowed.is_empty() {
        reason.push_str(&format!(
            ". These aliases shadow verbs of the same name, which stops those stages being \
             planned: {}. Quote one to reach the verb (`\\{}`), or rename the alias",
            shadowed.join(", "),
            shadowed[0]
        ));
    }
    Some(reason)
}

/// Report a command word that could not be run, with the command's own redirections in force.
///
/// The diagnostic belongs to the *command*, not to the shell, so it goes wherever the command
/// pointed its stderr: `nosuchcommand 2>/dev/null` is silent in every shell, and that is the
/// shape of every feature probe written before `command -v` existed. oslo printed to the shell's
/// own stderr instead, so a script full of such probes filled the terminal with noise it had
/// explicitly asked to discard.
pub(super) fn report_unrunnable(
    env: &mut Environment,
    redirections: &[Redirection],
    cmd_name: &str,
    words: &[String],
    reason: &str,
    status: i32,
) -> Result<i32> {
    let mut guard = RedirectGuard::new();
    if let Err(e) = guard.apply(env, redirections) {
        // The redirection failed too. That is the failure the user has to fix first, and it is
        // the one bash reports here as well.
        return Ok(report_redirect_failure(&env.origin(), &e));
    }
    // **The commonest diagnostic in the shell**, and the last one to get a caret. The word is the
    // command itself, so the report points at the head of the line rather than at an operand —
    // which is right: `lsd` is not a bad argument to something, it is the thing that is not there.
    crate::env::complain_from(
        &env.origin(),
        words,
        cmd_name,
        &format!("{}: {reason}", oslo_base::shown::shown(cmd_name)),
        "no command of this name",
        Some(WHERE_IT_LOOKED),
    );
    Ok(status)
}
