//! How much stack is left, asked of the stack.
//!
//! # Why counting levels was not enough
//!
//! Three things in the shell re-enter the interpreter — a function calling itself, a `source` or
//! `eval` chain, and a nested compound command — and each had a counter of its own, checked against
//! a limit of its own. Each limit was measured while the other two were idle. They are not idle
//! together, and they spend one stack:
//!
//! ```text
//! source chain 49 deep -> function recursing 20 deep -> inside 45 levels of { ( … ) }
//!   thread 'oslo' has overflowed its stack
//! ```
//!
//! Twenty levels of recursion, where the limit permitted a hundred. No setting of the three
//! constants closes that, because how much stack a level costs depends on the *shape* of what is
//! nested, not on how many levels there are — and a limit low enough to be safe against the worst
//! shape would refuse ordinary scripts.
//!
//! So this measures the thing that actually runs out. A counter is a proxy; the stack pointer is
//! not.
//!
//! # What it costs
//!
//! Two loads and a subtraction, on paths that are about to fork a process or walk a syntax tree.
//! It is not on the hot path of anything.

use std::cell::Cell;

/// How much room to keep in reserve.
///
/// **Not "until it runs out".** The check happens *before* descending, and the level being refused
/// still has to unwind: an error travels back through every frame between here and the top, some of
/// which run `Drop` glue, and the diagnostic itself is formatted and printed on the way. Reporting
/// with nothing left over would overflow while reporting, which is the failure this exists to stop,
/// arriving one frame later.
///
/// **Sized for reporting, not for running.** Unwinding *pops* frames; what still has to fit is the
/// error's own path out — a `format!`, an `eprintln!`, and whatever `Drop` glue sits between here
/// and the top. That is kilobytes.
///
/// A megabyte was the first guess and it was too much: at 6% of a 16 MiB stack it refused programs
/// that had been completing, including a plain recursion to the depth the counter permits. 256 KiB
/// is still two orders of magnitude more than the report needs, and it leaves the count limit
/// reachable — the stack should bind when the stack is the problem, and not before.
const RESERVE: usize = 256 * 1024;

thread_local! {
    /// The highest address this thread's stack reaches, and how many bytes it may use.
    ///
    /// `(0, 0)` means nothing recorded it — a thread oslo did not start, or one that starts work
    /// before [`mark`]. [`headroom`] answers `None` there rather than guessing, and every caller
    /// treats "unknown" as "fine": a wrong refusal is worse than the crash it was guarding, because
    /// it would refuse scripts that are nowhere near the limit.
    static BOUNDS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

/// Roughly where the stack pointer is now.
///
/// The address of a local, which is within a frame of the real one. Nothing here needs better than
/// that: the reserve is a megabyte and the answer is used to compare against it.
#[inline]
fn here() -> usize {
    let anchor = 0u8;
    std::ptr::addr_of!(anchor) as usize
}

/// Record this thread's stack as `limit` bytes reaching down from about here.
///
/// Call it first thing on a thread that runs the interpreter, before anything it calls has pushed a
/// frame worth counting. Calling it twice is harmless and the last call wins.
pub fn mark(limit: usize) {
    BOUNDS.set((here(), limit));
}

/// Bytes still available, or `None` if this thread never called [`mark`].
pub fn headroom() -> Option<usize> {
    let (base, limit) = BOUNDS.get();
    if limit == 0 {
        return None;
    }
    // Stacks grow down, so the base is the *highest* address and `here` has descended from it. A
    // thread that somehow reads above its own base has used nothing.
    let used = base.saturating_sub(here());
    Some(limit.saturating_sub(used))
}

/// Whether descending one more level is close enough to the end to refuse it.
///
/// False when nothing recorded this thread's bounds — unknown is never a refusal, because a wrong
/// refusal would stop scripts nowhere near the limit.
pub fn exhausted() -> bool {
    headroom().is_some_and(|left| left < RESERVE)
}

#[cfg(test)]
#[path = "stack/tests.rs"]
mod tests;
