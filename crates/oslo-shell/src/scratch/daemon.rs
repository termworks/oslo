//! One process in front of every scratch, for `oslo.scratch.daemon = true`.
//!
//! ```text
//!   client ──► daemon.sock   0x10                    which scratches are there?
//!                            0x11 len(u16be) name…   put me in this one
//!                            0x12 len(u16be) name…   end this one
//!
//!   after an attach the connection is a pipe: whatever the scratch says, and whatever is typed at it,
//!   spliced byte for byte. The daemon reads none of it.
//! ```
//!
//! # What it is for, given the other backend works
//!
//! One socket instead of one per scratch, and one process that knows the whole list without reading a
//! directory. That is the whole difference, and it is why this is a registry and a splice rather
//! than a second implementation of everything: the keeper below is the same keeper.
//!
//! # Started on demand, never installed
//!
//! There is no service to enable. The first client that needs a daemon forks one, exactly as a
//! client forks a keeper without one — so a machine with no scratches has no oslo process running, which
//! is the property that makes this safe to have on by default for anybody who wants it.

use super::{dir, keeper, store};
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};

/// Ask which scratches there are.
const LIST: u8 = 0x10;
/// Put me in this scratch, making it if it is not there.
const ATTACH: u8 = 0x11;
/// End this scratch.
const KILL: u8 = 0x12;

/// What a client asked the daemon for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    List,
    Attach(String),
    Kill(String),
}

/// Frame a request.
pub fn frame(request: &Request) -> Vec<u8> {
    match request {
        Request::List => vec![LIST],
        Request::Attach(name) => named(ATTACH, name),
        Request::Kill(name) => named(KILL, name),
    }
}

/// A request that carries a scratch's name.
fn named(kind: u8, name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() + 3);
    out.push(kind);
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

/// One request from the front of `bytes`, and how much of it was used.
///
/// `None` means not yet — or never, for a first byte that names no request, which the caller tells
/// apart with [`unparseable`].
pub fn parse(bytes: &[u8]) -> Option<(Request, usize)> {
    let kind = *bytes.first()?;
    if kind == LIST {
        return Some((Request::List, 1));
    }
    if kind != ATTACH && kind != KILL {
        return None;
    }
    let len = u16::from_be_bytes([*bytes.get(1)?, *bytes.get(2)?]) as usize;
    let end = 3 + len;
    if bytes.len() < end {
        return None;
    }
    let name = String::from_utf8(bytes[3..end].to_vec()).ok()?;
    match kind {
        ATTACH => Some((Request::Attach(name), end)),
        _ => Some((Request::Kill(name), end)),
    }
}

/// Whether the front of `bytes` can never become a request, however much more arrives.
pub fn unparseable(bytes: &[u8]) -> bool {
    bytes
        .first()
        .is_some_and(|kind| ![LIST, ATTACH, KILL].contains(kind))
}

/// Where the daemon listens.
pub fn socket() -> std::path::PathBuf {
    dir::path().join("daemon.sock")
}

/// The names the daemon knows, asking it to start if it is not there.
pub fn ask_list() -> io::Result<Vec<String>> {
    let mut socket = reach()?;
    socket.write_all(&frame(&Request::List))?;
    socket.flush()?;
    let mut said = String::new();
    socket.read_to_string(&mut said)?;
    Ok(said.lines().map(str::to_string).collect())
}

/// Ask the daemon to end `name`, and say what it made of it.
///
/// The answer is a line rather than a status byte, for the same reason `list` answers in lines:
/// there is one reader of it and it is a person, through `scratch -k`.
pub fn ask_kill(name: &str) -> io::Result<()> {
    let mut socket = reach()?;
    socket.write_all(&frame(&Request::Kill(name.to_string())))?;
    socket.flush()?;
    let mut said = String::new();
    socket.read_to_string(&mut said)?;
    match said.trim() {
        "" => Ok(()),
        complaint => Err(io::Error::other(complaint.to_string())),
    }
}

/// A connection that is already inside `name`.
pub fn attach_through(name: &str) -> io::Result<UnixStream> {
    let mut socket = reach()?;
    socket.write_all(&frame(&Request::Attach(name.to_string())))?;
    socket.flush()?;
    Ok(socket)
}

/// Connect to the daemon, starting one if nothing answers.
fn reach() -> io::Result<UnixStream> {
    if let Ok(socket) = UnixStream::connect(socket()) {
        return Ok(socket);
    }
    // A socket file with nobody behind it is what a daemon that died without tidying leaves. It
    // cannot be connected to and would stop the new one binding.
    let _ = std::fs::remove_file(socket());
    start()?;
    super::client::connect(&socket(), "daemon")
}

/// Fork a daemon, which never returns to the caller.
fn start() -> io::Result<()> {
    dir::open_checked()?;
    // SAFETY: the child only serves and exits; it touches no allocator state a parent thread could
    // be holding. The same rule the keeper's fork obeys, for the same reason.
    match oslo_base::forksafe::quiesced(|| unsafe { nix::unistd::fork() }).map_err(errno)? {
        nix::unistd::ForkResult::Parent { .. } => Ok(()),
        nix::unistd::ForkResult::Child => {
            let _ = nix::unistd::setsid();
            keeper::detach_stdio();
            let _ = serve();
            std::process::exit(0)
        }
    }
}

/// Answer clients until nobody has wanted anything for long enough to be worth stopping.
///
/// **It stops on its own.** A daemon nothing has asked for in ten minutes is a process holding a
/// socket for nobody, and one that had to be killed by hand would be worse than no daemon at all.
/// Anything that wants it again starts another; that is what [`reach`] is for.
fn serve() -> io::Result<()> {
    let listener = UnixListener::bind(socket())?;
    listener.set_nonblocking(true)?;
    let mut idle = std::time::Instant::now();

    loop {
        match listener.accept() {
            Ok((client, _)) => {
                idle = std::time::Instant::now();
                let _ = client.set_nonblocking(false);
                let _ = answer(client);
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                if idle.elapsed() > IDLE {
                    let _ = std::fs::remove_file(socket());
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return Ok(()),
        }
    }
}

/// How long a daemon waits with nothing to do before giving up its socket.
const IDLE: std::time::Duration = std::time::Duration::from_secs(600);

/// Read one request and do what it says.
fn answer(mut client: UnixStream) -> io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 64];
    let request = loop {
        if let Some((request, _)) = parse(&buffer) {
            break request;
        }
        if unparseable(&buffer) {
            return Ok(());
        }
        match client.read(&mut chunk) {
            Ok(0) | Err(_) => return Ok(()),
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    };

    match request {
        Request::List => {
            let names = store::list()?;
            for (name, _) in names {
                writeln!(client, "{name}")?;
            }
            client.flush()
        }
        Request::Kill(name) => {
            if !super::name::valid(&name) {
                return Ok(());
            }
            if let Err(err) = store::kill(&name) {
                writeln!(client, "{err}")?;
            }
            client.flush()
        }
        // The daemon makes the scratch, so the client never forks a shell in this mode — and then gets
        // out of the way, because a registry that read the stream would be a terminal that lies.
        Request::Attach(name) => {
            if !super::name::valid(&name) {
                return Ok(());
            }
            if !store::alive(&name)
                && let keeper::Role::Inside =
                    keeper::spawn(&name, oslo_ui::settings::current().scratch.log_bytes)?
            {
                super::enter::exec_inside(&name);
            }
            let scratch = super::client::connect(&store::Paths::new(&name).sock(), &name)?;
            splice(client, scratch)
        }
    }
}

/// Copy both ways until either side stops.
///
/// Nothing here looks at what it is copying. The framing between a client and a keeper is settled
/// in `wire`, and a middleman that understood it would be a second place to keep in step with it.
fn splice(mut client: UnixStream, mut scratch: UnixStream) -> io::Result<()> {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::fd::AsFd;

    let mut buffer = [0u8; 8192];
    loop {
        let (from_client, from_tab) = {
            let mut fds = [
                PollFd::new(client.as_fd(), PollFlags::POLLIN),
                PollFd::new(scratch.as_fd(), PollFlags::POLLIN),
            ];
            if poll(&mut fds, PollTimeout::NONE).map_err(errno)? == 0 {
                continue;
            }
            let ready = |fd: &PollFd<'_>| {
                fd.revents()
                    .is_some_and(|e| e.intersects(PollFlags::POLLIN | PollFlags::POLLHUP))
            };
            (ready(&fds[0]), ready(&fds[1]))
        };

        if from_client && !copy(&mut client, &mut scratch, &mut buffer) {
            return Ok(());
        }
        if from_tab && !copy(&mut scratch, &mut client, &mut buffer) {
            return Ok(());
        }
    }
}

/// One read moved across, or `false` when the far end has gone.
fn copy(from: &mut UnixStream, to: &mut UnixStream, buffer: &mut [u8]) -> bool {
    match from.read(buffer) {
        Ok(0) | Err(_) => false,
        Ok(n) => to.write_all(&buffer[..n]).is_ok(),
    }
}

fn errno(err: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(err as i32)
}

#[cfg(test)]
mod tests;
