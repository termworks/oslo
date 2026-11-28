//! `export` and `unset`: giving a name the export attribute, and taking a name away entirely.

use super::options;
use crate::env::announce::{Change, Scope, Source, announce};
use crate::env::builtins::declare::{self, Spec, print_matching};
use crate::env::origin_now;
use crate::env::scope::value::Attrs;
use crate::env::scope::{Environment, is_valid_identifier};
use oslo_base::error::{Result, ShellError};

const EXPORT_USAGE: &str = "usage: export [-fn] [name[=value] ...] or export -p";
const UNSET_USAGE: &str = "usage: unset [-f] [-v] [-n] [name ...]";

/// `export [-aAfnp] [name[=value] ...]`.
///
/// With no operands — and with `-p`, which POSIX makes the explicit spelling of the same thing —
/// this prints the exported variables as `declare -x` lines a shell can read back, sorted. `-a`
/// and `-A` make an assigned value an indexed or associative array, and narrow a listing to them.
pub fn builtin_export(env: &mut Environment, args: &[String]) -> Result<i32> {
    let written = declare::Written::take();
    let opts = match options::parse(args, "aAfnp") {
        Ok(o) => o,
        Err(letter) => return Err(options::invalid(args, "export", letter, EXPORT_USAGE)),
    };
    let operands = &args[opts.operands..];

    if opts.has('f') {
        return Ok(export_functions(env, operands, opts.has('n')));
    }
    if operands.is_empty() || opts.has('p') {
        if operands.is_empty() {
            print_matching(env, Some("export"), |_, var| {
                var.exported()
                    && (!opts.has('a') || var.indexed().is_some())
                    && (!opts.has('A') || var.assoc().is_some())
            });
        }
        return Ok(0);
    }

    let (on, off) = match opts.has('n') {
        true => (Attrs::default(), Attrs::EXPORT),
        false => (Attrs::EXPORT, Attrs::default()),
    };
    let spec = Spec {
        on,
        off,
        indexed: opts.has('a'),
        assoc: opts.has('A'),
        ..Spec::default()
    };
    // `-a` and `-A` shape an assigned value; a bare name keeps the value it has.
    let bare = Spec {
        indexed: false,
        assoc: false,
        ..spec.clone()
    };
    let mut status = 0;
    // A name `export` cannot accept is a *utility error*, not just a failed command, and POSIX
    // 2.8.1 ends a non-interactive shell over one because `export` is special. It is tracked
    // apart from `status` because the other ways this loop fails are not; the whole line is still
    // processed first, as bash sets the good names before it gives up.
    let mut bad_name = false;
    for (nth, arg) in operands.iter().enumerate() {
        let name = arg.split_once('=').map_or(arg.as_str(), |(n, _)| n);
        let name = name.strip_suffix('+').unwrap_or(name);
        if !is_valid_identifier(name) {
            super::not_an_identifier(args, "export", arg);
            status = 1;
            bad_name = true;
            continue;
        }
        let compound = written.operand(args, operands, nth);
        let scope = declare::Scope::Visible;
        let shape = if arg.contains('=') { &spec } else { &bare };
        if !declare::declare_one(env, "export", arg, shape, scope, compound)? {
            status = 1;
            continue;
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
        // Folded back to `Ok(1)` by `posix::resolve_builtin_result` outside POSIX mode.
        return Err(ShellError::utility_error(
            "export: not a valid identifier",
            1,
        ));
    }
    Ok(status)
}

/// `export -f name` hands a function to child shells; `export -nf name` takes it back.
fn export_functions(env: &mut Environment, names: &[String], remove: bool) -> i32 {
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
            continue;
        }
        match remove {
            true => env.unexport_function(name),
            false => {
                env.export_function(name);
            }
        }
    }
    status
}

/// `unset [-fvn] [name ...]`.
///
/// A read-only name is refused and keeps its value. `-n` removes a nameref itself rather than the
/// variable it names, and `name[subscript]` removes one element.
pub fn builtin_unset(env: &mut Environment, args: &[String]) -> Result<i32> {
    let opts = match options::parse(args, "fvn") {
        Ok(o) => o,
        Err(letter) => return Err(options::invalid(args, "unset", letter, UNSET_USAGE)),
    };
    if opts.has('f') && opts.has('v') {
        eprintln!(
            "{}unset: cannot simultaneously unset a function and a variable",
            origin_now()
        );
        return Ok(1);
    }

    let mut status = 0;
    // Whether any name was refused for being read-only. See the note where it is read.
    let mut refused = false;
    for name in &args[opts.operands..] {
        if !opts.has('f')
            && let Some(result) = crate::env::builtins::arrays::unset_element(env, name)
        {
            // A subscript whose arithmetic fails unsets nothing, silently, as in bash.
            match result {
                Err(oslo_base::error::ShellError::Discard(..)) | Ok(()) => {}
                Err(e) => {
                    eprintln!("{}unset: {}", origin_now(), e);
                    status = 1;
                }
            }
            continue;
        }
        // A name no variable can have may still be a function's, so only `-v` refuses it.
        if !is_valid_identifier(name) {
            if opts.has('v') || opts.has('n') {
                super::not_an_identifier(args, "unset", name);
                status = 1;
            } else if !env.is_function_readonly(name) {
                env.remove_function(name);
            }
            continue;
        }
        // With neither option, a name that is only a function names the function; when both a
        // variable and a function exist the variable goes first, as in bash.
        let target_function = opts.has('f')
            || (!opts.has('v')
                && !opts.has('n')
                && env.variable(name).is_none()
                && env.get_function(name).is_some());
        if target_function {
            if env.is_function_readonly(name) {
                crate::env::complain(
                    args,
                    name,
                    &format!("unset: {name}: cannot unset: readonly function"),
                    "readonly",
                    Some("`readonly -f` keeps a function for the life of the shell"),
                );
                status = 1;
                continue;
            }
            // `unset -f nosuchfn` is not an error in bash either.
            env.remove_function(name);
            continue;
        }
        let readonly = match opts.has('n') {
            true => env.variable(name).is_some_and(|v| v.readonly()),
            false => env.is_readonly(name),
        };
        if readonly {
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
        // The call stack's arrays stay for the life of the shell, as bash keeps them.
        if !opts.has('n') && crate::env::scope::is_call_stack_array(name) {
            crate::env::complain(
                args,
                name,
                &format!("unset: {name}: cannot unset"),
                "the shell keeps it",
                None,
            );
            status = 1;
            continue;
        }
        match opts.has('n') {
            true => env.unset_binding(name),
            false => env.unset_var(name),
        }
        announce(name, Change::Erased, Scope::Shell, Source::Local);
    }

    // **After the whole line, not at the name that failed.** `unset` is a special builtin, so a
    // read-only name ends a non-interactive shell in POSIX mode — but bash still unsets the other
    // names first. Outside POSIX `resolve_builtin_result` folds this back to the same status 1.
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
    /// A name no variable can have is refused under `-v`; without it, it may name a function, and
    /// bash answers 0 for one that names nothing.
    fn unset_of_an_invalid_name_fails_only_under_v() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_unset(&mut env, &words(&["unset", "-v", "a b"])).unwrap(),
            1
        );
        assert_eq!(
            builtin_unset(&mut env, &words(&["unset", "1x"])).unwrap(),
            0
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
