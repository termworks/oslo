//! The signal mask `main` arranges around handing the shell to its worker thread.
//!
//! **`kill` aimed at a *process* lands on any thread that is not blocking the signal.** With
//! `main` merely parked in `join`, the kernel was free to deliver there — and `kill -USR1 $$`
//! returned to the shell before its own trap had run, printing the next command's output first.
//! Blocking everything before the spawn and restoring it on the worker leaves exactly one
//! candidate thread, which is what keeps the single-threaded ordering the rest of the shell is
//! written against.

use nix::sys::signal::{SigSet, SigmaskHow, pthread_sigmask};

/// Block every signal on the calling thread, answering the mask that was in force.
pub(crate) fn block_every_signal() -> SigSet {
    let mut previous = SigSet::empty();
    let _ = pthread_sigmask(
        SigmaskHow::SIG_SETMASK,
        Some(&SigSet::all()),
        Some(&mut previous),
    );
    previous
}

/// Put a saved mask back, so the shell starts with whatever its caller handed it.
pub(crate) fn restore_signal_mask(mask: &SigSet) {
    let _ = pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(mask), None);
}
