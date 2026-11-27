//! Arithmetic expansion — `$(( … ))`.
//!
//! Split four ways along the seams every expression evaluator has: `operand` runs the shell's own
//! expansions over the expression text (POSIX does that before any of it is arithmetic), `lexer`
//! turns the result into tokens (and owns the numeric-literal bases), `parser` turns tokens into a
//! tree (and owns the precedence ladder), `eval` walks the tree against the environment (and owns
//! wrapping semantics and assignment side effects).

mod eval;
mod lexer;
mod operand;
mod parser;

pub use eval::eval_arithmetic;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Environment;

    fn eval(expr: &str) -> i64 {
        let mut env = Environment::new();
        eval_arithmetic(&mut env, expr).unwrap_or_else(|e| panic!("{expr}: {e}"))
    }

    fn eval_with(name: &str, value: &str, expr: &str) -> i64 {
        let mut env = Environment::new();
        env.set_var(name, value, false);
        eval_arithmetic(&mut env, expr).unwrap_or_else(|e| panic!("{expr}: {e}"))
    }

    /// Evaluate, then report both the result and what the variable ended up holding.
    fn eval_and_read(setup: &[(&str, &str)], expr: &str, read: &str) -> (i64, String) {
        let mut env = Environment::new();
        for (k, v) in setup {
            env.set_var(k, v, false);
        }
        let value = eval_arithmetic(&mut env, expr).unwrap_or_else(|e| panic!("{expr}: {e}"));
        (value, env.get_param(read).unwrap_or_default())
    }

    fn err_of(expr: &str) -> String {
        let mut env = Environment::new();
        eval_arithmetic(&mut env, expr).expect_err(expr).to_string()
    }

    #[test]
    fn arithmetic_still_computes_ordinary_values() {
        assert_eq!(eval("1 + 2 * 3"), 7);
        assert_eq!(eval("(1 + 2) * 3"), 9);
        assert_eq!(eval("7 / 2"), 3);
        assert_eq!(eval("7 % 2"), 1);
        assert_eq!(eval("-3 + 1"), -2);
        assert_eq!(eval(""), 0);
        assert_eq!(eval("   "), 0);
    }

    // --- R3.1: the full ladder. Every one of these used to return its left operand. ---

    #[test]
    fn comparison_operators() {
        let cases = [
            ("5 > 3", 1),
            ("3 > 5", 0),
            ("2 == 3", 0),
            ("2 != 3", 1),
            ("2 <= 2", 1),
            ("2 >= 3", 0),
            ("2 < 3", 1),
            ("1 == 1 == 1", 1),
        ];
        for (expr, want) in cases {
            assert_eq!(eval(expr), want, "{expr}");
        }
    }

    #[test]
    fn bitwise_operators() {
        let cases = [
            ("3 & 1", 1),
            ("3 | 4", 7),
            ("3 ^ 1", 2),
            ("~0", -1),
            ("1 << 4", 16),
            ("16 >> 2", 4),
            ("-8 >> 1", -4),
            // Shift counts are taken modulo 64, as on every machine bash runs on.
            ("1 << 64", 1),
            ("1 << 65", 2),
            ("1 << -1", i64::MIN),
            ("-1 >> 64", -1),
        ];
        for (expr, want) in cases {
            assert_eq!(eval(expr), want, "{expr}");
        }
    }

    #[test]
    fn logical_operators() {
        let cases = [
            ("1 && 0", 0),
            ("1 && 2", 1),
            ("0 || 0", 0),
            ("0 || 5", 1),
            ("!0", 1),
            ("!7", 0),
            ("!!7", 1),
        ];
        for (expr, want) in cases {
            assert_eq!(eval(expr), want, "{expr}");
        }
    }

    #[test]
    fn ternary_and_comma() {
        let cases = [
            ("1 ? 2 : 3", 2),
            ("0 ? 2 : 3", 3),
            ("1, 2, 3", 3),
            ("3 ? 4 : 5 ? 6 : 7", 4),
            ("0 ? 4 : 0 ? 6 : 7", 7),
            ("1 ? 2 : 3 , 4", 4),
        ];
        for (expr, want) in cases {
            assert_eq!(eval(expr), want, "{expr}");
        }
    }

    #[test]
    fn precedence_pairs() {
        let cases = [
            ("2 + 3 * 4", 14),
            ("(2 + 3) * 4", 20),
            ("10 - 2 - 3", 5),
            ("2 + 3 > 4", 1),
            ("1 | 2 & 3", 3),
            ("-2 * -3", 6),
            ("1 ^ 3 & 1", 0),
            ("1 << 2 + 1", 8),
            ("1 + 2 == 3", 1),
            ("0 || 1 && 0", 0),
            ("2 ** 10", 1024),
            ("3 ** 2 ** 2", 81),
            ("-2 ** 2", 4),
            ("0 ** 0", 1),
            ("2 ** 100", 0),
        ];
        for (expr, want) in cases {
            assert_eq!(eval(expr), want, "{expr}");
        }
    }

    /// The right side of `&&`/`||` and the untaken arm of `?:` must not run at all.
    #[test]
    fn short_circuit_skips_the_dead_side() {
        assert_eq!(eval("0 && (1 / 0)"), 0);
        assert_eq!(eval("1 || (1 / 0)"), 1);
        assert_eq!(eval("1 ? 1 : (1 / 0)"), 1);

        let (_, x) = eval_and_read(&[("x", "0")], "0 && (x = 9)", "x");
        assert_eq!(x, "0", "the dead branch must not assign");
    }

    /// R3.1's headline requirement: never a silent partial result.
    ///
    /// `"1"` is deliberately absent: R3.4 made quote removal part of arithmetic expansion, so a
    /// quoted operand is now the operand it quotes. A `$` with nothing expandable after it is
    /// still a character the grammar has no token for.
    #[test]
    fn unconsumed_input_is_an_error() {
        for expr in ["1 2", "1 +", "(1", "1)", "1 ? 2", "1 $ 2", "1 @ 2"] {
            let mut env = Environment::new();
            assert!(
                eval_arithmetic(&mut env, expr).is_err(),
                "{expr} must not evaluate"
            );
        }
    }

    // --- R3.2: assignment and stepping. ---

    #[test]
    fn plain_assignment_returns_and_stores() {
        let (v, x) = eval_and_read(&[("x", "5")], "x = 9", "x");
        assert_eq!((v, x.as_str()), (9, "9"));
    }

    #[test]
    fn compound_assignments_store_the_combined_value() {
        let cases = [
            ("x += 3", 8, "8"),
            ("x -= 3", 2, "2"),
            ("x *= 2", 10, "10"),
            ("x /= 2", 2, "2"),
            ("x %= 3", 2, "2"),
            ("x <<= 2", 20, "20"),
            ("x >>= 1", 2, "2"),
            ("x &= 3", 1, "1"),
            ("x |= 3", 7, "7"),
            ("x ^= 3", 6, "6"),
        ];
        for (expr, want, stored) in cases {
            let (v, x) = eval_and_read(&[("x", "5")], expr, "x");
            assert_eq!((v, x.as_str()), (want, stored), "{expr}");
        }
    }

    #[test]
    fn assignment_is_right_associative_and_chains() {
        let (v, x) = eval_and_read(&[("x", "5")], "x = y = 3", "x");
        assert_eq!((v, x.as_str()), (3, "3"));
        let (_, y) = eval_and_read(&[], "x = y = 3", "y");
        assert_eq!(y, "3");
    }

    #[test]
    fn post_increment_returns_the_old_value() {
        let (v, i) = eval_and_read(&[("i", "1")], "i++", "i");
        assert_eq!((v, i.as_str()), (1, "2"));
        let (v, i) = eval_and_read(&[("i", "1")], "i--", "i");
        assert_eq!((v, i.as_str()), (1, "0"));
    }

    #[test]
    fn pre_increment_returns_the_new_value() {
        let (v, i) = eval_and_read(&[("i", "1")], "++i", "i");
        assert_eq!((v, i.as_str()), (2, "2"));
        let (v, i) = eval_and_read(&[("i", "1")], "--i", "i");
        assert_eq!((v, i.as_str()), (0, "0"));
    }

    /// Stepping an unset variable starts from 0, exactly as reading one does.
    #[test]
    fn stepping_an_unset_variable_starts_at_zero() {
        let (v, y) = eval_and_read(&[], "y++", "y");
        assert_eq!((v, y.as_str()), (0, "1"));
    }

    /// A variable holding an octal or hex string is a number of that base, everywhere.
    #[test]
    fn stepping_reads_the_stored_value_in_its_own_base() {
        let (v, x) = eval_and_read(&[("x", "010")], "x++", "x");
        assert_eq!((v, x.as_str()), (8, "9"));
    }

    #[test]
    fn assignments_inside_larger_expressions_take_effect() {
        let (v, x) = eval_and_read(&[("x", "1")], "x = 2, x + 1", "x");
        assert_eq!((v, x.as_str()), (3, "2"));
        let (v, x) = eval_and_read(&[("x", "5")], "(x += 1) * 2", "x");
        assert_eq!((v, x.as_str()), (12, "6"));
    }

    #[test]
    fn assignment_to_a_non_variable_is_diagnosed() {
        assert!(err_of("1 = 2").contains("non-variable"));
        assert!(err_of("x = 2 = 3").contains("non-variable"));
        // A `++` with no variable to step is two signs, as in bash.
        assert_eq!(eval("++1"), 1);
        assert_eq!(eval("1 +++ 2"), 3);
    }

    // --- R3.3: literal bases. ---

    #[test]
    fn literal_bases() {
        let cases = [
            ("0x1f", 31),
            ("0X1F", 31),
            ("010", 8),
            ("0755", 493),
            ("2#101", 5),
            ("16#ff", 255),
            ("16#FF", 255),
            ("64#_", 63),
            ("0", 0),
            // The chmod case that used to come out as 10 * 20 = 200.
            ("010 * 020", 128),
        ];
        for (expr, want) in cases {
            assert_eq!(eval(expr), want, "{expr}");
        }
    }

    #[test]
    fn bad_literals_are_errors_not_zero() {
        assert!(err_of("08").contains("value too great for base"));
        assert!(err_of("2#3").contains("value too great for base"));
        assert!(err_of("1#0").contains("invalid arithmetic base"));
        assert!(err_of("65#1").contains("invalid arithmetic base"));
    }

    #[test]
    fn variables_resolve_in_their_own_base() {
        assert_eq!(eval_with("m", "0755", "m"), 493);
        assert_eq!(eval_with("m", "0x10", "m + 1"), 17);
        assert_eq!(eval_with("m", "-7", "m"), -7);
        // Anything that is not a plain number is 0, as in bash.
        assert_eq!(eval_with("m", "", "m + 1"), 1);
        assert_eq!(eval_with("m", "abc", "m + 1"), 1);
    }

    // --- R3.4: operand resolution. ---

    fn env_with(setup: &[(&str, &str)]) -> Environment {
        let mut env = Environment::new();
        for (k, v) in setup {
            env.set_var(k, v, false);
        }
        env
    }

    fn eval_vars(setup: &[(&str, &str)], expr: &str) -> i64 {
        let mut env = env_with(setup);
        eval_arithmetic(&mut env, expr).unwrap_or_else(|e| panic!("{expr}: {e}"))
    }

    fn err_with(setup: &[(&str, &str)], expr: &str) -> String {
        let mut env = env_with(setup);
        eval_arithmetic(&mut env, expr).expect_err(expr).to_string()
    }

    /// A variable whose value is an expression evaluates as one, exactly as in bash.
    #[test]
    fn an_expression_valued_variable_is_evaluated() {
        assert_eq!(eval_with("x", "1+1", "x"), 2);
        assert_eq!(eval_with("x", "2 * 3", "x + 1"), 7);
        assert_eq!(eval_vars(&[("x", "y + 1"), ("y", "4")], "x"), 5);
    }

    /// The indirection case from the finding: `a` names `b`, `b` holds the number.
    #[test]
    fn identifiers_resolve_recursively() {
        assert_eq!(eval_vars(&[("a", "b"), ("b", "7")], "a"), 7);
        assert_eq!(
            eval_vars(&[("a", "b"), ("b", "c"), ("c", "9")], "a * 2"),
            18
        );
    }

    /// Recursion has to end somewhere: a cycle is diagnosed, not run into the stack.
    #[test]
    fn a_resolution_cycle_is_diagnosed() {
        assert!(err_with(&[("a", "a")], "a").contains("recursion"));
        assert!(err_with(&[("a", "b"), ("b", "a")], "a").contains("recursion"));
    }

    /// A parameter expansion in the expression is expanded before the expression is scanned.
    #[test]
    fn parameter_expansions_run_over_the_expression() {
        assert_eq!(eval_with("x", "41", "$x + 1"), 42);
        assert_eq!(eval_with("x", "41", "${x} + 1"), 42);
        assert_eq!(eval_with("s", "abcde", "${#s} + 1"), 6);
        assert_eq!(eval_vars(&[("i", "2")], "${n:-3} * i"), 6);
        // The positional `$1` is what `countdown $(($1 - 1))` needs.
        let mut env = Environment::new();
        env.set_positional(vec!["3".into()]);
        assert_eq!(eval_arithmetic(&mut env, "$1 - 1").unwrap(), 2);
    }

    /// A nested `$(( ))` is an ordinary expansion of the enclosing expression's text.
    #[test]
    fn nested_arithmetic_expansion_is_resolved() {
        assert_eq!(eval("$((1 + 2)) * 3"), 9);
    }

    /// bash removes double quotes from the expression but leaves single quotes to be rejected:
    /// `$(( "1" + 1 ))` is 2, `$(( '2' * 3 ))` is a syntax error.
    #[test]
    fn double_quotes_are_removed_and_single_quotes_are_not() {
        assert_eq!(eval_with("x", "4", "\"$x\" + 1"), 5);
        assert_eq!(eval("\"1\""), 1);
        assert!(err_of("'2' * 3").contains("Invalid character"));
    }

    /// The word lexer would read a word-opening `~` as a home directory; here it is bitwise NOT.
    #[test]
    fn a_tilde_next_to_an_expansion_is_still_bitwise_not() {
        assert_eq!(eval_with("x", "0", "~$x"), -1);
    }

    // --- R1.8 / R3.5: the overflow safety this rewrite must preserve. ---

    #[test]
    fn addition_overflow_wraps_like_bash() {
        assert_eq!(eval("9223372036854775807 + 1"), i64::MIN);
        assert_eq!(eval("9223372036854775807 + 9223372036854775807"), -2);
    }

    #[test]
    fn subtraction_underflow_wraps_like_bash() {
        assert_eq!(eval("-9223372036854775807 - 2"), i64::MAX);
        assert_eq!(eval_with("a", "-9223372036854775808", "a - 1"), i64::MAX);
    }

    #[test]
    fn multiplication_overflow_wraps_like_bash() {
        assert_eq!(eval("9223372036854775807 * 2"), -2);
        assert_eq!(eval_with("a", "-9223372036854775808", "a * -1"), i64::MIN);
    }

    #[test]
    fn min_divided_by_negative_one_wraps_instead_of_panicking() {
        assert_eq!(eval_with("a", "-9223372036854775808", "a / -1"), i64::MIN);
        assert_eq!(eval("-9223372036854775808 / -1"), i64::MIN);
    }

    #[test]
    fn min_modulo_negative_one_is_zero() {
        assert_eq!(eval_with("a", "-9223372036854775808", "a % -1"), 0);
        assert_eq!(eval("-9223372036854775808 % -1"), 0);
    }

    #[test]
    fn min_literal_negates_without_overflow() {
        assert_eq!(eval("-9223372036854775808"), i64::MIN);
        assert_eq!(eval("- -9223372036854775808"), i64::MIN);
    }

    #[test]
    fn out_of_range_literal_wraps_rather_than_erroring() {
        // Previously this word never reached the parser: the operand pass turned any literal it
        // could not fit in an i64 into 0.
        assert_eq!(eval("9223372036854775808"), i64::MIN);
        assert_eq!(eval("18446744073709551616"), 0);
    }

    #[test]
    fn compound_assignment_overflow_wraps() {
        let (v, x) = eval_and_read(&[("x", "9223372036854775807")], "x += 1", "x");
        assert_eq!((v, x.as_str()), (i64::MIN, "-9223372036854775808"));
        let (v, _) = eval_and_read(&[("i", "9223372036854775807")], "i++", "i");
        assert_eq!(v, i64::MAX);
    }

    /// A negative exponent is an error in bash, not a silent 0.
    #[test]
    fn negative_exponent_is_an_error() {
        assert!(err_of("2 ** -1").contains("exponent"));
    }

    /// 50 000 parentheses used to recurse until the stack overflowed and the process aborted.
    #[test]
    fn absurd_parenthesis_nesting_is_an_error_not_a_crash() {
        let mut env = Environment::new();
        let expr = format!("{}1{}", "(".repeat(50_000), ")".repeat(50_000));
        let err = eval_arithmetic(&mut env, &expr).expect_err("must be refused");
        assert!(
            err.to_string().contains("maximum nesting level exceeded"),
            "{err}"
        );
    }

    /// Unary operators recurse through the same chain as parentheses do.
    #[test]
    fn absurd_unary_nesting_is_an_error_not_a_crash() {
        let mut env = Environment::new();
        let expr = format!("{}1", "-".repeat(50_000));
        assert!(eval_arithmetic(&mut env, &expr).is_err());
    }

    /// The limit has to leave ordinary — even unusually parenthesised — expressions alone.
    #[test]
    fn moderate_nesting_still_evaluates() {
        let expr = format!("{}1 + 1{}", "(".repeat(30), ")".repeat(30));
        assert_eq!(eval(&expr), 2);
    }

    /// The nesting limit exists to keep a hostile expression from overflowing the stack, so the
    /// limit itself has to fit on a stack oslo might actually get. A 1 MiB thread is half what
    /// Rust gives a spawned thread by default, so the limit holds with room to spare.
    ///
    /// Without this the invariant was only ever checked against whatever stack the test runner
    /// happened to provide, which is exactly how a limit of 100 shipped while overflowing.
    #[test]
    fn nesting_at_the_limit_fits_a_small_stack() {
        let worker = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(|| {
                let mut env = Environment::new();
                // One level under the limit: the deepest expression that must still evaluate.
                let expr = format!("{}7{}", "(".repeat(31), ")".repeat(31));
                assert_eq!(eval_arithmetic(&mut env, &expr).expect("must evaluate"), 7);

                // And one far past it, which must be refused rather than overflow.
                let expr = format!("{}7{}", "(".repeat(50_000), ")".repeat(50_000));
                assert!(eval_arithmetic(&mut env, &expr).is_err());
            })
            .expect("spawn");
        worker
            .join()
            .expect("the parser must not overflow a 1 MiB stack");
    }

    #[test]
    fn division_by_zero_is_still_an_error() {
        let mut env = Environment::new();
        assert!(eval_arithmetic(&mut env, "1 / 0").is_err());
        assert!(eval_arithmetic(&mut env, "1 % 0").is_err());
    }
}
