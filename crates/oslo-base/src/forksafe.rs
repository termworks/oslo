//! Forking from a process that has threads.
//!
//! `fork` keeps only the calling thread. A lock another thread held at that instant is still held
//! in the child and nothing there will ever release it, so a child that takes the same lock stops
//! there for good, with the parent blocked in `waitpid` behind it.
//!
//! Both halves of that are in this shell. It forks for every subshell, pipeline stage and command
//! substitution, and those children run the evaluator rather than `exec`ing. It also keeps
//! detached threads for the `$PATH` scan, the prediction model and the daily prune, each of which
//! takes a process-wide lock. `$(typpo)` reaches `command_index::nearest` in the child and wants
//! the `$PATH` cache lock the warming thread is holding.
//!
//! One gate settles it. A thread wraps the region in which it holds a process-wide lock in
//! [`guarded`]; a fork goes through [`quiesced`], which waits for those regions to end and holds
//! them off until the fork has returned. The child inherits every one of them unlocked.
//!
//! Nothing may fork from inside a [`guarded`] region — see [`quiesced`].

use std::cell::Cell;
use std::sync::RwLock;

static GATE: RwLock<()> = RwLock::new(());

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// How many [`guarded`] regions this thread is inside, counted for the length of each.
struct Nesting;

impl Nesting {
    /// Enter one, and say whether it is the outermost on this thread.
    fn enter() -> (Nesting, bool) {
        let outermost = DEPTH.with(|depth| {
            let was = depth.get();
            depth.set(was + 1);
            was == 0
        });
        (Nesting, outermost)
    }
}

impl Drop for Nesting {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Run `work`, during which no [`quiesced`] fork may start.
///
/// Wraps the acquisition of a process-wide lock, not a whole thread body: every fork in the
/// process waits for this to return.
///
/// Nesting is free. Only the outermost region takes the gate, because a second read guard on one
/// thread deadlocks against a writer that is already waiting.
pub fn guarded<T>(work: impl FnOnce() -> T) -> T {
    let (_nesting, outermost) = Nesting::enter();
    if !outermost {
        return work();
    }
    let _gate = GATE.read().unwrap_or_else(|held| held.into_inner());
    work()
}

/// Run `fork` with no [`guarded`] region in flight.
///
/// The gate is held across the call and dropped in both processes, so the child starts with it
/// free and can fork again itself.
///
/// A fork from inside a [`guarded`] region would wait on this thread's own read guard, so it is
/// let through ungated instead: the hazard the gate exists for is worse as a certainty than as a
/// race. Nothing in the shell does it, and a debug build asserts as much.
pub fn quiesced<T>(fork: impl FnOnce() -> T) -> T {
    let inside = DEPTH.with(|depth| depth.get()) > 0;
    debug_assert!(!inside, "forked from inside a guarded region");
    if inside {
        return fork();
    }
    let _gate = GATE.write().unwrap_or_else(|held| held.into_inner());
    fork()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Stands in for the `$PATH` cache: held by a background thread, wanted by a forked child.
    static PROBE: Mutex<u32> = Mutex::new(0);

    /// Both forking tests want `PROBE`, so they may not overlap.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// `pid`'s exit status, or `None` if it had not exited within `budget`.
    ///
    /// Polled rather than waited on: a child that deadlocked would otherwise hang the suite, which
    /// is the failure this whole module is about.
    fn exit_status_within(pid: i32, budget: Duration) -> Option<i32> {
        let deadline = Instant::now() + budget;
        let mut raw = 0;
        while Instant::now() < deadline {
            let seen = unsafe { libc::waitpid(pid, &mut raw, libc::WNOHANG) };
            if seen == pid {
                return Some(libc::WEXITSTATUS(raw));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// Hold `PROBE` on another thread, fork a child that wants it, and report what the child did.
    ///
    /// With `gated` the held region and the fork go through the gate; without it they do not, which
    /// is the shell as it was.
    fn child_wanting_a_held_lock(gated: bool, budget: Duration) -> Option<i32> {
        let worker = std::thread::spawn(move || {
            let hold = || {
                let _probe = PROBE.lock().unwrap_or_else(|held| held.into_inner());
                std::thread::sleep(Duration::from_millis(300));
            };
            match gated {
                true => guarded(hold),
                false => hold(),
            }
        });
        std::thread::sleep(Duration::from_millis(50));

        let fork = || unsafe { libc::fork() };
        let pid = match gated {
            true => quiesced(fork),
            false => fork(),
        };
        if pid == 0 {
            // Only the lock and `_exit`: nothing else in a child of a threaded process is safe.
            let _probe = PROBE.lock();
            unsafe { libc::_exit(7) }
        }

        let status = exit_status_within(pid, budget);
        if status.is_none() {
            unsafe { libc::kill(pid, libc::SIGKILL) };
            let _ = exit_status_within(pid, Duration::from_secs(5));
        }
        let _ = worker.join();
        status
    }

    #[test]
    fn a_fork_waits_for_a_guarded_region() {
        let _serial = SERIAL.lock().unwrap_or_else(|held| held.into_inner());
        assert_eq!(
            child_wanting_a_held_lock(true, Duration::from_secs(20)),
            Some(7),
            "the child never got the lock the background thread was holding"
        );
    }

    /// What the gate is for, and what makes the test above mean something.
    #[test]
    fn without_the_gate_the_child_never_gets_the_lock() {
        let _serial = SERIAL.lock().unwrap_or_else(|held| held.into_inner());
        assert_eq!(
            child_wanting_a_held_lock(false, Duration::from_secs(3)),
            None,
            "an ungated fork did not deadlock, so nothing here is being tested"
        );
    }

    /// One guarded region inside another, with a fork waiting on the gate the whole time.
    ///
    /// Taking the gate twice would stop here instead, because the queued writer blocks the second
    /// read guard. The store nests its own transactions, so this is a path the shell takes.
    #[test]
    fn a_nested_region_does_not_wait_on_itself() {
        let (ready, waiting) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();

        let inner = std::thread::spawn(move || {
            guarded(|| {
                ready.send(()).ok();
                std::thread::sleep(Duration::from_millis(200));
                guarded(|| done.send(()).ok());
            })
        });

        waiting.recv_timeout(Duration::from_secs(5)).expect("outer");
        let wanting_the_gate = std::thread::spawn(|| quiesced(|| ()));

        finished
            .recv_timeout(Duration::from_secs(5))
            .expect("a guarded region inside another waited on its own read guard");
        inner.join().expect("inner");
        wanting_the_gate.join().expect("the fork never ran");
    }

    #[test]
    fn depth_unwinds_with_a_panic() {
        let fell_over = std::panic::catch_unwind(|| guarded(|| panic!("inside")));
        assert!(fell_over.is_err(), "the panic was swallowed");
        assert_eq!(DEPTH.with(|depth| depth.get()), 0, "depth was left raised");
    }
}
