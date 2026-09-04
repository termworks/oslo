//! Debounce and coalesce or restart execution.

use super::process::{ChildGroup, status_code};
use super::set::WatchSet;
use super::{Policy, WatchSpec};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use std::io;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

static STOP_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn stopped(signal: i32) {
    STOP_SIGNAL.store(signal, Ordering::Relaxed);
}

pub fn run(spec: &WatchSpec) -> io::Result<i32> {
    spec.validate()?;
    let _signals = Signals::install()?;
    let mut watched = WatchSet::open(&spec.root, &spec.patterns)?;
    let mut child: Option<ChildGroup> = None;
    let mut dirty = spec.initial;
    let mut deadline = spec.initial.then_some(Instant::now());

    eprintln!("watch {}: ready", spec.name);
    loop {
        if let Some(signal) = take_signal() {
            if let Some(mut running) = child.take() {
                let _ = running.terminate(spec.grace, || {
                    let _ = watched.read();
                });
            }
            return Ok(128 + signal);
        }

        if let Some(running) = &mut child
            && let Some(status) = running.try_wait()?
        {
            eprintln!("watch {}: exit {}", spec.name, status_code(status));
            child = None;
            if dirty && deadline.is_none() {
                deadline = Some(Instant::now());
            }
        }

        if deadline.is_some_and(|at| Instant::now() >= at) {
            deadline = None;
            match (&mut child, spec.policy) {
                (Some(running), Policy::Restart) => {
                    let _ = running.terminate(spec.grace, || {
                        if watched.read().unwrap_or(false) {
                            dirty = true;
                        }
                    });
                    child = None;
                }
                (Some(_), Policy::Coalesce) => {
                    dirty = true;
                }
                (None, _) => {}
            }
            if child.is_none() && dirty {
                dirty = false;
                let running = ChildGroup::spawn(&spec.argv, &spec.root)?;
                eprintln!("watch {}: start pid {}", spec.name, running.id());
                child = Some(running);
            }
        }

        let timeout = timeout(deadline, child.is_some());
        let ready = {
            let mut descriptors = [PollFd::new(watched.as_fd(), PollFlags::POLLIN)];
            match poll(&mut descriptors, timeout) {
                Ok(count) => {
                    count > 0
                        && descriptors[0]
                            .revents()
                            .is_some_and(|flags| flags.contains(PollFlags::POLLIN))
                }
                Err(nix::errno::Errno::EINTR) => continue,
                Err(error) => return Err(errno(error)),
            }
        };
        if ready && watched.read()? {
            dirty = true;
            deadline = Some(Instant::now() + spec.debounce);
        }
    }
}

fn timeout(deadline: Option<Instant>, child: bool) -> PollTimeout {
    let child_tick = child.then_some(Duration::from_millis(50));
    let wait = deadline
        .map(|at| at.saturating_duration_since(Instant::now()))
        .into_iter()
        .chain(child_tick)
        .min();
    match wait {
        Some(duration) => PollTimeout::try_from(duration).unwrap_or(PollTimeout::MAX),
        None => PollTimeout::NONE,
    }
}

fn take_signal() -> Option<i32> {
    let signal = STOP_SIGNAL.swap(0, Ordering::Relaxed);
    (signal != 0).then_some(signal)
}

struct Signals {
    old: Vec<(Signal, SigAction)>,
}

impl Signals {
    fn install() -> io::Result<Self> {
        STOP_SIGNAL.store(0, Ordering::Relaxed);
        let action = SigAction::new(
            SigHandler::Handler(stopped),
            SaFlags::SA_RESTART,
            SigSet::empty(),
        );
        let mut old = Vec::new();
        for signal in [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP] {
            // SAFETY: the handler only stores an integer in a lock-free atomic.
            let previous = unsafe { sigaction(signal, &action) }.map_err(errno)?;
            old.push((signal, previous));
        }
        Ok(Self { old })
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        for (signal, action) in &self.old {
            // SAFETY: each action was returned by sigaction for the same signal.
            let _ = unsafe { sigaction(*signal, action) };
        }
    }
}

fn errno(error: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_without_a_child_waits_forever() {
        assert_eq!(timeout(None, false), PollTimeout::NONE);
    }

    #[test]
    fn a_running_child_is_reaped_promptly() {
        assert_eq!(timeout(None, true), PollTimeout::from(50u16));
    }
}
