//! The anonymous file a here-document or here-string body is read from.

use oslo_base::error::{Result, ShellError, reason};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};

/// Materialise a heredoc or here-string body as a readable fd positioned at byte 0.
///
/// A body that fits in a pipe's buffer goes into a pipe, as bash does, so that a command opening
/// `/dev/stdin` again reads on from where its stdin is rather than from the start. A larger one
/// would block the shell writing it, since nothing reads the far end yet, so it goes into an
/// anonymous file instead, which has no capacity limit.
pub(super) fn heredoc_body(content: &str) -> Result<File> {
    if let Some(pipe) = piped(content.as_bytes()) {
        return Ok(pipe);
    }
    let mut file = anonymous_file()?;
    file.write_all(content.as_bytes())
        .map_err(|e| ShellError::ExecutionError(format!("heredoc: {}", reason(&e))))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| ShellError::ExecutionError(format!("heredoc: {}", reason(&e))))?;
    Ok(file)
}

/// The read end of a pipe already holding `bytes`, or `None` when they would not fit in it.
fn piped(bytes: &[u8]) -> Option<File> {
    use nix::fcntl::{FcntlArg, OFlag, fcntl};
    use std::os::fd::AsRawFd;
    let (read, write) = nix::unistd::pipe2(OFlag::O_CLOEXEC).ok()?;
    let capacity = fcntl(write.as_raw_fd(), FcntlArg::F_GETPIPE_SZ).ok()?;
    if bytes.len() >= usize::try_from(capacity).ok()? {
        return None;
    }
    let mut writer = File::from(write);
    writer.write_all(bytes).ok()?;
    drop(writer);
    Some(File::from(read))
}

/// An empty read/write file with no name in the filesystem, so nothing has to clean it up.
fn anonymous_file() -> Result<File> {
    use nix::sys::memfd::{MemFdCreateFlag, memfd_create};
    // ENOSYS below Linux 3.17, and blocked by some seccomp policies; fall back rather than making
    // heredocs a kernel-version feature.
    if let Ok(fd) = memfd_create(c"oslo-heredoc", MemFdCreateFlag::MFD_CLOEXEC) {
        return Ok(File::from(fd));
    }
    unlinked_temp_file()
}

/// A temp file unlinked the instant it exists: the fd stays valid, the name does not.
pub(super) fn unlinked_temp_file() -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    let dir = std::env::temp_dir();
    // A name collision is another process (or an earlier heredoc in this one) owning the path;
    // retry rather than clobbering it, since `create_new` guarantees we never open someone else's.
    for _ in 0..64 {
        let path = dir.join(format!(
            "oslo-heredoc-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(nix::fcntl::OFlag::O_CLOEXEC.bits())
            .open(&path)
        {
            Ok(file) => {
                let _ = std::fs::remove_file(&path);
                return Ok(file);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(ShellError::ExecutionError(format!(
                    "heredoc: {}: {}",
                    dir.display(),
                    e
                )));
            }
        }
    }
    Err(ShellError::ExecutionError(format!(
        "heredoc: {}: could not create a temporary file",
        dir.display()
    )))
}
