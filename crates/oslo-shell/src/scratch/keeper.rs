//! The process that holds a scratch open when nobody is attached.
//!
//! ```text
//!   whoever asked
//!        │ fork
//!        ▼
//!      keeper ── setsid, holds the flock, owns the pty master, serves the socket, writes the log
//!        │ fork
//!        ▼
//!      the shell ── setsid, TIOCSCTTY on the slave, stdin/stdout/stderr on it
//! ```
//!
//! # Why two forks and two sessions
//!
//! The keeper `setsid`s so that the terminal it was forked from is no longer its controlling
//! terminal: closing that terminal then sends it no `SIGHUP`, which is the entire promise of a scratch.
//! The shell `setsid`s again and claims the pty, so *it* is a session leader on a terminal of its
//! own — which is what makes `fg`, `bg`, `^C` and `^Z` inside a scratch ordinary rather than forwarded.
//!
//! # What the keeper does not do
//!
//! It never interprets a byte. Input arrives from a client and goes to the pty; output comes off
//! the pty and goes to the client and the log. Every decision about what a keystroke *means* is the
//! pty's line discipline and the shell's, exactly as if the shell were on a real terminal.

use super::{dir, log, store, wire};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::pty::openpty;
use nix::unistd::{ForkResult, close, dup2, fork, setsid};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};

/// How much is moved in one go. A pty's own buffer is smaller than this, so it is never the limit.
const CHUNK: usize = 8192;

/// Which process you are, once a scratch has been made.
///
/// **Returned rather than taken as a callback**, because the two callers want opposite things from
/// the process inside. `scratch delta` wants it to `exec` a fresh oslo; a session being wrapped at
/// startup wants it to *carry on* as the shell it already is, with the config it has already read.
/// A closure that had to satisfy both would have to be told which, which is the same decision one
/// level less obviously.
pub enum Role {
    /// You asked for the scratch. It exists and is listening; attach, or carry on.
    Caller(nix::unistd::Pid),
    /// You are the shell inside the scratch. The pty is your controlling terminal and your standard
    /// descriptors; everything from here is an ordinary interactive session.
    Inside,
}

/// Fork a keeper for `name`.
///
/// Returns twice, in two processes — see [`Role`]. Nothing waits: by the time the caller sees
/// `Caller`, the scratch exists and is listening.
pub fn spawn(name: &str, cap: u64) -> io::Result<Role> {
    if !super::name::valid(name) {
        return Err(io::Error::other(format!(
            "{name:?} is not a usable scratch name"
        )));
    }
    // Before the fork, so a directory we would refuse is reported to the caller rather than to a
    // child that has nowhere to say it.
    dir::open_checked()?;
    store::room_for_a_socket(name)?;
    if store::alive(name) {
        return Err(io::Error::other(format!("{name} is already running")));
    }

    // SAFETY: the child either execs (through `run`) or exits. It touches no allocator state that a
    // parent thread could be holding, which is the rule this has to obey and the reason the fork is
    // taken before any of oslo's warm threads start.
    match unsafe { fork() }.map_err(errno)? {
        ForkResult::Parent { child } => Ok(Role::Caller(child)),
        ForkResult::Child => match keep(name, cap) {
            // The grandchild: the caller of `spawn` continues here as the shell.
            Ok(Role::Inside) => Ok(Role::Inside),
            // The keeper has finished serving, which means the scratch is over. It must never return
            // to the caller's stack — from here it is a process that only looked like one.
            Ok(Role::Caller(_)) => std::process::exit(0),
            // **And the same is true of a keeper that never started.** Propagating with `?` sent
            // the error up through the *child's* copy of the caller's stack, so a keeper that
            // could not bind its socket left behind a second shell — stdio already on `/dev/null`,
            // reporting the failure to nobody. Only the parent's error is anybody's to see.
            Err(_) => std::process::exit(1),
        },
    }
}

/// The keeper itself, in the first child.
fn keep(name: &str, cap: u64) -> io::Result<Role> {
    // Out of the terminal's session: from here a closed terminal is not our problem.
    setsid().map_err(errno)?;

    // **And out of the terminal's descriptors.** A keeper that kept the ones it was forked with
    // would hold the far end of whatever pipe its parent was writing to, so anything waiting for
    // that pipe to close would wait for the scratch to end — which is forever, since a scratch is the thing
    // that outlives its parent. It cost a hung test suite to find, and it is the same hang a user
    // would get from `oslo -c 'scratch alpha' | cat`.
    detach_stdio();

    let held = match store::hold(name)? {
        Some(held) => held,
        // Somebody won the race between the check above and here.
        None => std::process::exit(0),
    };

    let pty = openpty(None, None).map_err(errno)?;
    let paths = store::Paths::new(name);

    // SAFETY: as above — the child execs or exits.
    match unsafe { fork() }.map_err(errno)? {
        ForkResult::Child => {
            drop(pty.master);
            become_the_shell(pty.slave);
            // **The shell must not run the lock's destructor.** `fork` gave it a duplicate
            // descriptor onto the keeper's open file description, and `Flock`'s `Drop` is an
            // explicit `LOCK_UN` — which `flock(2)` says releases the lock through *any* duplicate,
            // not just the one it was taken on. So the shell unwinding out of here would quietly
            // unlock the keeper's own lock, and every scratch would report itself dead the moment it
            // started. Forgetting it leaves the descriptor to be closed by the kernel, which does
            // not release anything while the keeper still holds its own.
            std::mem::forget(held);
            Ok(Role::Inside)
        }
        ForkResult::Parent { child } => {
            drop(pty.slave);
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let mut meta = store::Meta::now(&cwd);
            meta.pid = child.as_raw();
            let _ = std::fs::write(paths.meta(), meta.encode());

            // A socket file left by a keeper that died without tidying would make `bind` fail, and
            // the lock above has already proved nobody is behind it.
            let _ = std::fs::remove_file(paths.sock());
            let listener = UnixListener::bind(paths.sock())?;

            let ended = serve(&listener, pty.master, &paths, cap);
            // The shell has gone, so the scratch has. Tidying here is what keeps the common case from
            // relying on the next attach to sweep.
            drop(listener);
            store::sweep(ended.as_deref().unwrap_or(name));
            let result = ended.map(|_| Role::Caller(child));
            drop(held);
            result
        }
    }
}

/// Point the keeper's own standard descriptors at `/dev/null`.
///
/// The shell inside gets the pty a moment later and overwrites its own, so this is only ever about
/// the keeper — which has nothing to say to anybody and must not hold open what it inherited.
pub(super) fn detach_stdio() {
    let Ok(null) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
    else {
        return;
    };
    for target in 0..=2 {
        let _ = dup2(null.as_raw_fd(), target);
    }
}

/// Put the shell on the pty and give it away as a controlling terminal.
///
/// Returns so the caller can *be* the shell; it only prepares the ground.
fn become_the_shell(slave: OwnedFd) {
    let fd = slave.as_raw_fd();
    // Its own session, so that claiming the terminal below is allowed and so that job control
    // inside the scratch is the shell's own rather than a share of somebody else's.
    let _ = setsid();
    // SAFETY: `fd` is the slave we just opened, and TIOCSCTTY takes no pointer.
    unsafe {
        nix::libc::ioctl(fd, nix::libc::TIOCSCTTY, 0);
    }
    for target in 0..=2 {
        let _ = dup2(fd, target);
    }
    if fd > 2 {
        let _ = close(fd);
    }
    // The descriptor now lives on as 0, 1 and 2; closing it again on drop would take one of those
    // with it on the next process to open a file.
    std::mem::forget(slave);
}

/// Move bytes until the shell goes away.
///
/// One client at a time, which is the decision behind `scratch` refusing a second attach: two
/// terminals on one pty means one window size for both and two people typing into the same line.
/// The refusal itself is said by the client, against `store::attached` — see there for why it
/// cannot be said here.
fn serve(
    listener: &UnixListener,
    master: OwnedFd,
    paths: &store::Paths,
    cap: u64,
) -> io::Result<String> {
    let mut log = log::Log::open(&paths.log(), cap)?;
    let called = paths.name.clone();
    let mut client: Option<UnixStream> = None;
    let mut buffer = [0u8; CHUNK];
    // What has arrived from the client but is not yet a whole message. See `wire::take`.
    let mut pending: Vec<u8> = Vec::new();

    loop {
        // The poll borrows the client, and everything below may replace it — so the answers are
        // taken out of the borrow before anything acts on them, and the descriptors are dropped
        // with the block.
        let (from_pty, arriving, from_client) = {
            let mut fds = vec![
                PollFd::new(master.as_fd(), PollFlags::POLLIN),
                PollFd::new(listener.as_fd(), PollFlags::POLLIN),
            ];
            if let Some(stream) = &client {
                fds.push(PollFd::new(stream.as_fd(), PollFlags::POLLIN));
            }
            if poll(&mut fds, PollTimeout::NONE).map_err(errno)? == 0 {
                continue;
            }
            (
                ready(&fds[0]),
                ready(&fds[1]),
                fds.len() > 2 && ready(&fds[2]),
            )
        };

        // The pty first: output is what a scratch is for, and a client that has just gone should not
        // cost the shell a round of buffering.
        if from_pty {
            match read_fd(master.as_fd(), &mut buffer) {
                // EOF on the master means the shell has exited and the scratch is over.
                Ok(0) | Err(_) => return Ok(called),
                Ok(n) => {
                    let _ = log.append(&buffer[..n]);
                    if let Some(stream) = &mut client
                        && io::Write::write_all(stream, &buffer[..n]).is_err()
                    {
                        client = None;
                    }
                }
            }
        }

        if arriving && let Ok((stream, _)) = listener.accept() {
            // Refusing a second attach is the client's decision, made against `store::attached`
            // before it ever connects; a keeper that already has one drops the new arrival and is
            // the backstop rather than the answer, because it has no way to say why.
            if client.is_none() {
                client = Some(stream);
            }
        }

        if from_client {
            let Some(stream) = &mut client else { continue };
            match io::Read::read(stream, &mut buffer) {
                Ok(0) | Err(_) => {
                    client = None;
                    pending.clear();
                }
                Ok(n) => {
                    pending.extend_from_slice(&buffer[..n]);
                    for message in wire::take(&mut pending) {
                        match message {
                            wire::Message::Data(bytes) => {
                                if write_fd(master.as_fd(), &bytes).is_err() {
                                    return Ok(called);
                                }
                            }
                            // The client is the only thing that knows how big the window is, and
                            // the pty is the only thing that can tell the programs inside.
                            wire::Message::Resize { rows, cols } => {
                                resize(master.as_fd(), rows, cols)
                            }
                        }
                    }
                    // A stream that is not this protocol will never become one, so waiting for more
                    // of it would be waiting forever with a growing buffer.
                    if wire::corrupt(&pending) {
                        client = None;
                        pending.clear();
                    }
                }
            }
        }
    }
}

/// Tell the pty how big the window is, so `SIGWINCH` reaches everything inside the scratch.
fn resize(master: BorrowedFd<'_>, rows: u16, cols: u16) {
    let size = nix::libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `size` outlives the call, and TIOCSWINSZ reads exactly one `winsize` from it. The
    // kernel signals the pty's foreground group; nothing here has to know who that is.
    unsafe {
        nix::libc::ioctl(master.as_raw_fd(), nix::libc::TIOCSWINSZ, &size);
    }
}

fn ready(fd: &PollFd<'_>) -> bool {
    fd.revents()
        .is_some_and(|events| events.intersects(PollFlags::POLLIN | PollFlags::POLLHUP))
}

fn read_fd(fd: BorrowedFd<'_>, into: &mut [u8]) -> io::Result<usize> {
    nix::unistd::read(fd.as_raw_fd(), into).map_err(errno)
}

fn write_fd(fd: BorrowedFd<'_>, bytes: &[u8]) -> io::Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        match nix::unistd::write(fd, &bytes[written..]) {
            Ok(0) => return Err(io::Error::other("wrote nothing")),
            Ok(n) => written += n,
            Err(nix::errno::Errno::EINTR) => {}
            Err(err) => return Err(errno(err)),
        }
    }
    Ok(())
}

fn errno(err: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(err as i32)
}

#[cfg(test)]
mod tests;
