//! `local` and `readonly`: the two builtins that attach a *scope* or an *attribute* to a name.

use super::options;
use super::quoting::single_quoted;
use crate::env::builtins::arrays::array_elements;
use crate::env::origin_now;
use crate::env::scope::{Environment, array_literal_body, is_valid_identifier};
use oslo_base::error::Result;

const LOCAL_USAGE: &str = "usage: local [-airx] name[=value] ...";
const READONLY_USAGE: &str = "usage: readonly [-p] [name[=value] ...]";

/// Split `name=value` into its halves; a bare name has no value, which is not the same as an
/// empty one (`local x` leaves an existing `x` visible, `local x=` blanks it).
fn split_assignment(arg: &str) -> (&str, Option<&str>) {
    match arg.find('=') {
        Some(idx) => (&arg[..idx], Some(&arg[idx + 1..])),
        None => (arg, None),
    }
}

/// `local [-airx] name[=value] ...`.
///
/// The options are parsed rather than assigned to: `local -r x=1` used to create a variable
/// literally called `-r` and then a second one called `x`.
///
/// `-i` (integer) is accepted and otherwise ignored — oslo keeps no attribute table, so there is
/// nothing to record and arithmetic on assignment is not implemented. Rejecting it outright
/// would break scripts for a declaration that is only ever an optimisation hint.
pub fn builtin_local(env: &mut Environment, args: &[String]) -> Result<i32> {
    // Outside a function there is no frame to pop, so the "local" would be a global that outlives
    // the line that declared it — the opposite of what was asked for. Refusing is what bash does,
    // and silence here is how a script ends up with a leaked global it never sees.
    if !env.in_function() {
        eprintln!("{}local: can only be used in a function", origin_now());
        return Ok(1);
    }

    let opts = match options::parse(args, "airx") {
        Ok(o) => o,
        Err(letter) => return Err(options::invalid(args, "local", letter, LOCAL_USAGE)),
    };

    let mut status = 0;
    for arg in &args[opts.operands..] {
        let (name, value) = split_assignment(arg);
        if !is_valid_identifier(name) {
            super::not_an_identifier(args, "local", arg);
            status = 1;
            continue;
        }
        // `local a=(1 2)` arrives here as the eight characters `a=(1 2)`, because an assignment
        // written after a command word is an ordinary argument. Storing them would make `a` the
        // string `(1 2)`.
        // **Before the value.** `-i` is what makes the assignment arithmetic, so `local -i n=2+3`
        // has to carry the mark by the time the 2+3 is stored. `local` accepted the letter and did
        // nothing with it, so an integer local held the expression as text.
        if opts.has('i') {
            env.set_integer(name);
        }
        // `localvar_inherit`: a local with no value starts with the one it shadows.
        let inherited = match value {
            None if crate::env::builtins::shopt::on("localvar_inherit") => {
                env.get_var(name).map(str::to_string)
            }
            _ => None,
        };
        let value = value.or(inherited.as_deref());
        let assigned = if let Some(body) = value.and_then(array_literal_body) {
            let array = array_elements(env, body)?;
            env.set_local_array(name, array)
        } else if opts.has('a') && value.is_none() {
            // `local -a x` declares an empty array. Without this it declared an empty *string*, so
            // the `x+=(a b)` that follows appended to a scalar and the array never existed —
            // which is how `substituteInPlace` failed at its first line.
            env.set_local_array(name, crate::env::scope::ShellArray::default())
        } else if opts.has('x') {
            env.set_local_exported_var(name, value.unwrap_or_default())
        } else {
            env.set_local_var(name, value.unwrap_or_default())
        };
        if !assigned {
            status = 1;
            continue;
        }
        // Marked only after the assignment succeeded: a name locked read-only with no value is
        // a name nobody can ever use again.
        if opts.has('r') {
            // Scoped: `local -r` is a local declaration, and its mark leaves with the frame.
            env.set_readonly_here(name);
        }
    }

    Ok(status)
}

/// `readonly [-p] [name[=value] ...]`.
///
/// The listing prints values, not just names. `readonly x` on its own told you a name was
/// read-only but not what it was frozen to, which is the one thing you cannot find out any other
/// way once the variable can no longer be assigned.
pub fn builtin_readonly(env: &mut Environment, args: &[String]) -> Result<i32> {
    let opts = match options::parse(args, "p") {
        Ok(o) => o,
        Err(letter) => return Err(options::invalid(args, "readonly", letter, READONLY_USAGE)),
    };
    let operands = &args[opts.operands..];

    if operands.is_empty() || opts.has('p') {
        print_listing(env);
        return Ok(0);
    }

    let mut status = 0;
    // `readonly` is a special builtin, so a name it cannot accept ends a POSIX-mode shell — see
    // the identical note in [`super::exporting::builtin_export`]. A *refused* assignment below is
    // an ordinary failure and is deliberately not tracked here.
    let mut bad_name = false;
    for arg in operands {
        let (name, assigned) = split_assignment(arg);
        if !is_valid_identifier(name) {
            super::not_an_identifier(args, "readonly", arg);
            status = 1;
            bad_name = true;
            continue;
        }
        // A refused assignment must not leave the name read-only: the user would then be unable
        // to set it at all, with nothing to show for it.
        let stored = match assigned.and_then(array_literal_body) {
            Some(body) => {
                let array = array_elements(env, body)?;
                env.set_array(name, array)
            }
            None => match assigned {
                Some(value) => env.set_var(name, value, false),
                None => true,
            },
        };
        if !stored {
            status = 1;
            continue;
        }
        env.set_readonly(name);
    }

    if bad_name {
        return Err(oslo_base::error::ShellError::utility_error(
            "readonly: not a valid identifier",
            1,
        ));
    }
    Ok(status)
}

/// Print the read-only variables as `readonly name='value'`, sorted.
fn print_listing(env: &Environment) {
    let vars = env.get_all_vars();
    let mut names: Vec<&String> = vars
        .keys()
        .filter(|name| is_valid_identifier(name) && env.is_readonly(name))
        .collect();
    names.sort();
    for name in names {
        println!("readonly {}={}", name, single_quoted(&vars[name]));
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::words;
    use super::{builtin_local, builtin_readonly};
    use crate::env::scope::Environment;

    /// What a function call does to the environment, which is the only state in which `local` is
    /// legal: a frame to write into *and* a non-zero call depth. `push_scope` alone is not enough,
    /// because a prefix assignment pushes a frame too and `local` is refused there.
    fn in_a_function() -> Environment {
        let mut env = Environment::new();
        env.enter_function().unwrap();
        env.push_scope();
        env
    }

    #[test]
    fn local_outside_a_function_is_refused() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_local(&mut env, &words(&["local", "x=1"])).unwrap(),
            1
        );
        assert!(env.get_var("x").is_none());
    }

    #[test]
    fn local_of_an_invalid_name_fails() {
        let mut env = in_a_function();
        assert_eq!(
            builtin_local(&mut env, &words(&["local", "=1"])).unwrap(),
            1
        );
        assert_eq!(builtin_local(&mut env, &words(&["local", "x"])).unwrap(), 0);
        // Popping is where a name smuggled into the frame would have aborted.
        env.pop_scope();
    }

    /// `local -r x=1` used to declare two variables, one of them called `-r`.
    #[test]
    fn local_options_are_not_names() {
        let mut env = in_a_function();
        assert_eq!(
            builtin_local(&mut env, &words(&["local", "-r", "LOCAL_RO=1"])).unwrap(),
            0
        );
        assert!(env.get_var("-r").is_none());
        assert!(env.get_var("r").is_none());
        assert_eq!(env.get_var("LOCAL_RO"), Some("1"));
        assert!(env.is_readonly("LOCAL_RO"));
        env.pop_scope();
    }

    #[test]
    fn local_i_is_accepted_without_becoming_a_variable() {
        let mut env = in_a_function();
        assert_eq!(
            builtin_local(&mut env, &words(&["local", "-i", "LOCAL_INT=7"])).unwrap(),
            0
        );
        assert!(env.get_var("-i").is_none());
        assert_eq!(env.get_var("LOCAL_INT"), Some("7"));
        env.pop_scope();
    }

    #[test]
    fn readonly_of_an_invalid_name_fails() {
        let mut env = Environment::new();
        // A utility error worth 1: `readonly` is special, so `bash --posix -c 'readonly 1bad=1;
        // echo alive'` prints no `alive`.
        let err = builtin_readonly(&mut env, &words(&["readonly", "=1"])).expect_err("fails");
        assert_eq!(err.survivable_utility_status(), Some(1));
        assert!(!env.is_readonly("=1"));
        assert!(!env.is_readonly(""));
    }

    /// A refused assignment must not leave the name read-only, or the variable becomes
    /// permanently unusable for no reason the user can see.
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

    /// `readonly -p` used to be an assignment to a variable called `-p`.
    #[test]
    fn readonly_p_lists_instead_of_declaring() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_readonly(&mut env, &words(&["readonly", "-p"])).unwrap(),
            0
        );
        assert!(!env.is_readonly("-p"));
    }

    /// `local -a x` declares an *array*, which is what `x+=(a b)` on the next line needs. It used
    /// to declare an empty string, so the append made a scalar and the array never existed —
    /// stdenv's `substituteInPlace` fails on its first line without this.
    #[test]
    fn local_dash_a_declares_an_array() {
        let mut env = in_a_function();
        assert_eq!(
            builtin_local(&mut env, &words(&["local", "-a", "ARR"])).unwrap(),
            0
        );
        assert!(env.get_array("ARR").is_some(), "ARR is an array");
        assert_eq!(env.get_array("ARR").map(|a| a.values().count()), Some(0));
        env.pop_scope();
    }
}
