//! `builtin` — force resolution to a shell builtin.
//!
//! The counterpart to `command`: where `command` steps around functions to reach the builtin or
//! the binary, `builtin` steps around *both* aliases and functions and insists on the builtin.
//! That is what makes a recursive wrapper writable — `cd() { builtin cd "$@" && ls; }` calls the
//! real `cd`, not itself.

use crate::env::scope::Environment;
use oslo_base::error::Result;

/// `builtin [name [args…]]`.
pub fn builtin_builtin(env: &mut Environment, args: &[String]) -> Result<i32> {
    let Some(name) = args.get(1) else {
        // bash treats a bare `builtin` as a no-op success rather than a usage error.
        return Ok(0);
    };
    // `builtin` takes no options; `--` ends them.
    if name == "--" {
        return builtin_builtin(env, &args[1..]);
    }
    if name.len() > 1 && name.starts_with('-') {
        crate::env::complain_with_usage(
            args,
            name,
            &format!("builtin: {name}: invalid option"),
            "builtin takes no options",
            "builtin: usage: builtin [shell-builtin [arg ...]]",
        );
        return Ok(2);
    }

    // The registry is consulted directly, so neither an alias (already expanded by the caller,
    // but never re-applied here) nor a function of the same name can intercept the call. `args`
    // is shifted by one so the builtin sees its own name as argv[0].
    match env.exec_custom_builtin(name, &args[1..]) {
        // As with `command`: reaching the builtin this way strips it of the specialness that
        // makes a utility error fatal, so the error folds back to its status.
        // `bash --posix -c 'builtin export BAD-NAME=1; echo alive'` prints `alive`.
        Some(Err(e)) => match e.survivable_utility_status() {
            Some(status) => Ok(status),
            None => Err(e),
        },
        Some(result) => result,
        None => {
            crate::env::complain(
                args,
                name,
                &format!("builtin: {name}: not a shell builtin"),
                "not a builtin",
                Some(
                    "`builtin NAME` runs the shell builtin NAME, bypassing any function of that name",
                ),
            );
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::builtin_builtin;
    use crate::env::Environment;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    /// The forced builtin still gets its own name as argv\[0\], because builtins that print a
    /// diagnostic name themselves from it.
    #[test]
    fn a_registered_builtin_runs() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_builtin(&mut env, &argv(&["builtin", "true"])).unwrap(),
            0
        );
        assert_eq!(
            builtin_builtin(&mut env, &argv(&["builtin", "false"])).unwrap(),
            1
        );
    }

    /// A function of the same name must not be reached: that is the point of the builtin.
    #[test]
    fn a_shadowing_function_is_ignored() {
        let mut env = Environment::new();
        let script = crate::syntax::parse_bash_script("true() { echo shadowed; }").expect("parse");
        crate::exec::eval_command_list(&mut env, &script).expect("define");
        assert!(env.get_function("true").is_some());
        assert_eq!(
            builtin_builtin(&mut env, &argv(&["builtin", "true"])).unwrap(),
            0
        );
    }

    #[test]
    fn an_external_command_is_refused() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_builtin(&mut env, &argv(&["builtin", "ls"])).unwrap(),
            1
        );
    }

    #[test]
    fn a_bare_invocation_succeeds() {
        let mut env = Environment::new();
        assert_eq!(builtin_builtin(&mut env, &argv(&["builtin"])).unwrap(), 0);
    }
}
