//! bash 5.3's function substitutions: `${ cmd; }` and `${| cmd; }`.
//!
//! Unlike `$(cmd)`, the body runs in the current shell: its assignments, `cd`s and function
//! definitions outlast it, and an `exit` in it ends the shell. `${ cmd; }` stands for what the
//! body printed; `${| cmd; }` leaves the body's output where it was going and stands for the value
//! the body left in `REPLY`, which is local to it.

use crate::env::Environment;
use crate::exec::compound::flush_stdout;
use crate::exec::pipeline::eval_command_list;
use nix::unistd::{close, dup, dup2};
use oslo_base::error::{Result, ShellError};
use std::io::{Read, Seek};
use std::os::fd::AsRawFd;

/// Run `body` in the current shell and answer with what the substitution stands for.
pub fn eval_function_substitution(
    env: &mut Environment,
    body: &str,
    reply: bool,
) -> Result<String> {
    let ast = crate::syntax::parse_with_aliases(body, !env.get_aliases().is_empty(), &|n| {
        env.get_alias(n).map(str::to_string)
    })?;
    if reply {
        return run_for_reply(env, &ast);
    }

    let io = |e: nix::Error| ShellError::ExecutionError(format!("function substitution: {e}"));
    let mut capture = tempfile::tempfile()?;
    flush_stdout();
    let saved = dup(1).map_err(io)?;
    dup2(capture.as_raw_fd(), 1).map_err(io)?;
    let result = eval_command_list(env, &ast);
    flush_stdout();
    dup2(saved, 1).map_err(io)?;
    let _ = close(saved);

    env.note_substitution_status(result?);
    let mut output = Vec::new();
    capture.rewind()?;
    capture.read_to_end(&mut output)?;
    Ok(crate::exec::substitution::captured_text(env, output))
}

/// `${| cmd; }`: run the body with a fresh `REPLY`, then put the old one back.
fn run_for_reply(env: &mut Environment, ast: &oslo_base::ast::CommandList) -> Result<String> {
    let outer = env.get_var("REPLY").map(str::to_string);
    let exported = env.is_exported("REPLY");
    env.unset_var("REPLY");
    let result = eval_command_list(env, ast);
    let value = env.get_var("REPLY").unwrap_or_default().to_string();
    match outer {
        Some(old) => {
            env.set_var("REPLY", &old, exported);
        }
        None => env.unset_var("REPLY"),
    }
    env.note_substitution_status(result?);
    Ok(value)
}
