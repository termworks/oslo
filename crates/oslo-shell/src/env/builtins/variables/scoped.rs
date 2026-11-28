//! `readonly`: the attribute no later assignment can take back.
//!
//! `local` lives with `declare`, whose engine it shares; so does this builtin's assignment half.

use super::options;
use crate::env::builtins::declare::{Scope, Spec, Written, declare_one, print_matching};
use crate::env::scope::value::Attrs;
use crate::env::scope::{Environment, is_valid_identifier};
use oslo_base::error::Result;

const READONLY_USAGE: &str = "usage: readonly [-aAf] [name[=value] ...] or readonly -p";

/// `readonly [-aAfp] [name[=value] ...]`.
pub fn builtin_readonly(env: &mut Environment, args: &[String]) -> Result<i32> {
    let written = Written::take();
    let opts = match options::parse(args, "aAfp") {
        Ok(o) => o,
        Err(letter) => return Err(options::invalid(args, "readonly", letter, READONLY_USAGE)),
    };
    let operands = &args[opts.operands..];

    if opts.has('f') {
        return Ok(readonly_functions(env, operands, opts.has('p')));
    }
    if operands.is_empty() || opts.has('p') {
        print_matching(env, Some("readonly"), |_, var| {
            var.readonly()
                && (!opts.has('a') || var.indexed().is_some())
                && (!opts.has('A') || var.assoc().is_some())
        });
        return Ok(0);
    }

    let spec = Spec {
        on: Attrs::READONLY,
        indexed: opts.has('a'),
        assoc: opts.has('A'),
        ..Spec::default()
    };
    let mut status = 0;
    // `readonly` is a special builtin, so a name it cannot accept ends a POSIX-mode shell — see
    // the identical note in [`super::exporting::builtin_export`].
    let mut bad_name = false;
    for (nth, arg) in operands.iter().enumerate() {
        let name = arg.split_once('=').map_or(arg.as_str(), |(n, _)| n);
        let name = name.strip_suffix('+').unwrap_or(name);
        if !is_valid_identifier(name) {
            super::not_an_identifier(args, "readonly", arg);
            status = 1;
            bad_name = true;
            continue;
        }
        let compound = written.operand(args, operands, nth);
        if !declare_one(env, "readonly", arg, &spec, Scope::Visible, compound)? {
            status = 1;
        }
    }

    if bad_name {
        return Err(oslo_base::error::ShellError::utility_error(
            "readonly: not a valid identifier",
            1,
        ));
    }
    Ok(status)
}

/// `readonly -f [name ...]`: mark functions, or list the marked ones with their definitions.
fn readonly_functions(env: &mut Environment, names: &[String], print: bool) -> i32 {
    if names.is_empty() || print {
        let mut marked: Vec<String> = env
            .get_functions()
            .keys()
            .filter(|name| env.is_function_readonly(name))
            .cloned()
            .collect();
        marked.sort();
        for name in marked {
            if let Some(body) = env.get_function(&name) {
                println!("{}", crate::env::builtins::format_function(&name, body));
            }
            let flags = match env.is_function_exported(&name) {
                true => "-frx",
                false => "-fr",
            };
            println!("declare {flags} {name}");
        }
        return 0;
    }
    let mut status = 0;
    for name in names {
        if !env.set_function_readonly(name) {
            crate::env::complain(
                &crate::env::line("readonly", names),
                name,
                &format!("readonly: {name}: not a function"),
                "no function of that name",
                Some("`declare -F` lists the functions there are"),
            );
            status = 1;
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::super::tests::words;
    use super::builtin_readonly;
    use crate::env::scope::Environment;

    #[test]
    fn readonly_of_an_invalid_name_fails() {
        let mut env = Environment::new();
        let err = builtin_readonly(&mut env, &words(&["readonly", "=1"])).expect_err("fails");
        assert_eq!(err.survivable_utility_status(), Some(1));
        assert!(!env.is_readonly(""));
    }

    /// A refused assignment must not leave the name read-only.
    #[test]
    fn readonly_with_a_rejected_value_does_not_lock_the_name() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_readonly(&mut env, &words(&["readonly", "RO_NUL=a\0b"])).unwrap(),
            1
        );
        assert!(!env.is_readonly("RO_NUL"));
        assert_eq!(
            builtin_readonly(&mut env, &words(&["readonly", "RO_OK=1"])).unwrap(),
            0
        );
        assert!(env.is_readonly("RO_OK"));
    }

    #[test]
    fn readonly_p_lists_instead_of_declaring() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_readonly(&mut env, &words(&["readonly", "-p"])).unwrap(),
            0
        );
        assert!(!env.is_readonly("-p"));
    }

    #[test]
    fn readonly_makes_arrays_too() {
        let mut env = Environment::new();
        builtin_readonly(&mut env, &words(&["readonly", "-a", "RO_A=(1 2)"])).unwrap();
        assert!(env.is_readonly("RO_A"));
        assert_eq!(env.get_array("RO_A").unwrap().len(), 2);
    }
}
