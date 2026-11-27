//! Functions kept one to a file and read the first time they are called.
//!
//! `~/.config/oslo/functions/NAME.sh` defines `NAME`, and is not read until something calls it.
//! fish's `functions/` directory, and the reason it is worth copying is arithmetic: a `conf.d`
//! snippet that defines twenty functions costs twenty function definitions on **every** shell
//! start, including the hundred short-lived ones a build spawns. An autoloaded one costs a
//! `stat(2)` on the call that needs it and nothing at all otherwise.
//!
//! # It can never shadow anything
//!
//! The lookup happens **after** the PATH search has already failed. A file named `ls.sh` is
//! therefore dead: `ls` resolves to the program long before this is reached. That is deliberate
//! and it is the conservative half of the trade — fish lets an autoloaded function override a
//! command, which is useful about once and surprising the rest of the time, and a shell that
//! promises scripts see POSIX behaviour cannot have a file on disk quietly redefining `test`.
//!
//! So: autoloading adds names, it never changes existing ones.
//!
//! # A file that does not define its function
//!
//! Reported once, as a mistake, rather than falling through to "command not found" — because the
//! file plainly exists and the user plainly meant it to work, and the useful message names the
//! discrepancy instead of denying the file was there.

use crate::env::Environment;
use oslo_base::error::Result;
use std::path::PathBuf;

/// Where `name` would be defined, if anywhere.
///
/// `$XDG_CONFIG_HOME` then `$HOME/.config`, matching where the config itself is looked for. A name
/// that is not a plain filename is refused outright: `../../etc/passwd` must not be reachable by
/// typing it as a command.
pub(crate) fn path_for(env: &Environment, name: &str) -> Option<PathBuf> {
    if !loadable(name) {
        return None;
    }
    let file = directory(env)?.join(format!("{name}.sh"));
    file.is_file().then_some(file)
}

/// Whether `name` is one this module would ever load.
///
/// The rule lives here, once, because two places need it and they had drifted: `names` advertised
/// every `*.sh` entry in the directory — including a *directory* called `x.sh`, and stems this
/// refuses — so the prompt completed and coloured names that then exited 127.
pub(crate) fn loadable(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.starts_with('.')
}

/// The directory autoloaded functions are read from, whether or not it exists.
///
/// Split out because the prompt needs the *set* of names, not one of them: an autoloaded function
/// is a command that runs, and until the interface could enumerate them every one of them was
/// painted as a command that does not exist. See [`crate::names`].
pub(crate) fn directory(env: &Environment) -> Option<PathBuf> {
    let base = env
        .get_var("XDG_CONFIG_HOME")
        .map(str::to_string)
        .or_else(|| std::env::var("XDG_CONFIG_HOME").ok())
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let home = env
                .get_var("HOME")
                .map(str::to_string)
                .or_else(|| std::env::var("HOME").ok())
                .filter(|h| !h.is_empty())?;
            Some(PathBuf::from(home).join(".config"))
        })?;
    Some(base.join("oslo/functions"))
}

/// Read the file defining `name` and answer whether it is now defined.
///
/// `None` means there was no such file and the caller should carry on reporting the command as
/// missing. `Some(false)` means the file existed and did not define the function — already
/// reported, and the caller should stop rather than also saying "command not found".
pub fn load(env: &mut Environment, name: &str) -> Option<bool> {
    // Already defined means we are being asked a second time from inside the file itself, which
    // would otherwise recurse: the file calls the function, the function is not yet defined
    // because the file has not finished, and we read the file again.
    if env.get_function(name).is_some() {
        return Some(true);
    }
    let path = path_for(env, name)?;
    let text = path.to_str()?.to_string();

    // Sourced through the ordinary builtin, so an autoloaded file behaves exactly like one the
    // user typed `source` on: same parser, same alias expansion, same nesting limit.
    let status = crate::env::builtins::builtin_source(env, &["source".to_string(), text]).ok()?;
    if status != 0 {
        // `source` has already said what was wrong with the file.
        return Some(false);
    }
    if env.get_function(name).is_some() {
        return Some(true);
    }
    eprintln!(
        "oslo: {}: does not define {name}",
        oslo_ui::marks::path(&path.display().to_string())
    );
    Some(false)
}

/// Load and then call `name`, for the command path.
///
/// Returns `None` when there was nothing to load, which is the caller's signal to carry on with
/// `command-not-found` and the eventual 127.
pub fn try_call(
    env: &mut Environment,
    name: &str,
    words: &[String],
    redirections: &[oslo_base::ast::Redirection],
) -> Option<Result<i32>> {
    match load(env, name)? {
        true => Some(super::call::call_named_function(
            env,
            name,
            words,
            redirections,
        )),
        // The file was there and was wrong. Status 127 still, because the command did not run —
        // but without a second message contradicting the one already printed.
        false => Some(Ok(127)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name that could escape the directory is not a function name.
    ///
    /// Without this, `../../../bin/x` typed as a command would read a file outside the functions
    /// directory, and a leading dot would reach the hidden files beside it.
    #[test]
    fn a_name_that_is_not_a_plain_filename_is_refused() {
        let env = Environment::new();
        for name in ["", "../evil", "a/b", ".hidden", "./x"] {
            assert!(
                path_for(&env, name).is_none(),
                "{name:?} must not resolve to a path"
            );
        }
    }

    /// The happy path, end to end: a file in the directory defines the function, and calling the
    /// name finds it without anything having sourced the file first.
    #[test]
    fn a_function_is_read_the_first_time_it_is_called() {
        let dir = tempfile::tempdir().expect("tempdir");
        let functions = dir.path().join("oslo/functions");
        std::fs::create_dir_all(&functions).expect("mkdir");
        std::fs::write(functions.join("greet.sh"), "greet() { echo hello; }\n").expect("write");

        let mut env = Environment::new();
        env.set_var("XDG_CONFIG_HOME", dir.path().to_str().expect("utf8"), false);

        assert!(env.get_function("greet").is_none(), "not defined yet");
        assert_eq!(load(&mut env, "greet"), Some(true));
        assert!(env.get_function("greet").is_some(), "now defined");
    }

    /// A name with no file is not an error here — it is the ordinary case, and the caller goes on
    /// to report "command not found".
    #[test]
    fn a_name_with_no_file_loads_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("oslo/functions")).expect("mkdir");
        let mut env = Environment::new();
        env.set_var("XDG_CONFIG_HOME", dir.path().to_str().expect("utf8"), false);
        assert_eq!(load(&mut env, "nosuch"), None);
    }

    /// A file that does not define what its name promises is a mistake worth naming, and must not
    /// leave the function looking loadable — a second call would read it again.
    #[test]
    fn a_file_that_defines_nothing_is_reported_and_not_retried_as_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        let functions = dir.path().join("oslo/functions");
        std::fs::create_dir_all(&functions).expect("mkdir");
        std::fs::write(functions.join("empty.sh"), "# nothing here\n").expect("write");

        let mut env = Environment::new();
        env.set_var("XDG_CONFIG_HOME", dir.path().to_str().expect("utf8"), false);
        assert_eq!(load(&mut env, "empty"), Some(false));
        assert!(env.get_function("empty").is_none());
    }

    /// A file whose function calls itself must not re-read the file on the inner call.
    #[test]
    fn an_already_defined_function_is_not_read_again() {
        let mut env = Environment::new();
        crate::exec::eval_command_list(
            &mut env,
            &crate::syntax::parse_bash_script("here() { echo x; }").expect("parse"),
        )
        .expect("define");
        // No `XDG_CONFIG_HOME` and no file: the answer comes from the function already existing,
        // which is what stops the recursion.
        assert_eq!(load(&mut env, "here"), Some(true));
    }
}
