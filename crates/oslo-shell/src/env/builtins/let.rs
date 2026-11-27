//! `let` — evaluate arithmetic expressions for their side effects.
//!
//! A thin wrapper over the arithmetic evaluator: `let i=i+1` is `$(( i = i + 1 ))` with the
//! result thrown away. The only part worth stating is the exit status, which is *inverted*
//! relative to intuition — `let` succeeds when the last expression is non-zero, because that is
//! what makes `if let "x > 3"` read the way an arithmetic condition should.

use crate::env::origin_now;
use crate::env::scope::Environment;
use crate::expand::arithmetic::eval_arithmetic;
use oslo_base::error::Result;

/// `let [--] expr [expr…]`.
pub fn builtin_let(env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut exprs = &args[1.min(args.len())..];
    // `--` ends the options, as it does for every bash builtin. `let` has no options of its own,
    // which is exactly why the marker matters: `let -- "-1 < 0"` is how a script passes an
    // expression that begins with a minus without it being read as one. modernish writes every
    // arithmetic test as `alias let='let --'`, so without this its whole init dies on
    // "Unexpected end of arithmetic expression".
    if exprs.first().map(String::as_str) == Some("--") {
        exprs = &exprs[1..];
    }
    if exprs.is_empty() {
        eprintln!("{}let: expression expected", origin_now());
        return Ok(1);
    }

    let mut last = 0;
    for expr in exprs {
        match eval_arithmetic(env, expr) {
            Ok(value) => last = value,
            // An unparseable expression is the builtin's failure, not the shell's: bash reports
            // it, gives `let` status 1, and carries on with the next command. An unset name
            // under `set -u` is fatal, as it is everywhere.
            Err(e @ oslo_base::error::ShellError::UnsetParameter(_)) => return Err(e),
            Err(e) => {
                eprintln!("{}let: {}", origin_now(), e);
                return Ok(1);
            }
        }
    }

    // Non-zero is success, zero is failure — the same convention `((expr))` uses.
    Ok(i32::from(last == 0))
}

#[cfg(test)]
mod tests {
    use super::builtin_let;
    use crate::env::Environment;

    fn run(env: &mut Environment, exprs: &[&str]) -> i32 {
        let mut args = vec!["let".to_string()];
        args.extend(exprs.iter().map(|s| s.to_string()));
        builtin_let(env, &args).expect("let")
    }

    #[test]
    fn an_assignment_reaches_the_environment() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["oslo_let_x = 1 + 2"]), 0);
        assert_eq!(env.get_var("oslo_let_x"), Some("3"));
    }

    /// Every expression is evaluated; only the last one decides the status.
    #[test]
    fn several_expressions_all_run() {
        let mut env = Environment::new();
        assert_eq!(
            run(&mut env, &["oslo_let_a=5", "oslo_let_b=oslo_let_a*2"]),
            0
        );
        assert_eq!(env.get_var("oslo_let_b"), Some("10"));
    }

    /// The inverted status: a zero result is a *failed* `let`, which is what makes `let` usable
    /// as an `if` condition.
    #[test]
    fn a_zero_result_is_a_failure() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["1 > 3"]), 1);
        assert_eq!(run(&mut env, &["3 > 1"]), 0);
        assert_eq!(run(&mut env, &["oslo_let_z = 0"]), 1);
    }

    #[test]
    fn a_bad_expression_fails_without_killing_the_shell() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["1 +"]), 1);
    }

    #[test]
    fn no_expression_at_all_is_a_usage_error() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &[]), 1);
    }

    /// `--` ends the options. modernish aliases `let` to `let --`, so every arithmetic test it
    /// makes arrives this way; before this, each one died as an unparseable expression.
    #[test]
    fn a_double_dash_ends_the_options() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["--", "1 + 1"]), 0);
        assert_eq!(run(&mut env, &["--", "0"]), 1);
        assert_eq!(run(&mut env, &["--", "-1 < 0"]), 0);
        // `--` on its own leaves no expression, which is the usage error bash reports.
        assert_eq!(run(&mut env, &["--"]), 1);
        // A second `--` is an expression, not another marker, as in bash.
        assert_eq!(run(&mut env, &["--", "--"]), 1);
    }
}
