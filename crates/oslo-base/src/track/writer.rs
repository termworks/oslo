//! The store's writes, off the thread the prompt is drawn on.
//!
//! What makes these writes movable is what they *are*. They are records of something that has
//! already happened: which directory a command ran in, what it exited with, how long it took.
//! Nothing the next prompt draws is computed from them, so the only thing that was ever waiting on
//! them landing was the person about to type the next command.
//!
//! **What it is worth, honestly.** Measured on this machine it moves about 0.2 ms of CPU per
//! command off the prompt thread, and changes Enter-to-prompt-ready by nothing at all — that path
//! is 0.4 ms either way. `fsync` on this filesystem costs 0.000 ms, so the commits were never
//! waiting on a disk here. It pays where the commit really blocks: an encrypted or network home, a
//! filesystem that honours barriers, a machine under load. It is not a latency fix for this one.
//!
//! # One thread, and a queue in order
//!
//! The writes are not independent. A boundary resolves the directory that the outcome written after
//! it names; a log row is rewritten before the outcome that joins to it. Two threads would reorder
//! them the first time one commit was slower than another. So: one thread, one queue, and the order
//! jobs are handed over is the order they are written — the same order the prompt thread used to
//! write them in itself.
//!
//! # What it costs
//!
//! A shell that dies between a command and the next prompt — `kill -9`, a power cut — loses that
//! command's tracking record where before it would have had it. It does **not** lose the command
//! *line*: the log is appended before the command runs and is written on the prompt thread, which
//! is the whole reason that one write stays there. See `repl::precmd::write_down`, which records
//! what deferring it cost when that was tried.
//!
//! Every ordinary way out waits, and "ordinary" has to include the way that never comes back:
//! [`settle`] is called from `settle_stores` on the way out of the loop, and from the `exec`
//! builtin immediately before it replaces the process image. `exec $SHELL` is how most people
//! restart a shell, and skipping the barrier there lost the tail of the session.

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

/// One write, already holding everything it needs.
///
/// A closure rather than a description of the work, because the decisions behind these writes — a
/// `pre-record` rule's answer, whether a chain kept one link or all of them — are made on the
/// prompt thread against Lua and thread-local state. Sending a *description* would mean deciding
/// twice, in two places, and the two would drift.
type Job = Box<dyn FnOnce() + Send>;

static QUEUE: OnceLock<Option<Sender<Job>>> = OnceLock::new();

/// The process the writer thread belongs to.
///
/// **A thread does not survive `fork`, and a channel does.** Only the forking thread continues in
/// the child, so the `oslo-history` loop is gone there — but the `Receiver` it was looping over is
/// still sitting in the memory the child inherited, so `send` succeeds and the job is queued into
/// something nothing will ever drain. [`settle`] then waited on it for the life of the process.
///
/// Recorded here rather than announced by each fork site, because every one of them would have to
/// remember: a subshell, a pipeline stage, a command substitution and a background job all fork,
/// and the `exec` builtin is reachable from all four.
static OWNER: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Whether the writer thread is this process's own.
fn ours() -> bool {
    let owner = OWNER.load(std::sync::atomic::Ordering::SeqCst);
    owner != 0 && owner == std::process::id() as i32
}

/// The queue, starting the writer thread the first time anything is deferred.
///
/// `None` if the thread could not be started, which is not a reason to lose a write — see [`defer`].
fn queue() -> Option<&'static Sender<Job>> {
    QUEUE
        .get_or_init(|| {
            let (send, receive) = channel::<Job>();
            std::thread::Builder::new()
                .name("oslo-history".to_string())
                .spawn(move || {
                    for job in receive {
                        // A job that panics must not take the rest of the session's history with
                        // it. The store's own transactions already unwind cleanly; this is the
                        // outer guard, so one bad record costs one record.
                        //
                        // **Inert in a release build**, where `panic = "abort"` leaves nothing to
                        // catch — see the note on that setting in the workspace manifest. It holds
                        // in a debug build and under `cargo test`.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    }
                })
                .ok()
                .map(|_| {
                    OWNER.store(
                        std::process::id() as i32,
                        std::sync::atomic::Ordering::SeqCst,
                    );
                    send
                })
        })
        .as_ref()
}

/// Hand a write to the writer thread.
///
/// Runs it here instead if there is no writer thread — a process that could not spawn one still
/// records its history, slowly, rather than silently recording none of it.
pub fn defer(job: impl FnOnce() + Send + 'static) {
    let job: Job = Box::new(job);
    match queue() {
        Some(queue) => {
            if let Err(returned) = queue.send(job) {
                returned.0();
            }
        }
        None => job(),
    }
}

/// Block until every write queued so far has been committed.
///
/// A barrier rather than a shutdown: the queue stays open, so this can be called on the way out and
/// by anything that needs to read back what it just wrote. Returns at once for a process that never
/// deferred anything, which is every non-interactive shell.
pub fn settle() {
    let Some(Some(queue)) = QUEUE.get() else {
        return;
    };
    // **Not in a forked child**, where the thread that would answer does not exist — see [`OWNER`].
    // The queued jobs belong to the parent and it will settle them; there is nothing here to wait
    // for, and waiting hung every subshell that reached `exec`.
    if !ours() {
        return;
    }
    let (done, wait) = channel();
    if queue
        .send(Box::new(move || {
            let _ = done.send(());
        }))
        .is_err()
    {
        return;
    }
    // An error means the writer thread is gone and with it every job ahead of this one — there is
    // nothing left to wait for either way.
    let _ = wait.recv();
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    /// The whole contract: everything runs, in the order it was handed over, and [`super::settle`]
    /// is the point after which that is true.
    ///
    /// Order is not a nicety here — a boundary resolves the directory the outcome written after it
    /// names, so two jobs that swapped would attribute a command to the wrong place.
    #[test]
    fn jobs_run_in_order_and_settle_waits_for_them() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        // Slow enough that the queue is certainly still draining when `settle` is called, so this
        // fails without the barrier rather than only on an unlucky machine.
        super::defer(|| std::thread::sleep(std::time::Duration::from_millis(150)));
        for i in 0..200 {
            let seen = Arc::clone(&seen);
            super::defer(move || seen.lock().unwrap().push(i));
        }
        super::settle();

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 200, "settle returned with work outstanding");
        assert!(
            seen.iter().copied().eq(0..200),
            "jobs ran out of the order they were queued in"
        );
    }

    /// A job that panics costs one job, not the rest of the session's history.
    #[test]
    fn a_panicking_job_does_not_stop_the_queue() {
        let after = Arc::new(Mutex::new(false));
        super::defer(|| panic!("a job that could not be written"));
        let flag = Arc::clone(&after);
        super::defer(move || *flag.lock().unwrap() = true);
        super::settle();

        assert!(*after.lock().unwrap(), "the queue died with the bad job");
    }
}
