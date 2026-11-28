//! `.` and `source`: run a file in this shell.

use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::{Result, ShellError};

/// The file `. name` reads: `name` itself when it holds a `/`, otherwise the first regular file
/// of that name on `$PATH` (with `sourcepath`, the default), and failing that the current
/// directory — except in POSIX mode, where a name not on `$PATH` is not found at all.
fn sourced_path(env: &Environment, name: &str) -> Option<String> {
    if name.contains('/') || !crate::env::builtins::shopt::on("sourcepath") {
        return Some(name.to_string());
    }
    let path = env.get_var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let candidate = format!("{dir}/{name}");
        if std::path::Path::new(&candidate).is_file() {
            return Some(candidate);
        }
    }
    (!env.posix()).then(|| name.to_string())
}

pub fn builtin_source(env: &mut Environment, args: &[String]) -> Result<i32> {
    if args.len() < 2 {
        eprintln!("{}source: filename argument required", origin_now());
        return Ok(1);
    }

    let Some(found) = sourced_path(env, &args[1]) else {
        crate::env::complain(
            args,
            &args[1],
            &format!(".: {}: file not found", args[1]),
            "not on $PATH",
            Some("POSIX mode does not look in the current directory for a bare name"),
        );
        return Err(ShellError::utility_error("source", 1));
    };
    let file_path = &found;
    let content = match oslo_base::lossless::read_file(oslo_base::lossless::to_os(file_path)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "oslo: source: {}: {}",
                oslo_base::shown::shown(file_path),
                oslo_base::error::reason(&e)
            );
            // `.` is a special builtin, so a file it cannot read ends a non-interactive shell in
            // POSIX mode — `bash --posix -c '. /nonexistent; echo ALIVE'` prints nothing. Outside
            // POSIX this folds back to status 1 and the script carries on, as it always did.
            return Err(ShellError::utility_error("source", 1));
        }
    };

    // **Not every sourced file is shell.** A Lua one used to reach the shell parser and come back
    // as a syntax error; `oslo script.lua` has always detected it, and this is the other entry
    // point applying the same rule. Asked before the nesting counter, because a Lua chunk does not
    // re-enter this evaluator.
    if let Some(status) = crate::sourced::run_if_not_shell(file_path, &content) {
        return Ok(status);
    }

    // A file that sources itself re-enters the parser and the evaluator once per level. The
    // counter is entered only after the file is known to be readable, so a missing file still
    // costs nothing, and exited on every path out so a `return` cannot leave it drifting.
    env.enter_nested_script()?;
    // **`. file a b` gives the file `$1` and `$2`.** Only when there are operands: bare `. file`
    // leaves the caller's parameters in place, as POSIX and bash do.
    let outer = (args.len() > 2).then(|| env.swap_positional(args[2..].to_vec()));
    let set_before = super::super::variables::positional_sets();
    // `$BASH_COMMAND` names the `.` again once the file is done, as it does after `eval`.
    let command = env.get_var("BASH_COMMAND").unwrap_or_default().to_string();
    let traps = env.enter_source_traps();
    // `$FUNCNAME`'s outermost entry, as bash spells it: a function called from a sourced file
    // reads `f source`. `eval` gets none, and bash gives it none either.
    env.enter_script_frame("source");
    // And the file itself, so a failure inside it names it rather than the script that sourced it.
    env.enter_source_file(file_path);
    let traced = crate::exec::simple::trace::Nested::enter();
    let parsed = crate::syntax::parse_with_aliases(&content, !env.get_aliases().is_empty(), &|n| {
        env.get_alias(n).map(str::to_string)
    });
    // As with `eval`: the lines before a syntax error run, and `source` ends there with status 2.
    let mut result = match parsed {
        Ok(ast) if !crate::exec::chunks::needs_chunks(env, &content) => {
            crate::exec::eval_command_unit(env, &ast)
        }
        _ => crate::exec::chunks::run_unit_chunks(env, "source", &content, &mut |_, e| {
            eprintln!("{}{}: {}", origin_now(), file_path, e)
        }),
    };
    drop(traced);
    if matches!(result, Ok(_) | Err(ShellError::Return(_)))
        && let Err(e) = super::super::process::run_return_trap(env)
    {
        result = Err(e);
    }
    if !super::super::process::in_trap() {
        env.set_bash_command(command);
    }
    env.exit_source_file();
    env.exit_script_frame();
    env.leave_trap_frame(traps);
    // Before the status is looked at, so a `return` from inside the file restores them too —
    // unless the file ran `set` to replace them at the top level, which bash keeps.
    let replaced = super::super::variables::positional_sets() != set_before && !env.in_function();
    if let Some(outer) = outer
        && !replaced
    {
        env.set_positional(outer);
    }
    env.exit_nested_script();

    // `return` ends a sourced script early and supplies its status.
    match result {
        Err(ShellError::Return(code)) => Ok(code),
        other => other,
    }
}
