//! Calling a shell function: its frame, its traps, its positional parameters and its scope.

use super::report_redirect_failure;
use crate::env::Environment;
use crate::exec::pipeline::eval_command;
use crate::exec::redirect::RedirectGuard;
use oslo_base::ast::{Command, Redirection};
use oslo_base::error::{Result, ShellError};

/// Call the function registered under `name`.
///
/// Looking it up here rather than taking a body keeps `autoload` from having to know how a
/// function is stored — it loads a file and asks for the name back.
pub(in crate::exec) fn call_named_function(
    env: &mut Environment,
    name: &str,
    words: &[String],
    redirections: &[Redirection],
) -> Result<i32> {
    let Some(body) = env.shared_function(name) else {
        return Ok(127);
    };
    call_function_command(env, &body, words, redirections)
}

/// Call a shell function, absorbing the control flow that must not escape it.
///
/// The call takes out the traps a body does not inherit (see
/// [`Environment::enter_function_traps`]), opens no loop for `break` to leave, and gives
/// `$LINENO` back to the caller's line when it returns.
pub(super) fn call_function_command(
    env: &mut Environment,
    body: &Command,
    words: &[String],
    redirections: &[Redirection],
) -> Result<i32> {
    // A function body is the program's last command only at the end of a substitution or job.
    let tail = super::oneshot::function_tail(env, body);
    super::oneshot::end();
    // Checked before anything is set up, so a refused call has nothing to unwind: `f() { f; }`
    // would otherwise overflow the stack.
    env.enter_function_named(words.first().map_or("", String::as_str))?;
    let traps = env.enter_function_traps(crate::env::builtins::in_debug_trap());
    let loops = env.take_loops();
    let res = call_function(env, body, words, redirections, tail);
    env.restore_loops(loops);
    env.leave_trap_frame(traps);
    env.exit_function();
    // Back at the top level, every substitution still open is closed.
    if !env.in_function() {
        crate::exec::procsub::finish_from(env, 0);
    }

    // **A call is judged on its own, whatever the body did.** The `set -e` exemption a compound
    // inherits from its last command stops at the function boundary: `set -e; f() { false && :; };
    // f` ends the shell in bash and dash even though the same `false && :` inline does not.
    crate::exec::pipeline::clear_status_exempt();

    match res {
        Err(ShellError::Return(code)) => Ok(code),
        Err(ShellError::Break(_)) | Err(ShellError::Continue(_)) => Ok(0),
        other => other,
    }
}

/// Run a function body with its own positional parameters and variable scope, then its RETURN
/// trap while that scope is still in place.
fn call_function(
    env: &mut Environment,
    body: &Command,
    words: &[String],
    redirections: &[Redirection],
    tail: Option<super::oneshot::Tail>,
) -> Result<i32> {
    let mut guard = RedirectGuard::new();
    if let Err(e) = guard.apply(env, redirections) {
        // The body does not run at all: `f < /nonexistent` is a failed command.
        return Ok(report_redirect_failure(&env.origin(), &e));
    }

    let old_pos = env.swap_positional(words[1..].to_vec());
    env.push_scope();
    // A DEBUG trap the body inherits (`-T`) fires once more as the body starts.
    crate::env::builtins::run_debug_trap(env);
    if let Some(kind) = tail {
        super::oneshot::tail_list_next(kind);
    }
    let mut res = eval_command(env, body);
    super::oneshot::take_tail_list();
    if matches!(res, Ok(_) | Err(ShellError::Return(_)))
        && let Err(e) = crate::env::builtins::run_return_trap(env)
    {
        res = Err(e);
    }
    env.pop_scope();
    env.set_positional(old_pos);
    res
}
