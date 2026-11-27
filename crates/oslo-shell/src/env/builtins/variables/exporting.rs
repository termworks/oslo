//! `export` and `unset`: giving a name the export attribute, and taking a name away entirely.

use super::options;
use super::quoting::single_quoted;
use crate::env::announce::{Change, Scope, Source, announce};
use crate::env::origin_now;
use crate::env::scope::{Environment, is_valid_identifier};
use oslo_base::error::{Result, ShellError};

const EXPORT_USAGE: &str = "usage: export [-fnp] [name[=value] ...]";
const UNSET_USAGE: &str = "usage: unset [-fv] [name ...]";

/// `export [-fnp] [name[=value] ...]`.
///
/// With no operands — and with `-p`, which POSIX makes the explicit spelling of the same thing —
/// this prints the exported variables as assignments a shell can read back. Sorted, because a
/// listing whose order changes between two runs of the same script is not output, it is noise.
pub fn builtin_export(env: &mut Environment, args: &[String]) -> Result<i32> {
    let opts = match options::parse(args, "fnp") {
        Ok(o) => o,
        Err(letter) => return Err(options::invalid(args, "export", letter, EXPORT_USAGE)),
    };
    let operands = &args[opts.operands..];

    if operands.is_empty() {
        print_listing(env, None);
        return Ok(0);
    }
    if opts.has('p') {
        print_listing(env, Some(operands));
        return Ok(0);
    }
    if opts.has('f') {
        return Ok(export_functions(env, operands));
    }

    let mut status = 0;
    // A name `export` cannot accept is a *utility error*, not just a failed command, and POSIX
    // 2.8.1 ends a non-interactive shell over one because `export` is special. It is tracked
    // separately from `status` because the other ways this loop fails are not: `export -f
    // nosuchfn` reports 1 and `bash --posix` carries on. The whole line is still processed first —
    // bash sets the good names before it gives up.
    let mut bad_name = false;
    for arg in operands {
        let (name, value) = match arg.find('=') {
            Some(idx) => (&arg[..idx], Some(&arg[idx + 1..])),
            None => (arg.as_str(), None),
        };
        if !is_valid_identifier(name) {
            super::not_an_identifier(args, "export", arg);
            status = 1;
            bad_name = true;
            continue;
        }
        // `-n` still performs the assignment: `export -n x=1` sets x and leaves it unexported.
        let assigned = match value {
            Some(v) if opts.has('n') => env.set_var(name, v, false),
            Some(v) => env.set_var(name, v, true),
            None if opts.has('n') => true,
            None => env.export_var(name),
        };
        if !assigned {
            status = 1;
            continue;
        }
        if opts.has('n') && !unexport(env, name) {
            status = 1;
        }
        announce(
            name,
            Change::Set {
                exported: env.is_exported(name),
            },
            Scope::Shell,
            Source::Local,
        );
    }

    if bad_name {
        // Folded back to `Ok(1)` by `posix::resolve_builtin_result` outside POSIX mode, so this
        // is invisible to an ordinary shell — see [`crate::exec::simple::posix`].
        return Err(ShellError::utility_error(
            "export: not a valid identifier",
            1,
        ));
    }
    Ok(status)
}

/// Print exported variables as `export name='value'`, restricted to `only` when given.
///
/// Names that are not valid shell identifiers are skipped. `environ` can hold them — bash's
/// exported-function entries are called things like `BASH_FUNC_x%%` — but no shell can *parse*
/// an assignment to one, and a listing that cannot be read back is the bug this replaces.
fn print_listing(env: &Environment, only: Option<&[String]>) {
    let vars = env.get_exported_vars();
    let mut names: Vec<&String> = vars
        .keys()
        .filter(|name| is_valid_identifier(name))
        .filter(|name| only.is_none_or(|sel| sel.iter().any(|s| s == *name)))
        .collect();
    names.sort();
    for name in names {
        println!("export {}={}", name, single_quoted(&vars[name]));
    }
}

/// `export -f name` — check that `name` really is a function, which is all oslo can promise.
///
/// bash smuggles functions to children through `environ`; oslo has no such encoding, and does
/// not need one for its own subshells, which are forked and therefore already hold every
/// function the parent had. Validating the name is the honest remainder: `export -f nosuch`
/// must still fail the way it does everywhere else.
fn export_functions(env: &Environment, names: &[String]) -> i32 {
    let mut status = 0;
    for name in names {
        if env.get_function(name).is_none() {
            crate::env::complain(
                &crate::env::line("export", names),
                name,
                &format!("export: {name}: not a function"),
                "no function of that name",
                Some("`export -f` exports a shell function; `declare -F` lists the ones there are"),
            );
            status = 1;
        }
    }
    status
}

/// Clear the export attribute of `name`, keeping its value in the shell.
///
/// Done by removing and re-setting: [`Environment`] exposes no way to flip the flag in place,
/// and the removal is needed anyway to get the name out of `environ`, which is the whole point
/// of `-n`. A read-only variable cannot make the round trip, so it is refused rather than
/// silently destroyed.
fn unexport(env: &mut Environment, name: &str) -> bool {
    let Some(value) = env.get_var(name).map(str::to_string) else {
        return true;
    };
    if env.is_readonly(name) {
        crate::env::complain(
            &crate::env::line("export", std::slice::from_ref(&name.to_string())),
            name,
            &format!("export: {name}: readonly variable"),
            "readonly",
            Some("declared with `readonly`; the name keeps its value for the life of the shell"),
        );
        return false;
    }
    env.unset_var(name);
    env.set_var(name, &value, false)
}

/// `unset [-fv] [name ...]`.
///
/// The read-only check is the point of this rewrite. Without it `unset` dropped the *value* while
/// the read-only mark stayed behind, so the variable ended up permanently empty and permanently
/// unassignable — a state no shell has and no script can recover from.
pub fn builtin_unset(env: &mut Environment, args: &[String]) -> Result<i32> {
    let opts = match options::parse(args, "fv") {
        Ok(o) => o,
        Err(letter) => return Err(options::invalid(args, "unset", letter, UNSET_USAGE)),
    };
    if opts.has('f') && opts.has('v') {
        eprintln!(
            "{}unset: cannot simultaneously unset a function and a variable",
            origin_now()
        );
        return Ok(2);
    }

    let mut status = 0;
    // Whether any name was refused for being read-only. See the note where it is read.
    let mut refused = false;
    for name in &args[opts.operands..] {
        // `unset 'a[1]'` drops one element and leaves the rest of the array where it was.
        if let Some(result) = crate::env::builtins::arrays::unset_element(env, name) {
            if let Err(e) = result {
                eprintln!("{}unset: {}", origin_now(), e);
                status = 1;
            }
            continue;
        }
        if !is_valid_identifier(name) {
            super::not_an_identifier(args, "unset", name);
            status = 1;
            continue;
        }
        // With neither option, a name that is only a function names the function; when both a
        // variable and a function exist the variable goes first, as in bash.
        let target_function = opts.has('f')
            || (!opts.has('v') && env.get_var(name).is_none() && env.get_function(name).is_some());
        if target_function {
            // Not an error when there is no such function: bash exits 0 for
            // `unset -f nosuchfn`, so the removal's answer is deliberately discarded. What
            // matters is that this arm no longer falls through to the *variable* path.
            env.remove_function(name);
            continue;
        }
        if env.is_readonly(name) {
            crate::env::complain(
                args,
                name,
                &format!("unset: {name}: cannot unset: readonly variable"),
                "readonly",
                Some("a readonly name keeps its value for the life of the shell"),
            );
            status = 1;
            refused = true;
            continue;
        }
        env.unset_var(name);
        announce(name, Change::Erased, Scope::Shell, Source::Local);
    }

    // **After the whole line, not at the name that failed.** `unset` is a special builtin, so a
    // read-only name ends a non-interactive shell in POSIX mode — but bash still unsets the other
    // names first: `bash -c 'readonly r=1; x=2; unset r x; echo "[$x]"'` prints `[]`. Raising here
    // keeps both, and outside POSIX `resolve_builtin_result` folds it back to the same status 1.
    if refused {
        return Err(ShellError::utility_error("unset: readonly variable", 1));
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::super::tests::words;
    use super::{builtin_export, builtin_unset};
    use crate::env::builtins::builtin_readonly;
    use crate::env::scope::Environment;
    use oslo_base::error::ShellError;

    /// The bug this whole rewrite is named for: `-p` used to be taken as a variable name.
    #[test]
    fn options_are_not_mistaken_for_names() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_export(&mut env, &words(&["export", "-p"])).unwrap(),
            0
        );
        assert!(env.get_var("-p").is_none());
        assert!(env.get_var("p").is_none());

        assert_eq!(
            builtin_unset(&mut env, &words(&["unset", "-v"])).unwrap(),
            0
        );
        assert!(env.get_var("-v").is_none());
    }

    #[test]
    /// Worth 2 — and worth it as a *utility* error, because both builtins are special and
    /// `bash --posix -c 'export -z; echo alive'` prints no `alive`. Outside POSIX mode
    /// `crate::exec::simple::posix` folds it straight back to `Ok(2)`.
    fn an_unknown_option_is_a_usage_error() {
        let mut env = Environment::new();
        let err = builtin_export(&mut env, &words(&["export", "-z"])).expect_err("utility error");
        assert_eq!(err.survivable_utility_status(), Some(2));
        let err = builtin_unset(&mut env, &words(&["unset", "-z"])).expect_err("utility error");
        assert_eq!(err.survivable_utility_status(), Some(2));
    }

    /// Unsetting a read-only variable must fail *and change nothing*, or the name is left
    /// empty and unassignable at once.
    #[test]
    fn unset_refuses_a_readonly_variable() {
        let mut env = Environment::new();
        env.push_scope();
        assert!(env.set_local_var("RO_KEEP", "kept"));
        builtin_readonly(&mut env, &words(&["readonly", "RO_KEEP"])).unwrap();
        // **A utility error, not a status.** `unset` is a POSIX special builtin, so this has to
        // end a non-interactive shell in POSIX mode; `posix::resolve_builtin_result` is what
        // decides that, and folds it back to this same 1 for a shell that is not in POSIX mode.
        // Asserting the status here would have been asserting it below the layer that chooses.
        match builtin_unset(&mut env, &words(&["unset", "RO_KEEP"])) {
            Err(ShellError::UtilityError { status, fatal, .. }) => {
                assert_eq!((status, fatal), (1, 1));
            }
            other => panic!("expected a utility error, got {other:?}"),
        }
        assert_eq!(env.get_var("RO_KEEP"), Some("kept"));
        env.pop_scope();
    }

    /// `unset -f` must not fall through to the variable of the same name.
    #[test]
    fn unset_f_leaves_the_variable_alone() {
        let mut env = Environment::new();
        env.push_scope();
        assert!(env.set_local_var("SHADOW", "value"));
        assert_eq!(
            builtin_unset(&mut env, &words(&["unset", "-f", "SHADOW"])).unwrap(),
            0
        );
        assert_eq!(env.get_var("SHADOW"), Some("value"));
        env.pop_scope();
    }

    #[test]
    fn unset_of_an_invalid_name_fails() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_unset(&mut env, &words(&["unset", "a b"])).unwrap(),
            1
        );
        assert_eq!(
            builtin_unset(&mut env, &words(&["unset", "1x"])).unwrap(),
            1
        );
    }

    /// An empty command, for tests that only need *a* function body.
    fn nothing() -> oslo_base::ast::Command {
        oslo_base::ast::Command::Simple(oslo_base::ast::SimpleCommand {
            assignments: Vec::new(),
            words: Vec::new(),
            redirections: Vec::new(),
            unbraced: None,
        })
    }

    /// The whole point of `-f`: the function is gone afterwards.
    #[test]
    fn unset_f_removes_the_function() {
        let mut env = Environment::new();
        env.set_function("gone", nothing());
        assert_eq!(
            builtin_unset(&mut env, &words(&["unset", "-f", "gone"])).unwrap(),
            0
        );
        assert!(env.get_function("gone").is_none());
    }

    /// With no flag, a name that is only a function names the function — and a name that is
    /// neither is still not an error, as in bash.
    #[test]
    fn bare_unset_removes_a_function_and_forgives_a_missing_one() {
        let mut env = Environment::new();
        env.set_function("only_fn", nothing());
        assert_eq!(
            builtin_unset(&mut env, &words(&["unset", "only_fn"])).unwrap(),
            0
        );
        assert!(env.get_function("only_fn").is_none());
        assert_eq!(
            builtin_unset(&mut env, &words(&["unset", "-f", "no_such_fn"])).unwrap(),
            0
        );
    }

    /// `export -f` reports a name that is not a function, and accepts one that is.
    #[test]
    fn export_f_checks_the_name_is_a_function() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_export(&mut env, &words(&["export", "-f", "no_such_function"])).unwrap(),
            1
        );
    }
}
