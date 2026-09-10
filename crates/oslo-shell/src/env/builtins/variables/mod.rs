//! Variables and aliases: `export`, `unset`, `set`, `shift`, `local`, `readonly`,
//! `alias`, `unalias`.
//!
//! Split by what each builtin acts on rather than kept in one file, because the three jobs here
//! barely overlap: giving a name an attribute ([`exporting`], [`scoped`]), maintaining the alias
//! table ([`aliases`]), and printing the shell's state back as something a shell can read again
//! ([`quoting`], [`parameters`]).
//!
//! Every listing these builtins produce is *re-inputtable*: sorted so two runs agree, and quoted
//! so `export -p > f; . f` restores exactly what was there. A `HashMap` iteration order and a
//! `{:?}` value are neither, which is what the listings used to be.
//!
//! # Utility errors
//!
//! Four of these builtins are POSIX *special* builtins (`export`, `unset`, `readonly` via
//! [`scoped`], `set` via [`parameters`]), and POSIX 2.8.1 ends a non-interactive shell that gives
//! one a *utility* error. So a bad option letter or a name that is not an identifier is returned
//! as [`ShellError::UtilityError`] rather than as a status, and
//! `crate::exec::simple::posix::resolve_builtin_result` decides what becomes of it — folding it
//! back to the same status this code used to return whenever POSIX mode is off, which is always
//! for `alias`/`unalias`/`local`, none of which are special.
//!
//! An ordinary failure stays an ordinary status: `export -f nosuchfn` and `local` outside a
//! function report 1 and are survivable in every shell, and mixing the two up is what would kill
//! a `--posix` script on `shift 5`.
//!
//! [`Environment`]: crate::env::scope::Environment
//! [`ShellError::UtilityError`]: oslo_base::error::ShellError::UtilityError

mod aliases;
mod exporting;
mod options;
mod parameters;
pub(crate) mod quoting;
mod scoped;

pub use aliases::{builtin_alias, builtin_unalias};
pub use exporting::{builtin_export, builtin_unset};
pub use parameters::{builtin_set, builtin_shift};
pub use scoped::{builtin_local, builtin_readonly};

/// Complain about `word` the way bash does for a name that is not `[A-Za-z_][A-Za-z0-9_]*`.
///
/// The whole word is quoted back, not the part before the `=`, so `export '=1'` names what the
/// user actually typed.
fn not_an_identifier(args: &[String], builtin: &str, word: &str) {
    // Backtick-then-quote, which is bash's spelling and the one `declare`, `mapfile` and `printf`
    // already use here. This was the only place left writing it with two apostrophes.
    crate::env::complain(
        args,
        word,
        &format!(
            "{builtin}: `{}': not a valid identifier",
            oslo_base::shown::shown(word)
        ),
        "not a name",
        Some(IDENTIFIER),
    );
}

/// What a shell name may be, for the help line under one that is not.
///
/// Worth saying because the rule is not obvious from the failure: the commonest way to reach this
/// message is `export 2FOO=x` or `unset a-b`, and neither looks wrong until the rule is stated.
const IDENTIFIER: &str =
    "a name starts with a letter or underscore and continues with letters, digits or underscores";

#[cfg(test)]
mod tests {
    use super::{builtin_export, builtin_local};
    use crate::env::scope::Environment;

    pub fn words(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// Before R1.7 this call reached `env::set_var("", "1")` and killed the process.
    #[test]
    fn export_of_an_invalid_name_fails_without_setting_anything() {
        let mut env = Environment::new();
        for bad in ["=1", "1abc=x", "a b=1", "a-b"] {
            // A utility error worth 1, which is what a POSIX-mode shell exits with; outside
            // POSIX mode `crate::exec::simple::posix` folds it back to `Ok(1)`.
            let err = builtin_export(&mut env, &words(&["export", bad]))
                .expect_err("export {bad:?} should fail");
            assert_eq!(err.survivable_utility_status(), Some(1), "export {bad:?}");
        }
        assert!(env.get_var("=1").is_none());
        assert!(env.get_var("1abc").is_none());
    }

    /// A value carrying a NUL — from `read` over a binary file, say — cannot go into `environ`.
    #[test]
    fn export_of_a_nul_bearing_value_fails() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_export(&mut env, &words(&["export", "NUL_VAR=a\0b"])).unwrap(),
            1
        );
        assert!(env.get_var("NUL_VAR").is_none());
    }

    /// One bad name must not stop the good names on the same line, but must still set status 1.
    ///
    /// Tested through `local` rather than `export` so the assertion never writes to the real
    /// `environ`: these unit tests run on parallel threads, and mutating `environ` under them is
    /// exactly the hazard the `unsafe` blocks in `scope.rs` are documented against.
    #[test]
    fn a_bad_name_does_not_stop_the_rest_of_the_line() {
        let mut env = Environment::new();
        // `local` is legal only inside a function call, so the test has to be in one.
        env.enter_function().unwrap();
        env.push_scope();
        let args = words(&["local", "=1", "GOOD_ONE=yes"]);
        assert_eq!(builtin_local(&mut env, &args).unwrap(), 1);
        assert_eq!(env.get_var("GOOD_ONE"), Some("yes"));
        env.pop_scope();
    }
}
