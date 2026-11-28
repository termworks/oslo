//! The arithmetic comparisons of `[[ ]]`.
//!
//! Inside `[[ ]]` both operands of `-eq`, `-ne`, `-lt`, `-le`, `-gt` and `-ge` are arithmetic
//! expressions, as in `(( ))`: an empty operand is 0, a name is evaluated, and `1+1`, `0x10`,
//! `2#101` and `010` (octal) all mean what they mean there. `test` and `[` stay decimal-only.
//!
//! An expression that does not evaluate makes the test fail with status 1 after a diagnostic,
//! except an unset name under `set -u`, which ends the shell as any expansion would.

use crate::env::origin_now;
use crate::env::scope::Environment;
use crate::expand::arithmetic::eval_arithmetic;
use oslo_base::error::{Result, ShellError};

/// Whether `op` is one of the six arithmetic comparisons.
pub(super) fn is_arithmetic_op(op: &str) -> bool {
    matches!(op, "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge")
}

/// `[[ left op right ]]` as an exit status.
pub(super) fn compare(env: &mut Environment, left: &str, op: &str, right: &str) -> Result<i32> {
    let left = match operand(env, left) {
        Ok(value) => value,
        Err(e) => return failed(e),
    };
    let right = match operand(env, right) {
        Ok(value) => value,
        Err(e) => return failed(e),
    };
    let truth = match op {
        "-eq" => left == right,
        "-ne" => left != right,
        "-lt" => left < right,
        "-le" => left <= right,
        "-gt" => left > right,
        _ => left >= right,
    };
    Ok(if truth { 0 } else { 1 })
}

/// One operand's value: an empty or all-blank operand is 0.
fn operand(env: &mut Environment, text: &str) -> Result<i64> {
    if text.trim().is_empty() {
        return Ok(0);
    }
    eval_arithmetic(env, text)
}

/// Report an evaluation error as a false test, or unwind if it is fatal.
fn failed(error: ShellError) -> Result<i32> {
    if matches!(
        error,
        ShellError::UnsetParameter(_) | ShellError::Discard(..)
    ) {
        return Err(error);
    }
    eprintln!("{}[[: {error}", origin_now());
    Ok(1)
}
