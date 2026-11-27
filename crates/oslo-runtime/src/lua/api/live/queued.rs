//! What a connected peer asked the shell to *do*, waiting for the thread that may do it.
//!
//! Reading is answered where the question arrives: [`super::super::live`]'s verbs take the
//! environment lock on the server thread and reply. Changing the shell cannot be, and the reason is
//! not the lock.
//!
//! # Why a queue and not just a `chdir`
//!
//! `set_current_dir` is **process-wide**. A server thread calling it moves the ground under every
//! other thread in the shell, including one that is part-way through resolving a relative path for
//! an `exec`. This codebase has already paid for that lesson somewhere quieter — `tests/common`
//! records three flaky tests caused by exactly this — and a shell is a far worse place to learn it
//! again: the failure is a command that runs in the wrong directory, occasionally.
//!
//! So the server thread records what was asked and wakes the shell, and the shell does it at the
//! same safe point it reaps children and fires timers at: on its own thread, between keystrokes,
//! holding nothing. [`oslo_base::background`] already owns that machinery, and this only registers
//! one more descriptor with it.
//!
//! # What the peer is told
//!
//! That it was **accepted**, not that it happened — because at the moment of the reply it has not.
//! A shell running a foreground command gets there when the command ends. Claiming otherwise would
//! be a lie that a caller could act on, and the honest answer costs nothing to give.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

/// The directory the shell should move to, if a peer has asked for one.
///
/// One slot rather than a queue: two peers asking at once means one of them is about to be
/// overwritten either way, and a backlog of directories to visit in turn is not a thing anybody
/// wants — the last word is the answer, as it would be for two `cd`s typed quickly.
static WANTED: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The pipe the shell's input wait watches. Read end first.
static NUDGE: OnceLock<(i32, i32)> = OnceLock::new();

/// Open the wake pipe and register it with the editor's wait.
///
/// Idempotent, and the descriptors are never closed: [`oslo_base::background::wake_on`] borrows the
/// number for the life of the shell, and one that closed would leave the editor waking on whatever
/// the kernel handed the number to next.
pub fn arm() -> bool {
    let (read, _) = *NUDGE.get_or_init(|| {
        let mut ends = [0i32; 2];
        // `O_CLOEXEC` so a spawned command does not inherit the shell's private wake, and
        // `O_NONBLOCK` so the write below can never park the server thread — the same two flags,
        // for the same two reasons, as `oslo_base::background`'s own self-pipe.
        // SAFETY: a live array of two descriptors, which is what `pipe2` fills.
        let made = unsafe {
            nix::libc::pipe2(
                ends.as_mut_ptr(),
                nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK,
            )
        };
        if made != 0 {
            return (-1, -1);
        }
        oslo_base::background::wake_on(ends[0]);
        (ends[0], ends[1])
    });
    read >= 0
}

/// Ask the shell to move, from whatever thread is asking. Answers whether the shell can be told.
pub fn ask(dir: PathBuf) -> bool {
    let Some((_, write)) = NUDGE.get().copied() else {
        return false;
    };
    if write < 0 {
        return false;
    }
    match WANTED.lock() {
        Ok(mut wanted) => *wanted = Some(dir),
        Err(_) => return false,
    }
    // One byte, and a full pipe is not a failure: it means a wake is already pending, which is the
    // thing this was trying to arrange.
    let byte = b".";
    // SAFETY: `write` is a pipe descriptor this module opened and never closes.
    unsafe {
        nix::libc::write(write, byte.as_ptr().cast(), 1);
    }
    true
}

/// Take what was asked for, on the shell thread. `None` when nothing is waiting.
pub fn take() -> Option<PathBuf> {
    WANTED.lock().ok()?.take()
}

#[cfg(test)]
#[path = "queued/tests.rs"]
mod tests;
