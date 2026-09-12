//! `command` — run a command with shell functions out of the way, or describe one.
//!
//! The escape hatch that makes it possible to write a wrapper at all: `ls() { command ls -F
//! "$@"; }` only terminates because `command` skips the function lookup. `-v` and `-V` reuse the
//! same lookup to answer "what would run?" without running it, which is what every
//! `if command -v foo >/dev/null` feature probe in the wild is asking.

use crate::env::builtins::control::is_keyword;
use crate::env::builtins::spawn::{NOT_FOUND, resolve_program, run_external};
use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::path::PathBuf;

/// The `PATH` `-p` substitutes: a value guaranteed to find the standard utilities even when the
/// caller's `PATH` is empty or hostile.
const DEFAULT_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    /// No `-v`/`-V`: actually run the command.
    Run,
    /// `-v`: print what would run, one word per name.
    Terse,
    /// `-V`: print a human-readable description.
    Verbose,
}

/// `command [-p] [-v|-V] name [args…]`.
pub fn builtin_command(env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut mode = Mode::Run;
    let mut default_path = false;

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if arg.len() < 2 || !arg.starts_with('-') {
            break;
        }
        for c in arg[1..].chars() {
            match c {
                'p' => default_path = true,
                'v' => mode = Mode::Terse,
                'V' => mode = Mode::Verbose,
                other => {
                    crate::env::complain_option(
                        args,
                        other,
                        &format!("command: -{other}: invalid option"),
                        "command: usage: command [-pVv] command [arg ...]",
                    );
                    return Ok(2);
                }
            }
        }
        i += 1;
    }

    let operands = &args[i.min(args.len())..];
    match mode {
        Mode::Run => run(env, operands, default_path),
        _ => describe(env, operands, mode, default_path),
    }
}

/// Run `operands` as a command, consulting builtins and `PATH` but never shell functions.
fn run(env: &mut Environment, operands: &[String], default_path: bool) -> Result<i32> {
    let Some(name) = operands.first() else {
        // `command` with nothing to run is not an error, just nothing to do.
        return Ok(0);
    };

    // Deliberately *not* `env.get_function` first: skipping the function lookup is the entire
    // point of this builtin.
    if let Some(result) = env.exec_custom_builtin(name, operands) {
        // POSIX 2.9.1.1: `command` makes a special builtin behave as a regular one, which is
        // precisely the property that would have ended a POSIX-mode shell over a utility error.
        // `bash --posix -c 'command export BAD-NAME=1; echo alive'` prints `alive`.
        return match result {
            Err(e) => match e.survivable_utility_status() {
                Some(status) => Ok(status),
                None => Err(e),
            },
            ok => ok,
        };
    }

    match lookup_program(name, default_path) {
        Some(path) => run_external(&path, operands, name),
        None => {
            crate::env::complain(
                &crate::env::line("command", operands),
                name,
                &format!("{}: command not found", oslo_base::shown::shown(name)),
                "no command of this name",
                Some(
                    "`command` looks at builtins and $PATH only; an alias or function of this name is skipped on purpose",
                ),
            );
            Ok(NOT_FOUND)
        }
    }
}

/// Answer `-v`/`-V` for each name; status 1 if any name has no answer.
fn describe(
    env: &mut Environment,
    operands: &[String],
    mode: Mode,
    default_path: bool,
) -> Result<i32> {
    let mut status = 0;
    for name in operands {
        let described = match mode {
            Mode::Verbose => verbose_description(env, name, default_path),
            _ => terse_description(env, name, default_path),
        };
        match described {
            Some(line) => println!("{}", line),
            None => {
                // `-v` is silent on failure: every `if command -v foo >/dev/null 2>&1` probe in
                // the wild relies on the status alone, and bash prints nothing there either.
                if mode == Mode::Verbose {
                    crate::env::complain(
                        &crate::env::line("command", operands),
                        name,
                        &format!("command: {}: not found", oslo_base::shown::shown(name)),
                        "no command of this name",
                        Some(
                            "looked at builtins and $PATH; `command` skips aliases and functions on purpose",
                        ),
                    );
                }
                status = 1;
            }
        }
    }
    Ok(status)
}

/// What `command -v name` prints: a single word suitable for reuse as a command.
fn terse_description(env: &Environment, name: &str, default_path: bool) -> Option<String> {
    if name.contains('/') {
        return executable_path(name).map(|p| p.display().to_string());
    }
    if let Some(body) = env.get_alias(name) {
        return Some(format!("alias {}='{}'", name, body));
    }
    // A reserved word is not a builtin and has no path, but `command -v if` still has to print
    // `if` — POSIX lists reserved words among what `-v` reports, and the probe that noticed this
    // was missing (modernish's) treats its absence as fatal rather than as a missing feature.
    if is_keyword(name) || env.get_function(name).is_some() || env.is_builtin(name) {
        return Some(name.to_string());
    }
    if let Some(path) = lookup_program(name, default_path) {
        return Some(path.display().to_string());
    }
    // A stored macro is found after `$PATH` and has no path to print, so it answers with its own
    // name — the same word a function answers with, and one this shell can run.
    if crate::exec::stored::kind_of(name).is_some() {
        return Some(name.to_string());
    }
    // And the rest of what runs without `$PATH` knowing: a structured verb, a registered tool, a
    // function still in the autoload directory. `command -v X >/dev/null || …` is the commonest
    // probe there is, and answering "no" for a name this shell will happily run makes every one of
    // them wrong. Same word, for the same reason as the macro above.
    runs_without_path(env, name).then(|| name.to_string())
}

/// Whether the shell can run `name` even though `$PATH` cannot account for it.
///
/// Vocab first, because the prompt keeps that map anyway; then the autoload directory directly,
/// because a `-c` shell never calls `names::refresh` and so has an empty vocab while still being
/// perfectly able to load the file.
fn runs_without_path(env: &Environment, name: &str) -> bool {
    oslo_base::vocab::kind_of(name).is_some()
        || crate::exec::simple::autoload::path_for(env, name).is_some()
}

/// What `command -V name` prints: a sentence naming the kind of thing `name` is.
fn verbose_description(env: &Environment, name: &str, default_path: bool) -> Option<String> {
    if name.contains('/') {
        return executable_path(name).map(|p| format!("{} is {}", name, p.display()));
    }
    if let Some(body) = env.get_alias(name) {
        return Some(format!("{} is aliased to `{}'", name, body));
    }
    if is_keyword(name) {
        return Some(format!("{} is a shell keyword", name));
    }
    if env.get_function(name).is_some() {
        return Some(format!("{} is a function", name));
    }
    if env.is_builtin(name) {
        return Some(format!("{} is a shell builtin", name));
    }
    if let Some(path) = lookup_program(name, default_path) {
        return Some(format!("{} is {}", name, path.display()));
    }
    if let Some(kind) = crate::exec::stored::kind_of(name) {
        return Some(format!("{} is {}", name, stored_as(kind)));
    }
    match oslo_base::vocab::kind_of(name) {
        Some(kind) => Some(format!("{} is a shell {}", name, kind)),
        None => crate::exec::simple::autoload::path_for(env, name)
            .map(|_| format!("{} is a function", name)),
    }
}

/// How a stored macro is named to someone reading `-V` or `type`.
fn stored_as(kind: oslo_base::macros::Kind) -> &'static str {
    match kind {
        oslo_base::macros::Kind::Func => "a stored function",
        _ => "a stored script",
    }
}

/// Resolve a bare command word, honouring `-p`'s substitute `PATH`.
fn lookup_program(name: &str, default_path: bool) -> Option<PathBuf> {
    if !default_path || name.contains('/') {
        return resolve_program(name);
    }
    // `-p` searches a different `$PATH`, not a different question: a hidden name is hidden here
    // too. See `oslo_base::command`.
    if oslo_base::command::hidden(name) {
        return None;
    }
    let cwd = std::env::current_dir().ok()?;
    which::which_in(name, Some(DEFAULT_PATH), cwd).ok()
}

/// A path operand only counts as a command if it is a file the shell could actually execute —
/// `command -v /etc/passwd` finds a perfectly readable file and still answers "no".
fn executable_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = resolve_program(name)?;
    let mode = path.metadata().ok()?.permissions().mode();
    (mode & 0o111 != 0).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::{terse_description, verbose_description};
    use crate::env::Environment;

    #[test]
    fn a_builtin_reports_itself() {
        let env = Environment::new();
        assert_eq!(
            terse_description(&env, "echo", false).as_deref(),
            Some("echo")
        );
        assert_eq!(
            verbose_description(&env, "echo", false).as_deref(),
            Some("echo is a shell builtin")
        );
    }

    #[test]
    fn a_path_operand_is_reported_as_given() {
        let env = Environment::new();
        assert_eq!(
            terse_description(&env, "/bin/sh", false).as_deref(),
            Some("/bin/sh")
        );
    }

    /// A readable non-executable file is not a command, however much it looks like a path.
    #[test]
    fn a_non_executable_path_is_not_a_command() {
        let env = Environment::new();
        // Unit tests run in the crate root, so this file is always there and never executable.
        assert_eq!(terse_description(&env, "./Cargo.toml", false), None);
    }

    #[test]
    fn an_unknown_name_has_no_description() {
        let env = Environment::new();
        assert_eq!(terse_description(&env, "no-such-command-xyz", false), None);
        assert_eq!(
            verbose_description(&env, "no-such-command-xyz", false),
            None
        );
    }

    #[test]
    fn an_alias_is_reported_in_a_reusable_form() {
        let mut env = Environment::new();
        env.set_alias("oslov", "ls -la");
        assert_eq!(
            terse_description(&env, "oslov", false).as_deref(),
            Some("alias oslov='ls -la'")
        );
    }
}
