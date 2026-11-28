//! `oslo secret run --file VAR=NAME`: a value handed over as a file that exists only in memory.
//!
//! `memfd_create` with `MFD_CLOEXEC | MFD_ALLOW_SEALING`, the value written and the offset put
//! back at 0, then `F_SEAL_WRITE | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL`, and `FD_CLOEXEC`
//! cleared on this one descriptor so the child inherits it. The child reads `/dev/fd/N`; the bytes
//! are never on a filesystem, and any value works, binary or not.

use std::io::{Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, OwnedFd};

/// A sealed, read-only in-memory file holding `value`, inheritable across `exec`.
pub fn sealed(value: &[u8]) -> Result<OwnedFd, String> {
    use nix::fcntl::{FcntlArg, FdFlag, SealFlag, fcntl};
    use nix::sys::memfd::{MemFdCreateFlag, memfd_create};

    let flags = MemFdCreateFlag::MFD_CLOEXEC | MemFdCreateFlag::MFD_ALLOW_SEALING;
    let fd = memfd_create(c"oslo-secret", flags).map_err(|e| format!("memfd_create: {e}"))?;
    let mut file = std::fs::File::from(fd);
    file.write_all(value)
        .and_then(|()| file.seek(SeekFrom::Start(0)).map(drop))
        .map_err(|e| format!("cannot fill the in-memory file: {e}"))?;
    let fd = OwnedFd::from(file);
    let seals = SealFlag::F_SEAL_WRITE
        | SealFlag::F_SEAL_SHRINK
        | SealFlag::F_SEAL_GROW
        | SealFlag::F_SEAL_SEAL;
    fcntl(fd.as_raw_fd(), FcntlArg::F_ADD_SEALS(seals))
        .map_err(|e| format!("cannot seal it: {e}"))?;
    fcntl(fd.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::empty()))
        .map_err(|e| format!("cannot hand it to the child: {e}"))?;
    Ok(fd)
}

/// The path the child opens.
pub fn path(fd: &OwnedFd) -> String {
    format!("/dev/fd/{}", fd.as_raw_fd())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::fcntl::{FcntlArg, fcntl};

    #[test]
    fn the_file_holds_the_bytes_and_refuses_a_write() {
        let value = b"binary\0\xff\xfe value";
        let fd = sealed(value).expect("sealed");
        let read = std::fs::read(path(&fd)).expect("read back");
        assert_eq!(read, value);

        let link = std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd())).expect("link");
        assert!(
            link.to_string_lossy().starts_with("/memfd:oslo-secret"),
            "on no filesystem: {}",
            link.display()
        );
        assert!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(path(&fd))
                .and_then(|mut f| f.write_all(b"more"))
                .is_err(),
            "sealed against writes"
        );
        let flags = fcntl(fd.as_raw_fd(), FcntlArg::F_GETFD).expect("flags");
        assert_eq!(flags & nix::libc::FD_CLOEXEC, 0, "inherited by the child");
    }
}
