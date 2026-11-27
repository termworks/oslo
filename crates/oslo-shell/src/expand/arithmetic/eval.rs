//! Evaluation of a parsed arithmetic expression against the shell environment.
//!
//! Every operator here is wrapping. bash's arithmetic is C `intmax_t` arithmetic, which wraps;
//! plain Rust operators would panic in a debug build and wrap in a release one, and the same
//! script must not depend on the build profile.

use crate::env::Environment;
use crate::expand::arithmetic::lexer::{self, tokenize};
use crate::expand::arithmetic::operand::expand_expression;
use crate::expand::arithmetic::parser::{self, BinOp, Expr, Ref, UnOp};
use oslo_base::error::{Result, ShellError};

/// How long a chain of "this variable's value is itself an expression" may get.
///
/// An identifier that does not hold a plain number is evaluated as an expression in its own right:
/// `a=b; b=7; $((a))` is 7 and `x="1+1"; $((x))` is 2, both of which used to be 0. That makes
/// `a=a` — or any longer cycle — non-terminating, so the chain is bounded and a cycle is reported
/// rather than run into the stack. 32 is far past any real indirection and far short of the limit.
const MAX_RESOLVE_DEPTH: usize = 32;

/// How many pieces of text one evaluation may expand and evaluate, all told.
///
/// **[`MAX_RESOLVE_DEPTH`] bounds the height of the recursion and says nothing about its width.**
/// A variable whose value is an expression naming two more variables branches, and thirty-two
/// levels of branching by two is four billion evaluations:
///
/// ```text
/// a='b+b'; b='c+c'; c='d+d'; …    thirty levels
/// ```
///
/// Each is a legal assignment and each doubles the work, so the depth cap alone lets a shell hang
/// on a page of ordinary-looking variables. Ten thousand evaluations is far past anything a person
/// writes — the POSIX corpus never exceeds a few dozen — and short enough to be imperceptible.
const MAX_EVALUATIONS: usize = 10_000;

thread_local! {
    /// Evaluations spent by the call to [`eval_arithmetic`] currently running.
    ///
    /// A counter rather than an argument threaded through `eval`, `resolve` and `read`: it is one
    /// number for the whole evaluation and every one of those functions would otherwise have to
    /// carry it to hand it to the next.
    static SPENT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };

    /// How many [`eval_arithmetic`] calls are on the stack right now.
    ///
    /// **Which is what says whether a call is the top-level one.** Without it the budget below was
    /// reset by every call, and a nested `$(( ))` is a call: the value of a variable is expanded
    /// while it is being read, so `a='$((a))'` re-entered here through word expansion and started a
    /// fresh allowance each time. The ceiling was never reached and the stack ran out instead —
    /// `echo $((a))` aborted the shell where bash prints a syntax error.
    static NESTING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One arithmetic evaluation in progress. The outermost resets the budget; the rest inherit it.
struct Evaluation;

impl Evaluation {
    /// **This is the guard `depth` cannot be.** `eval_text` expands its expression before reading
    /// it, because POSIX says parameter expansion runs first — so a `$(( ))` written *inside* a
    /// variable's value leaves here through the word expander and arrives back at
    /// [`eval_arithmetic`], which starts a fresh `depth` of 0. `a='$((a))'` recursed through that
    /// door until the stack ran out. Counting the doorway is what closes it.
    fn enter() -> Result<Evaluation> {
        NESTING.with(|nesting| {
            let outer = nesting.get();
            if outer >= MAX_RESOLVE_DEPTH {
                return Err(ShellError::ExpansionError(
                    "arithmetic expression recursion level exceeded".to_string(),
                ));
            }
            nesting.set(outer + 1);
            if outer == 0 {
                // The budget belongs to one *top-level* evaluation, so a shell that has done a
                // million expansions today starts this one with the whole allowance.
                SPENT.with(|spent| spent.set(0));
            }
            Ok(Evaluation)
        })
    }
}

impl Drop for Evaluation {
    fn drop(&mut self) {
        NESTING.with(|nesting| nesting.set(nesting.get().saturating_sub(1)));
    }
}

/// Evaluate an arithmetic expression, applying any assignments it performs.
///
/// The environment is taken mutably because `$((i++))` and `$((x = 9))` are expressions with
/// side effects; a read-only environment made them structurally impossible to support.
pub fn eval_arithmetic(env: &mut Environment, expr: &str) -> Result<i64> {
    let _evaluation = Evaluation::enter()?;
    eval_text(env, expr, 0)
}

/// Expand, scan, parse and evaluate one piece of expression text.
///
/// `depth` counts how many variable values deep this text was found; it is what stops a cycle of
/// variables naming each other from recursing forever.
fn eval_text(env: &mut Environment, expr: &str, depth: usize) -> Result<i64> {
    let spent = SPENT.with(|spent| {
        let now = spent.get() + 1;
        spent.set(now);
        now
    });
    if spent > MAX_EVALUATIONS {
        return Err(ShellError::ExpansionError(
            "arithmetic expression is too large to evaluate".to_string(),
        ));
    }
    // Expansion first, and over the whole string: POSIX runs parameter expansion, command
    // substitution and quote removal across the expression before any of it is arithmetic.
    let expanded = expand_expression(env, expr)?;
    let tokens = tokenize(&expanded)?;
    let ast = parser::parse(&tokens)?;
    eval(env, &ast, depth)
}

/// The text a reference currently holds, and — for an element — which slot it names.
///
/// Both are worked out together because the subscript is itself arithmetic and may have side
/// effects: `a[i++]` must evaluate `i++` exactly once, so the index is computed here and handed to
/// whoever needs it rather than being evaluated again on the way back.
fn read(
    env: &mut Environment,
    reference: &Ref,
    depth: usize,
) -> Result<(Option<i64>, Option<String>)> {
    let Some(index) = &reference.index else {
        return Ok((None, env.get_param(&reference.name)));
    };
    let index = eval(env, index, depth)?;
    let text = env
        .get_array(&reference.name)
        .and_then(|array| array.get(index))
        .map(str::to_string);
    Ok((Some(index), text))
}

/// Read a reference as a number.
///
/// A plain numeric value is that number. Anything else is re-evaluated as an expression, which is
/// how bash resolves `a=b; b=7` and how a variable holding `1+1` becomes 2. An unset variable, an
/// element that is not there, or one whose value evaluates to nothing recognisable, is 0.
fn resolve(env: &mut Environment, reference: &Ref, depth: usize) -> Result<i64> {
    let name = &reference.name;
    let Some(text) = read(env, reference, depth)?.1 else {
        // `set -u` covers a plain name; an element that is not there is still 0.
        if reference.index.is_none() && env.nounset() {
            return Err(ShellError::UnsetParameter(format!(
                "{name}: unbound variable"
            )));
        }
        return Ok(0);
    };
    if let Some(n) = lexer::literal_value(&text) {
        return Ok(n);
    }
    if text.trim().is_empty() {
        return Ok(0);
    }
    if depth >= MAX_RESOLVE_DEPTH {
        return Err(ShellError::ExpansionError(format!(
            "{name}: expression recursion level exceeded"
        )));
    }
    eval_text(env, &text, depth + 1)
}

/// Write a reference back, and answer the value written.
///
/// The subscript is evaluated here rather than reused from an earlier `resolve`, which is a real
/// difference for `a[i++] = 5`: bash evaluates the subscript once per *reference*, and the two
/// halves of `a[i++] += 1` are two references. Matching that exactly would need the index threaded
/// through, so it is written down here as the divergence it is — an assignment whose subscript has
/// a side effect steps it twice.
fn store(env: &mut Environment, reference: &Ref, value: i64, depth: usize) -> Result<i64> {
    let text = value.to_string();
    match &reference.index {
        None => {
            env.set_var(&reference.name, &text, false);
        }
        Some(index) => {
            let index = eval(env, index, depth)?;
            env.set_array_element(&reference.name, index, &text);
        }
    }
    Ok(value)
}

fn eval(env: &mut Environment, expr: &Expr, depth: usize) -> Result<i64> {
    match expr {
        Expr::Number(n) => Ok(*n),
        Expr::Var(reference) => resolve(env, reference, depth),
        Expr::Unary(op, operand) => {
            let v = eval(env, operand, depth)?;
            Ok(match op {
                UnOp::Pos => v,
                // `-i64::MIN` overflows, and `-9223372036854775808` is how `i64::MIN` is written.
                UnOp::Neg => v.wrapping_neg(),
                UnOp::Not => i64::from(v == 0),
                UnOp::BitNot => !v,
            })
        }
        Expr::Binary(op, left, right) => {
            let l = eval(env, left, depth)?;
            let r = eval(env, right, depth)?;
            apply(*op, l, r)
        }
        // The short-circuiting pair must not evaluate the right side at all: `0 && (1/0)` is 0 in
        // bash, not a division error, and `0 && (x = 1)` must leave `x` alone.
        Expr::LogicalAnd(left, right) => {
            if eval(env, left, depth)? == 0 {
                Ok(0)
            } else {
                Ok(i64::from(eval(env, right, depth)? != 0))
            }
        }
        Expr::LogicalOr(left, right) => {
            if eval(env, left, depth)? != 0 {
                Ok(1)
            } else {
                Ok(i64::from(eval(env, right, depth)? != 0))
            }
        }
        Expr::Conditional(cond, then, other) => {
            if eval(env, cond, depth)? != 0 {
                eval(env, then, depth)
            } else {
                eval(env, other, depth)
            }
        }
        Expr::Comma(left, right) => {
            eval(env, left, depth)?;
            eval(env, right, depth)
        }
        Expr::Assign(name, op, rhs) => {
            let value = match op {
                None => eval(env, rhs, depth)?,
                Some(op) => {
                    let current = resolve(env, name, depth)?;
                    let operand = eval(env, rhs, depth)?;
                    apply(*op, current, operand)?
                }
            };
            store(env, name, value, depth)
        }
        Expr::PreStep(name, delta) => {
            let value = resolve(env, name, depth)?.wrapping_add(*delta);
            store(env, name, value, depth)
        }
        Expr::PostStep(name, delta) => {
            let old = resolve(env, name, depth)?;
            store(env, name, old.wrapping_add(*delta), depth)?;
            Ok(old)
        }
    }
}

fn apply(op: BinOp, l: i64, r: i64) -> Result<i64> {
    Ok(match op {
        BinOp::Add => l.wrapping_add(r),
        BinOp::Sub => l.wrapping_sub(r),
        BinOp::Mul => l.wrapping_mul(r),
        BinOp::Div => {
            if r == 0 {
                return Err(ShellError::MalformedExpansion("division by 0".to_string()));
            }
            // `i64::MIN / -1` is the one non-zero divisor that overflows; bash yields `i64::MIN`
            // for it, which is what the checked failure wraps to.
            l.checked_div(r).unwrap_or(i64::MIN)
        }
        BinOp::Rem => {
            if r == 0 {
                return Err(ShellError::MalformedExpansion("division by 0".to_string()));
            }
            // Same overflow case; the mathematical remainder is 0 and bash agrees.
            l.checked_rem(r).unwrap_or(0)
        }
        BinOp::Pow => return power(l, r),
        // C leaves an over-wide shift undefined; every shell in practice inherits the hardware's
        // count-modulo-64, which is what Rust's `wrapping_sh*` does. `-1 as u32` lands on 63,
        // matching bash's `1 << -1` == i64::MIN.
        BinOp::Shl => l.wrapping_shl(r as u32),
        BinOp::Shr => l.wrapping_shr(r as u32),
        BinOp::BitAnd => l & r,
        BinOp::BitOr => l | r,
        BinOp::BitXor => l ^ r,
        BinOp::Eq => i64::from(l == r),
        BinOp::Ne => i64::from(l != r),
        BinOp::Lt => i64::from(l < r),
        BinOp::Le => i64::from(l <= r),
        BinOp::Gt => i64::from(l > r),
        BinOp::Ge => i64::from(l >= r),
    })
}

/// `base ** exp` with C wrapping.
///
/// Squaring rather than repeated multiplication: wrapping multiplication is arithmetic modulo
/// 2^64, so both give the same answer, but `2 ** 9223372036854775807` finishes here instead of
/// hanging the shell.
fn power(base: i64, exp: i64) -> Result<i64> {
    if exp < 0 {
        return Err(ShellError::ExpansionError(
            "exponent less than 0".to_string(),
        ));
    }
    let mut result: i64 = 1;
    let mut acc = base;
    let mut n = exp as u64;
    while n > 0 {
        if n & 1 == 1 {
            result = result.wrapping_mul(acc);
        }
        acc = acc.wrapping_mul(acc);
        n >>= 1;
    }
    Ok(result)
}

#[cfg(test)]
mod nesting_tests {
    use crate::env::Environment;
    use crate::expand::arithmetic::eval_arithmetic;

    /// **A `$(( ))` inside a variable's value is a door back into this module.**
    ///
    /// `eval_text` expands before it evaluates, because POSIX runs parameter expansion first — so
    /// the nested expression leaves through the word expander and re-enters `eval_arithmetic`,
    /// where `depth` starts again at zero. `a='$((a))'` recursed through that door until the stack
    /// ran out and the shell aborted; bash prints a syntax error and lives.
    #[test]
    fn a_variable_whose_value_re_enters_arithmetic_is_bounded() {
        let mut env = Environment::new();
        env.set_var("a", "$((a))", false);
        let answer = eval_arithmetic(&mut env, "a");
        assert!(answer.is_err(), "expected a refusal, got {answer:?}");
    }

    /// The same through a cycle of two, which is the shape a person actually writes by accident.
    #[test]
    fn a_cycle_of_two_is_bounded_too() {
        let mut env = Environment::new();
        env.set_var("a", "$((b))", false);
        env.set_var("b", "$((a))", false);
        assert!(eval_arithmetic(&mut env, "a").is_err());
    }

    /// …and honest nesting still evaluates, so the cap is not simply refusing everything.
    #[test]
    fn ordinary_nesting_still_evaluates() {
        let mut env = Environment::new();
        env.set_var("n", "3", false);
        assert_eq!(eval_arithmetic(&mut env, "$((n)) + 1").unwrap(), 4);
        assert_eq!(eval_arithmetic(&mut env, "2 + 3 * 4").unwrap(), 14);
        // A chain of values that are themselves expressions is the feature the cap must not break.
        env.set_var("x", "1+1", false);
        assert_eq!(eval_arithmetic(&mut env, "x * 3").unwrap(), 6);
    }

    /// The counter comes back down, so one refused expression does not poison the next.
    #[test]
    fn a_refusal_does_not_leave_the_counter_raised() {
        let mut env = Environment::new();
        env.set_var("a", "$((a))", false);
        for _ in 0..3 {
            assert!(eval_arithmetic(&mut env, "a").is_err());
        }
        assert_eq!(eval_arithmetic(&mut env, "2 + 2").unwrap(), 4);
    }
}
