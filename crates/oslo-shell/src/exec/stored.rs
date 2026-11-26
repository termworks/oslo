//! Running a stored function — the macro kind that has no file and runs in this shell.
//!
//! # Found after `$PATH`, never before it
//!
//! Exactly the rule `super::simple::autoload` states for `functions/*.sh`, and for its reason: a
//! shell that promises a script sees POSIX behaviour cannot have a *database row* quietly redefining
//! `test`. So this is reached only once the `$PATH` search has already failed, which also means it
//! costs a database open on a line that was going to fail anyway and nothing at all otherwise.
//!
//! A stored *script* is not here: it is a file in `oslo_base::macros::scripts::directory`, and
//! dispatch finds it on `$PATH` like any other program.

use crate::env::Environment;
use oslo_base::macros::{self, Kind};

/// Run the stored function `words[0]` names, with the redirections applied.
///
/// `None` when nothing is stored under that name, which is what lets the caller carry on to
/// "command not found".
pub(super) fn try_call(
    env: &mut Environment,
    name: &str,
    words: &[String],
    redirections: &[oslo_base::ast::Redirection],
) -> Option<oslo_base::error::Result<i32>> {
    let body = lookup(name)?;

    // The same guard a function call uses, and for its reason: a redirection that cannot be set up
    // fails the command rather than running it with the shell's own streams.
    let mut guard = crate::exec::redirect::RedirectGuard::new();
    if guard.apply(env, redirections).is_err() {
        return Some(Ok(1));
    }
    let args: Vec<String> = words.iter().skip(1).cloned().collect();
    Some(Ok(function(env, &body, name, &args)))
}

/// The body of the stored function `name` once the `$PATH` search has failed, or `None`.
///
/// The whole rule, in the one place that can be asked without running anything: `type` and
/// `command -v` exist to answer "what would run?", and a `type` that disagrees with dispatch is
/// worse than no `type` at all.
fn lookup(name: &str) -> Option<String> {
    // Nothing is stored at all in the overwhelming case, and finding that out must not cost a
    // database open on every failed command. Whether the file exists is what a `stat(2)` answers.
    if !macros::database().is_some_and(|path| path.exists()) {
        return None;
    }
    let store = macros::open().ok()?;
    // **Off is off here too**, read at the moment of the call rather than applied at startup.
    let entry = macros::get(&store, Kind::Func, name).filter(|entry| entry.active)?;
    drop(store);
    if macros::live::session::is_off(&oslo_base::track::session::id(), name) {
        return None;
    }
    Some(entry.body)
}

/// Whether `name` would run as a stored function — what `type` and `command -v` report.
pub fn is_function(name: &str) -> bool {
    lookup(name).is_some()
}

/// Run the stored function or script `name` with `args`, for something that is not oslo.
///
/// `None` when nothing runnable is stored under the name, which is what lets the caller report it.
pub fn run_named(env: &mut Environment, name: &str, args: &[String]) -> Option<i32> {
    let store = macros::open().ok()?;
    if let Some(entry) = macros::get(&store, Kind::Func, name).filter(|entry| entry.active) {
        drop(store);
        return Some(function(env, &entry.body, name, args));
    }
    let script = macros::scripts::get(name).filter(|entry| entry.active)?;
    let path = macros::scripts::directory()?.join(&script.name);
    Some(
        match std::process::Command::new(&path).args(args).status() {
            Ok(status) => status.code().unwrap_or(128),
            Err(e) => {
                eprintln!("oslo: {name}: {}", oslo_base::error::reason(&e));
                126
            }
        },
    )
}

/// A stored function: the shell's own, run in this shell.
///
/// **In-process, not exec'd.** A function is expected to be able to `cd`, set a variable and change
/// the shell it was called from; running it in a child would make it a script that happens to have
/// no file. The positional parameters are set to `args` and put back afterwards.
fn function(env: &mut Environment, body: &str, name: &str, args: &[String]) -> i32 {
    // `swap_positional` rather than read-then-write: it is what a function call already uses, and
    // both halves are moves rather than a copy of every parameter twice.
    let saved = env.swap_positional(args.to_vec());
    let saved_name = std::mem::replace(&mut env.shell_name, name.to_string());

    let status = match crate::syntax::parse_bash_script(body) {
        Ok(program) => match crate::exec::eval_command_list(env, &program) {
            Ok(status) => status,
            Err(error) => {
                if let Some(code) = error.control_flow_status() {
                    code
                } else {
                    eprintln!("oslo: {name}: {error}");
                    1
                }
            }
        },
        Err(error) => {
            eprintln!("oslo: {name}: {error}");
            2
        }
    };

    env.swap_positional(saved);
    env.shell_name = saved_name;
    status
}
