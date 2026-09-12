//! Bounded recursion counters for constructs that re-enter the interpreter.
//!
//! A shell function calling itself, a file sourcing itself and `eval` evaluating text that calls
//! `eval` all recurse through the whole evaluator. Rust turns the resulting stack overflow into
//! `SIGABRT`, so before these counters existed `f() { f; }; f` died with status 134 and a core
//! dump — no diagnostic, no chance for the shell to react. A counter cannot make the recursion
//! safe, but it can stop it while there is still stack left to report the error on.

use oslo_base::error::{Result, ShellError};

/// Deepest shell-function nesting, in the spirit of bash's `FUNCNEST`.
///
/// Measured, not guessed, and measured **again** because the first numbers went stale: the
/// interpreter runs on its own thread with a fixed 16 MiB stack, so `ulimit -s` does not reach it
/// and a debug build overflows at about 1,010 levels of plain function recursion — not the 300 to
/// 400 on 8 MiB this comment used to claim. Both figures were checked by lifting the cap and
/// bisecting, under `ulimit -s 8192` and `16384` alike, which answered identically.
///
/// **100 was refusing ordinary code.** `f 500` runs in bash and in dash; oslo alone answered
/// `maximum nesting level exceeded`, and recursive shell functions — a tree walk, a parser — reach
/// a few hundred without trying.
///
/// **A count is not what runs out, so this is no longer the only check.** Three things spend one
/// stack — this, the `source`/`eval` chain below, and nested compound commands — and each limit was
/// measured while the other two were idle. Together they were not: a `source` chain 49 deep calling
/// a function recursing 20 deep inside 45 levels of `{ ( … ) }` overflowed and aborted while this
/// counter said ninety-odd levels were free, at the old 100 exactly as at this 1000.
///
/// [`oslo_base::stack`] measures the stack itself, and [`DepthGuard::enter`] asks it as well as the
/// count. This number is now the *ceiling on how deep a script may sensibly go*, not the thing
/// standing between the shell and a crash.
pub const MAX_FUNCTION_DEPTH: usize = 1000;

/// Deepest nesting of `source` and `eval`, which re-enter the parser as well as the evaluator.
///
/// Shared by both because they are the same hazard: a file that sources itself and
/// `x='eval "$x"'; eval "$x"` recurse identically.
pub const MAX_SCRIPT_DEPTH: usize = 50;

/// A depth counter that refuses to go past its limit.
///
/// Every entry point is paired: `enter` on the way in, `exit` on the way out *whatever* the
/// outcome, so an unwinding `return`, `break` or error cannot leave the count drifting upwards
/// and eventually refuse a call that is nowhere near the limit.
pub struct DepthGuard {
    depth: usize,
    limit: usize,
}

impl DepthGuard {
    pub const fn new(limit: usize) -> Self {
        Self { depth: 0, limit }
    }

    /// Descend one level, or fail if that would exceed the limit — or the stack.
    ///
    /// **The count is the cheap answer and the stack is the true one.** A level costs a different
    /// amount of stack depending on what is nested inside it, and this counter is one of three
    /// spending the same stack, so its limit alone was never a guarantee: twenty levels of
    /// recursion inside enough nesting overflowed while the count said a hundred were free. See
    /// [`oslo_base::stack`].
    pub fn enter(&mut self) -> Result<()> {
        if self.depth >= self.limit || oslo_base::stack::exhausted() {
            return Err(ShellError::ExecutionError(
                "maximum nesting level exceeded".to_string(),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    /// Come back up one level. Saturating: an unpaired `exit` must not wrap to `usize::MAX`.
    pub fn exit(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub fn depth(&self) -> usize {
        self.depth
    }
}

#[cfg(test)]
mod tests {
    use super::DepthGuard;

    #[test]
    fn enters_up_to_the_limit_then_refuses() {
        let mut g = DepthGuard::new(3);
        for _ in 0..3 {
            assert!(g.enter().is_ok());
        }
        let err = g.enter().expect_err("the fourth level is over the limit");
        assert!(
            err.to_string().contains("maximum nesting level exceeded"),
            "{err}"
        );
        // A refused entry must not have counted: unwinding still calls `exit` only for the
        // levels that were actually entered.
        assert_eq!(g.depth(), 3);
    }

    #[test]
    fn unwinding_restores_the_budget() {
        let mut g = DepthGuard::new(2);
        for _ in 0..8 {
            assert!(g.enter().is_ok());
            assert!(g.enter().is_ok());
            assert!(g.enter().is_err());
            g.exit();
            g.exit();
        }
        assert_eq!(g.depth(), 0);
    }

    #[test]
    fn exit_without_enter_stays_at_zero() {
        let mut g = DepthGuard::new(1);
        g.exit();
        assert_eq!(g.depth(), 0);
        assert!(g.enter().is_ok());
    }
}
