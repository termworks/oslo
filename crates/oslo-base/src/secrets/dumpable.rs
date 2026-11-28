//! Keeping key material out of core dumps and away from another process's `ptrace`.
//!
//! A process that is not dumpable writes no core file, and the kernel refuses a `ptrace` attach, a
//! `/proc/PID/mem` read and most of `/proc/PID/` to every process without `CAP_SYS_PTRACE`. An
//! `execve` of an ordinary binary makes the new image dumpable again, so a child is unaffected.
//!
//! Two uses:
//!
//! * [`for_good`], in `oslo secret`: the process exits or execs, so nothing is restored.
//! * [`Hidden`], around one seal or unseal in a long-lived shell, which has to be dumpable again
//!   afterwards: `/proc/PID/cwd` is what a terminal reads to open a tab in the same directory, and a
//!   crash of an interactive shell still wants its core file.

use nix::libc::{PR_GET_DUMPABLE, PR_SET_DUMPABLE, c_int, c_ulong, prctl};
use std::sync::{Mutex, MutexGuard};

/// How many guards are alive, and whether the last one to go makes the process dumpable again.
static HELD: Mutex<(usize, bool)> = Mutex::new((0, false));

fn held() -> MutexGuard<'static, (usize, bool)> {
    HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn get() -> c_int {
    // SAFETY: PR_GET_DUMPABLE reads no argument and answers in the return value.
    unsafe {
        prctl(
            PR_GET_DUMPABLE,
            0 as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
        )
    }
}

fn set(dumpable: bool) -> c_int {
    // SAFETY: PR_SET_DUMPABLE takes one integer and no pointer.
    unsafe {
        prctl(
            PR_SET_DUMPABLE,
            dumpable as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
        )
    }
}

/// Not dumpable from now on, whatever guard is alive.
pub fn for_good() {
    let mut held = held();
    set(false);
    held.1 = false;
}

/// Not dumpable while any of these lives; dumpable again after the last one, if it was before.
///
/// Only a process at `1` is changed and restored: `0` is already hidden, and `2`, which only
/// `execve` sets, would come back as `1` and be weaker than it was.
#[must_use = "the process is dumpable again as soon as this is dropped"]
pub struct Hidden(());

impl Hidden {
    pub fn new() -> Hidden {
        let mut held = held();
        if held.0 == 0 {
            held.1 = get() == 1 && set(false) == 0;
        }
        held.0 += 1;
        Hidden(())
    }
}

impl Default for Hidden {
    fn default() -> Self {
        Hidden::new()
    }
}

impl Drop for Hidden {
    fn drop(&mut self) {
        let mut held = held();
        held.0 -= 1;
        if held.0 == 0 && held.1 {
            set(true);
            held.1 = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hidden while a guard lives, and back to what it was once no guard is left.
    #[test]
    fn a_guard_hides_and_restores() {
        let before = {
            let held = held();
            if held.0 > 0 { 1 } else { get() }
        };
        if before != 1 {
            return; // A test process that is not dumpable to begin with has nothing to hide.
        }
        {
            let _outer = Hidden::new();
            assert_eq!(get(), 0, "not hidden inside the guard");
            {
                let _inner = Hidden::new();
                assert_eq!(get(), 0);
            }
            assert_eq!(get(), 0, "the inner guard restored too early");
        }
        // Another test's guard may still be alive; read both under the lock the guards take.
        let held = held();
        assert!(held.0 > 0 || get() == 1, "not restored");
    }
}
