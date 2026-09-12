//! Reading and writing: `echo`, `printf` and `read`.
//!
//! `read` is three separable problems and is split along those seams rather than by size:
//! [`read_input`] owns the bytes (which descriptor, which delimiter, how many characters, how
//! long to wait), [`read_split`] owns the `IFS` field semantics that turn one line into named
//! variables, and [`read`] owns the option grammar that connects them.

mod echo;
mod printf;
#[allow(clippy::module_inception)]
mod read;
mod read_input;
mod read_split;

pub use echo::builtin_echo;
pub use printf::{builtin_printf, shell_quote};
pub use read::builtin_read;

use crate::env::origin_now;
use std::io::Write;
use std::os::fd::BorrowedFd;

/// Write `bytes` to standard output, reporting a failure the way bash does.
///
/// The result is *not* discarded, and that is the point. A builtin that cannot write has failed,
/// and a script needs to be told: `printf x > /dev/full` exits 1 in bash. modernish calls the
/// missing check `BUG_PUTIOERR` and warns it "would cause this module to leave background
/// processes hanging in infinite loops" — which is exactly what happened, because the process
/// feeding its `LOOP` never learned that its reader had closed.
///
/// Written straight to descriptor 1 rather than through `std::io::stdout()`. That handle is
/// globally buffered, and on a failed write the bytes *stay* in the buffer: `printf x > /dev/full`
/// then printed `x` on the terminal when the runtime flushed at exit, by which time the shell had
/// restored the descriptor. Rust's buffer is flushed first so that anything already queued by a
/// `println!` elsewhere cannot be overtaken.
///
/// Returns the status the calling builtin should return.
pub(crate) fn write_stdout(name: &str, bytes: &[u8]) -> i32 {
    let _ = std::io::stdout().flush();
    // A filename that is not UTF-8 is written with the bytes it has; see `oslo_base::lossless`.
    let bytes = oslo_base::lossless::restore_bytes(bytes);

    // Safety: descriptor 1 is open for the lifetime of the shell, and this borrows it without
    // taking ownership, so nothing here can close it.
    let fd = unsafe { BorrowedFd::borrow_raw(1) };
    let mut written = 0;
    while written < bytes.len() {
        match nix::unistd::write(fd, &bytes[written..]) {
            // A zero-length write is not an error but makes no progress; treating it as success
            // would spin here for ever.
            Ok(0) => return 1,
            Ok(n) => written += n,
            Err(nix::errno::Errno::EINTR) => continue,
            // A broken pipe needs no diagnostic: with SIGPIPE at its default the shell is being
            // killed anyway, and bash prints nothing for it either.
            Err(nix::errno::Errno::EPIPE) => return 1,
            Err(e) => {
                eprintln!("{}{name}: write error: {}", origin_now(), e.desc());
                return 1;
            }
        }
    }
    0
}
