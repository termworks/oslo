//! A prompt that moves while nothing is typed.
//!
//! A spinner beside a long-running job, a clock, a segment that fades — all the same shape: the
//! prompt has to be redrawn on a schedule rather than in answer to something. Two things stand in
//! the way, and this file is both of them.
//!
//! # The editor is blocked, so the schedule has to be part of the wait
//!
//! [`crate::pending::wake_in`] already exists for exactly this — a provider's debounce asking for a
//! turn at a stated time — and the input wait already shortens itself to whichever moment is
//! nearest. An animation is another caller of it, not another mechanism.
//!
//! # Rebuilding the prompt is not free, and a tick must not pay for it
//!
//! [`super::invalidate`] means *something changed*: the directory, a variable, the branch. Every
//! segment is rebuilt, which is right, and at a keystroke's pace it costs nothing.
//!
//! A tick is not that. Ten times a second, re-running a segment that shells out to `git` would make
//! a spinner the most expensive thing in the shell. So there are **two counters**: one that says
//! the prompt string is stale, and one that says the *segments behind it* are. A tick moves the
//! first and leaves the second, and a renderer that caches by segment then re-runs only the ones
//! that asked to be re-run.
//!
//! The renderer is `oslo-runtime`'s, because segments are Lua. This holds the clock and the two
//! counters, which are the parts the editor has to see.

use std::cell::Cell;
use std::time::{Duration, Instant};

/// Bumped only when a segment's *content* could have changed — never by a tick.
///
/// A cache of rendered segments is stale when this moves, and only then. Separate from
/// [`super::generation`], which moves for both, because a redraw and a rebuild are different
/// questions and answering them with one number means a spinner rebuilds the branch name.
static CONTENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The reading a cache stores alongside what it cached.
pub fn content_generation() -> u64 {
    CONTENT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Say that the segments themselves are stale. Called by [`super::invalidate`], and nowhere else.
pub(super) fn content_changed() {
    CONTENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

thread_local! {
    /// When the next frame of an animation is due.
    ///
    /// Thread-local, like [`crate::pending`]'s own deadline and for the same reason: the editor is
    /// the only thread that sets it and the only one that asks.
    static NEXT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Ask for the prompt to be redrawn in `after`, without saying anything is stale yet.
///
/// Called by whatever rendered a segment that wants to move, with that segment's own interval. The
/// wait is shortened to the nearest of everything asking, so several animated segments at different
/// speeds cost one timer between them.
pub fn animate_in(after: Duration) {
    // **A frame nobody can see is work nobody asked for**, and for an external prompt it is a
    // *process* nobody asked for: `oslo.prompt.left = { command = …, every = 150 }` is a spawn per
    // frame, for as long as the shell is open, whether or not anything is on the screen.
    //
    // That is the whole of a bug found by a program driving oslo over a pty as a command channel.
    // It looked like a per-keystroke render — two `git status` per character typed — and it was
    // not: it was this clock, ticking six times a second into a terminal that draws nothing.
    // Measured on a 0×0 pty with `TERM=dumb`: 53 spawns in eight seconds, against one with this
    // check in place.
    //
    // The deadline is simply never armed. `tick_due` is therefore never due, the wait goes back to
    // blocking on a key, and the prompt is rendered once per prompt — which is what a terminal that
    // cannot draw wanted from the start.
    if !crate::term::draws() {
        return;
    }
    let want = Instant::now() + after;
    NEXT.with(|next| match next.get() {
        // Somebody already wants a turn sooner. Theirs is the deadline; this one comes round on the
        // frame after it, because every animated segment is asked again on every frame.
        Some(sooner) if sooner <= want => {}
        _ => next.set(Some(want)),
    });
    crate::pending::wake_in(after);
}

/// Whether a frame is due, clearing the deadline if it is.
///
/// **Clearing is what stops a spinner spinning.** The deadline is re-armed by rendering, so a
/// segment that stops asking simply stops being asked, and the wait goes back to blocking on a key.
pub fn tick_due() -> bool {
    NEXT.with(|next| match next.get() {
        Some(want) if Instant::now() >= want => {
            next.set(None);
            true
        }
        _ => false,
    })
}

/// Forget any pending frame. For the moment a line is accepted, and for tests.
pub fn settle() {
    NEXT.with(|next| next.set(None));
}

#[cfg(test)]
#[path = "animation/tests.rs"]
mod tests;
