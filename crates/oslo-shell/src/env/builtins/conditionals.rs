//! Conditional expressions: the `test` / `[` builtin and the extended `[[` form.
//!
//! Both entry points share one operator table (the `operators` submodule); only `test`/`[` needs
//! the recursive-descent parser in `grammar`, because the `[[` form's connectives (`&&`, `||`,
//! `!`, parentheses) are lowered by [`crate::syntax::rune_adapter`] into ordinary shell control
//! flow, leaving this builtin a single predicate to evaluate.
//!
//! The contract both forms now honour: an expression either evaluates to a truth value (exit 0 for
//! true, 1 for false) or fails to parse (a diagnostic on stderr and **exit 2**). There is no third
//! outcome in which an unrecognised operator quietly becomes an answer.

mod arith;
mod grammar;
mod matching;
mod operators;

use crate::env::origin_now;
use crate::env::scope::Environment;
use operators::{Mode, TestError, TestResult};
use oslo_base::error::Result;

/// `test` and `[`.
pub fn builtin_test(env: &mut Environment, args: &[String]) -> Result<i32> {
    let Some(name) = args.first().map(String::as_str) else {
        return Ok(1);
    };

    let mut expr = &args[1..];
    if name == "[" {
        if expr.last().map(String::as_str) == Some("]") {
            expr = &expr[..expr.len() - 1];
        } else {
            eprintln!("{}[: missing `]'", origin_now());
            return Ok(2);
        }
    }

    let expr = resolve_set_tests(env, expr, true)?;
    report(name, grammar::evaluate(env, &expr))
}

/// The operands of `-v` with an element's subscript evaluated, as bash evaluates `-v 'a[i+1]'`,
/// so the tests themselves never need to evaluate anything. `test` expands the subscript first;
/// `[[ ]]` has expanded its operand already and reads the subscript as it stands.
fn resolve_set_tests(env: &mut Environment, expr: &[String], expand: bool) -> Result<Vec<String>> {
    let mut resolved = expr.to_vec();
    for at in 1..resolved.len() {
        if resolved[at - 1] == "-v"
            && let Some(element) =
                crate::env::builtins::arrays::resolve_element(env, &resolved[at], expand)?
        {
            resolved[at] = element;
        }
    }
    Ok(resolved)
}

/// `[[ ... ]]` — the extended test.
///
/// Never written by hand at this level: [`crate::syntax::rune_adapter`] converts the parsed
/// expression tree into these calls, using the shell's own `&&`/`||`/`!` for the connectives so
/// this only has to evaluate a single predicate.
///
/// The difference from [`builtin_test`] that matters is `==`: inside `[[ ]]` an unquoted
/// right-hand side is a glob *pattern*, so `[[ abc == a* ]]` is true. The adapter picks `==`
/// (pattern) or `=` (literal) based on whether the operand was quoted in the source, and picks
/// between the two `=~` spellings the same way — see the `matching` submodule.
pub fn builtin_extended_test(env: &mut Environment, args: &[String]) -> Result<i32> {
    // Strip the `[[` / `]]` bookends the adapter adds.
    let mut expr: &[String] = args.get(1..).unwrap_or(&[]);
    if expr.last().map(String::as_str) == Some("]]") {
        expr = &expr[..expr.len() - 1];
    }
    let mut negated = false;
    while expr.len() > 1 && expr[0] == crate::syntax::lower::cond::NEGATION {
        negated = !negated;
        expr = &expr[1..];
    }

    let resolved = match expr.len() {
        2 => resolve_set_tests(env, expr, false)?,
        _ => Vec::new(),
    };
    let outcome: TestResult<bool> = match expr.len() {
        0 => Ok(false),
        1 => Ok(!expr[0].is_empty()),
        2 => operators::eval_unary(env, &resolved[0], &resolved[1]),
        // `=~` is routed before the shared table because it is the one operator that *writes*:
        // it publishes `BASH_REMATCH`, so it needs `&mut Environment`, which the POSIX `test`
        // grammar sharing `eval_binary` does not have. That split is also the enforcement of
        // `=~` being `[[ ]]`-only.
        3 if matching::is_regex_op(&expr[1]) => {
            matching::eval_regex_match(env, &expr[0], &expr[1], &expr[2])
        }
        3 if arith::is_arithmetic_op(&expr[1]) => {
            return arith::compare(env, &expr[0], &expr[1], &expr[2]);
        }
        // The C locale matches a pattern a byte to a character.
        3 if matches!(expr[1].as_str(), "==" | "=" | "!=")
            && !crate::expand::locale::is_utf8(env) =>
        {
            let bytewise = crate::expand::locale::bytewise;
            operators::eval_binary(
                Mode::Extended,
                &bytewise(&expr[0]),
                &expr[1],
                &bytewise(&expr[2]),
            )
        }
        3 => operators::eval_binary(Mode::Extended, &expr[0], &expr[1], &expr[2]),
        _ => Err(TestError::new("too many arguments")),
    };

    report("[[", outcome.map(|truth| truth != negated))
}

/// One place that turns an evaluation into an exit status, so `test`, `[` and `[[` cannot drift
/// apart on what a syntax error costs.
fn report(name: &str, outcome: TestResult<bool>) -> Result<i32> {
    match outcome {
        Ok(true) => Ok(0),
        Ok(false) => Ok(1),
        Err(err) => {
            eprintln!("{}{}: {}", origin_now(), name, err.message());
            Ok(2)
        }
    }
}

#[cfg(test)]
mod tests;
