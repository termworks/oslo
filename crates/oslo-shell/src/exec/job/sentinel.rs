//! Ctrl-C enough times takes the terminal back from a job that will not listen.
//!
//! Off unless `oslo.misc.interrupt_escape` is set to how many presses it should take. Nothing here
//! runs — not even the fork — in a shell that has not asked for it.
//!
//! # Why this needs a second process at all
//!
//! A shell doing job control is **not in the terminal's foreground process group** — it hands that
//! to the job so the job can read the terminal and receive Ctrl-C. The kernel delivers the tty's
//! `SIGINT` to the foreground group and nowhere else, so the shell, sitting in `waitpid`, never
//! learns a key was pressed. Everything oslo knows about an interrupted command it infers *after
//! the fact*, from the child's wait status — see [`super::super::simple::external`].
//!
//! That inference is enough for a job that dies. It is no use at all for one that does not:
//!
//! ```text
//! $ sh -c 'trap "" INT; sleep 300'
//! ^C ^C ^C          nothing happens, and nothing can — the shell is not being signalled
//! ```
//!
//! Something has to *observe* the Ctrl-C and act, and the only way to observe it is to be in the
//! group the kernel is signalling.
//!
//! So: one small process, forked once per interactive session, that joins whichever process group
//! currently owns the terminal and counts the interrupts it receives. It reads no input, writes no
//! output, and holds no terminal.
//!
//! # It stops the job; it does not kill it
//!
//! `SIGSTOP`, not `SIGKILL`, and the difference is most of the value. It cannot be caught or
//! ignored — so it works on exactly the programs this exists for — and `waitpid` already returns
//! `Stopped` for it, which means the shell's existing path takes over: the job goes into the job
//! table, the terminal comes back, and the prompt returns. Nothing is destroyed. `fg`, `bg` and
//! `kill %1` all then work on it, so the decision about what to do with a job that will not
//! listen stays with the person, where it belongs.
//!
//! That also means there is **no new signal aimed at the shell**, and so nothing for a user `trap`
//! to collide with — a shell-side handler could be replaced by `trap ... USR1` and the feature
//! would silently stop working.
//!
//! # What it will not save you from
//!
//! **A process wedged in an uninterruptible kernel call.** `SIGSTOP` is recorded and delivered when
//! the syscall returns, exactly as `SIGKILL` is, so a task blocked on a dead NFS mount is beyond
//! this and beyond everything else. What this helps with is a program that catches or ignores the
//! interrupt, or spins retrying it, which is the case a person can otherwise do nothing about.
//!
//! # The counter resets per job, not on a timer
//!
//! Three presses *during one command* escalate. A window would need a clock and would make the
//! behaviour depend on how fast somebody types; "I have now asked three times and it is still
//! there" is the thing being expressed, and it is as true over ten seconds as over one.

use nix::libc;
use nix::sys::signal::{self, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::unistd::{ForkResult, Pid, fork, setpgid};
use std::io::{Read, Write};
use std::os::unix::io::{FromRawFd, IntoRawFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

/// In the *sentinel*: how many interrupts in one job before it acts. Sent by the shell, so the
/// setting lives in one place and the sentinel needs no configuration of its own.
static ESCALATE_AFTER: AtomicU32 = AtomicU32::new(0);

/// The pipe the shell tells the sentinel through, and nothing else.
///
/// `None` until the first foreground job, so a shell that never runs one — a script, a `-c`, a
/// session spent entirely in builtins — never pays for the fork.
static CHANNEL: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// In the *sentinel*: the group it is watching, or 0 while it is watching nothing.
///
/// Read by the signal handler, so it is an atomic and nothing else.
static WATCHING: AtomicI32 = AtomicI32::new(0);

/// In the *sentinel*: interrupts seen since the current job started.
static SEEN: AtomicU32 = AtomicU32::new(0);

/// In the *sentinel*: the signal to send when the count is reached.
static ACTION: AtomicI32 = AtomicI32::new(libc::SIGSTOP);

/// In the *sentinel*: whether to say what the next press will do.
static ANNOUNCE: AtomicBool = AtomicBool::new(true);

/// In the *sentinel*: where to report what it did. `-1` until it is forked.
static EVENT_FD: AtomicI32 = AtomicI32::new(-1);

/// In the *shell*: the read end of the watcher's report pipe. See [`take_events`].
static EVENTS: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// What the shell asks the watcher to do about one job.
///
/// A struct rather than three arguments because it is what crosses the pipe, and every field of it
/// comes from the same setting — see [`oslo_ui::settings::InterruptEscape`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Orders {
    /// The group that owns the terminal, or `0` for "watch nothing".
    pub pgid: i32,
    /// How many interrupts before acting. `0` never acts.
    pub after: u32,
    /// The signal to send, as a number so the sentinel needs no vocabulary of its own.
    pub signal: i32,
    /// Whether the press before the last says what the next one will do.
    pub notify: bool,
}

impl Orders {
    /// The eight bytes that cross the pipe. Fixed width, so a short read is a broken pipe rather
    /// than a misparse.
    fn encode(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.pgid.to_ne_bytes());
        out[4..8].copy_from_slice(&self.after.to_ne_bytes());
        out[8..12].copy_from_slice(&self.signal.to_ne_bytes());
        out[12..16].copy_from_slice(&u32::from(self.notify).to_ne_bytes());
        out
    }

    fn decode(raw: [u8; 16]) -> Orders {
        let word = |at: usize| i32::from_ne_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
        Orders {
            pgid: word(0),
            after: word(4) as u32,
            signal: word(8),
            notify: word(12) != 0,
        }
    }
}

/// Start watching the group that now owns the terminal.
///
/// Called after the terminal is handed over, so the sentinel joins a group the tty is already
/// signalling. Silent about every failure — a shell that cannot fork a helper is a working shell
/// with the interrupt behaviour it had before.
pub(crate) fn watch(orders: Orders) {
    tell(orders);
}

/// Stop watching: the job is over and the shell has the terminal back.
pub(crate) fn stand_down() {
    tell(Orders {
        pgid: 0,
        after: 0,
        signal: 0,
        notify: false,
    });
}

/// Send orders to the sentinel, starting it if this is the first foreground job.
fn tell(orders: Orders) {
    let Ok(mut channel) = CHANNEL.lock() else {
        return;
    };
    if channel.is_none() {
        // Nothing to stand down from if it was never started, and starting one to immediately tell
        // it to do nothing would fork on the way *out* of the first job.
        if orders.pgid == 0 {
            return;
        }
        *channel = start();
    }
    if let Some(pipe) = channel.as_mut()
        && pipe.write_all(&orders.encode()).is_err()
    {
        // The sentinel is gone. Forget it rather than retrying every command; the shell is
        // otherwise unaffected.
        *channel = None;
    }
}

/// Fork the sentinel, answering the pipe to talk to it with.
///
/// Two pipes, because the traffic runs both ways: orders down, and *events* back — see
/// [`take_events`]. Without the second one the shell could see that a job stopped and never learn
/// whether that was Ctrl-Z or the watcher acting, which are different things to tell somebody.
fn start() -> Option<std::fs::File> {
    // **`O_CLOEXEC`, and here that is a security property rather than tidiness.** `nix`'s `pipe()`
    // is a bare `libc::pipe`, so both pairs were inherited by every program the shell ran: any
    // child could reprogram the watcher — its target process group, its signal, its delay — by
    // writing sixteen bytes to a descriptor it never asked for. The sentinel itself is a `fork`
    // with no `exec`, so it keeps both ends regardless.
    let (orders_read, orders_write) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).ok()?;
    let (events_read, events_write) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).ok()?;
    // SAFETY: the child calls only syscalls — no allocation, no locks, no Rust destructors that
    // could be waiting on a mutex another thread held at the moment of the fork.
    match oslo_base::forksafe::quiesced(|| unsafe { fork() }) {
        Ok(ForkResult::Child) => {
            drop(orders_write);
            drop(events_read);
            run(orders_read.into_raw_fd(), events_write.into_raw_fd());
        }
        Ok(ForkResult::Parent { .. }) => {
            drop(orders_read);
            drop(events_write);
            // Non-blocking, because the shell drains this *after* a job ends and must not wait on
            // a watcher that had nothing to say — which is the ordinary case.
            let back = events_read.into_raw_fd();
            unsafe {
                let flags = libc::fcntl(back, libc::F_GETFL);
                libc::fcntl(back, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
            if let Ok(mut slot) = EVENTS.lock() {
                *slot = Some(unsafe { std::fs::File::from_raw_fd(back) });
            }
            Some(unsafe { std::fs::File::from_raw_fd(orders_write.into_raw_fd()) })
        }
        Err(_) => None,
    }
}

/// What the watcher did, once. Read by the shell after a job ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Escalation {
    /// The group it acted on.
    pub pgid: i32,
    /// The signal it sent.
    pub signal: i32,
    /// How many interrupts it had counted by then.
    pub presses: u32,
}

/// Whether the watcher has actually been forked in this shell.
///
/// **Distinct from the setting.** A config can ask for escalation in a shell with no job control —
/// a script, an `oslo -c`, a session with no terminal — and there nothing is forked and Ctrl-C
/// means exactly what it always did. Reporting the setting alone would claim a feature that is not
/// running. See `oslo.job.watcher()`.
pub fn started() -> bool {
    CHANNEL.lock().map(|held| held.is_some()).unwrap_or(false)
}

/// The events the watcher has reported since this was last asked.
///
/// **Drained rather than polled.** The shell is inside `waitpid` for the whole time the watcher is
/// awake, so there is no moment to read this until the job ends — and by then the answer is either
/// there or it never will be. Non-blocking, so "nothing happened" costs one `read` returning
/// `EAGAIN`.
pub(crate) fn take_events() -> Vec<Escalation> {
    let Ok(mut slot) = EVENTS.lock() else {
        return Vec::new();
    };
    let Some(pipe) = slot.as_mut() else {
        return Vec::new();
    };
    let mut seen = Vec::new();
    let mut word = [0u8; 12];
    while let Ok(()) = pipe.read_exact(&mut word) {
        let field =
            |at: usize| i32::from_ne_bytes([word[at], word[at + 1], word[at + 2], word[at + 3]]);
        seen.push(Escalation {
            pgid: field(0),
            signal: field(4),
            presses: field(8) as u32,
        });
    }
    seen
}

/// The sentinel itself. Never returns.
fn run(orders: std::os::unix::io::RawFd, events: std::os::unix::io::RawFd) -> ! {
    EVENT_FD.store(events, Ordering::SeqCst);
    // Die with the shell. Without this a sentinel outlives a shell that was killed rather than
    // exited, and would sit in a process group forever.
    let _ = nix::sys::prctl::set_pdeathsig(Signal::SIGKILL);

    // Count interrupts rather than dying of them, and stay out of the way of everything else the
    // terminal can send: the sentinel is in the job's process group, so it receives the job's
    // Ctrl-Z and Ctrl-\ as well, and it must not stop or dump core in response.
    let counting = SigAction::new(
        SigHandler::Handler(on_interrupt),
        SaFlags::empty(),
        SigSet::empty(),
    );
    let ignored = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
    unsafe {
        let _ = signal::sigaction(Signal::SIGINT, &counting);
        for sig in [
            Signal::SIGTSTP,
            Signal::SIGTTIN,
            Signal::SIGTTOU,
            Signal::SIGQUIT,
            Signal::SIGHUP,
        ] {
            let _ = signal::sigaction(sig, &ignored);
        }
    }

    let mut orders = unsafe { std::fs::File::from_raw_fd(orders) };
    let mut word = [0u8; 16];
    loop {
        match orders.read_exact(&mut word) {
            Ok(()) => {}
            // The shell closed the pipe, which means the shell is gone.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            // A signal arrived mid-read. That is the ordinary case here.
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        let told = Orders::decode(word);
        // Set before joining, so an interrupt delivered during the `setpgid` is attributed to the
        // job rather than to whatever the sentinel was in before.
        SEEN.store(0, Ordering::SeqCst);
        ESCALATE_AFTER.store(told.after, Ordering::SeqCst);
        ACTION.store(told.signal, Ordering::SeqCst);
        ANNOUNCE.store(told.notify, Ordering::SeqCst);
        WATCHING.store(told.pgid, Ordering::SeqCst);
        if told.pgid != 0 {
            // `ESRCH` when the job finished before this ran, which is a race the shell wins and
            // the sentinel does not need to care about: the next order will be a 0.
            let _ = setpgid(Pid::from_raw(0), Pid::from_raw(told.pgid));
        }
    }
    std::process::exit(0);
}

/// The sentinel's `SIGINT` handler: count, and stop the group once there have been enough.
///
/// Touches three atomics and calls `setpgid` and `kill`, all of which POSIX lists as safe to call
/// from a handler. Nothing here allocates or takes a lock.
extern "C" fn on_interrupt(_: libc::c_int) {
    let pgid = WATCHING.load(Ordering::SeqCst);
    let after = ESCALATE_AFTER.load(Ordering::SeqCst);
    if pgid == 0 || after == 0 {
        return;
    }
    let seen = SEEN.fetch_add(1, Ordering::SeqCst) + 1;
    if seen < after {
        // **The press before the last says what the next one will do.** Two Ctrl-C into a job that
        // is ignoring them, a person is deciding whether anything is listening at all — and a
        // feature nobody knows fired is a feature nobody has. One line answers it.
        //
        // `write` straight to the terminal, because this runs in a signal handler: it is on the
        // list of calls that are safe there, and `eprintln!` allocates and takes a lock.
        if seen + 1 == after && ANNOUNCE.load(Ordering::SeqCst) {
            const AGAIN: &[u8] = b"\r\noslo: press ^C again to take the terminal back\r\n";
            unsafe {
                libc::write(2, AGAIN.as_ptr() as *const libc::c_void, AGAIN.len());
            }
        }
        return;
    }
    let signal = ACTION.load(Ordering::SeqCst);
    // **Reported before it is done.** `waitpid` wakes on the stop and drains this pipe without
    // blocking, so a report written afterwards is one the shell can already have missed — and the
    // next drain is a whole command away, since only an external command drains it. The job then
    // stopped with nothing said about it and `on-job-escalated` never fired. Six runs in six with
    // 50 ms of deschedule between the signal and a report written after it.
    let fd = EVENT_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        let mut event = [0u8; 12];
        event[0..4].copy_from_slice(&pgid.to_ne_bytes());
        event[4..8].copy_from_slice(&signal.to_ne_bytes());
        event[8..12].copy_from_slice(&(seen as i32).to_ne_bytes());
        unsafe {
            libc::write(fd, event.as_ptr() as *const libc::c_void, event.len());
        }
    }
    // **Leave the group before signalling it.** The sentinel is *in* the group it is about to
    // signal, and `kill(-pgid, …)` does not spare the sender — a `SIGSTOP` would stop the sentinel
    // along with the job, and it would never read another order.
    unsafe {
        libc::setpgid(0, 0);
        // **The whole group**, because a job is its process group — signalling the leader alone
        // would leave a pipeline's other stages and any grandchildren behind.
        //
        // `SIGSTOP` by default, and it is the one that destroys nothing: it cannot be caught or
        // ignored — the entire point, since the programs this exists for are the ones that caught
        // the interrupt — and `waitpid` already reports it, so the shell's own Ctrl-Z path takes
        // over. A config can ask for `kill`, `hup` or `quit` instead.
        libc::kill(-pgid, signal);
    }
    // And stop watching: the shell is about to send a fresh order for the next job, and until it
    // does there is nothing here worth acting on.
    WATCHING.store(0, Ordering::SeqCst);
}
