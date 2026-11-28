//! Files only this user may read: made private, written whole, read without following a planted
//! link.
//!
//! # Writing
//!
//! Every write goes to a scratch file beside its target, named `.NAME.<random>.new` and opened with
//! `O_CREAT|O_EXCL|O_NOFOLLOW` at mode `0600`. The bytes are written and `fsync`ed, the scratch file
//! is renamed over the target (or hard-linked to it, when nothing may be replaced), and the
//! directory is `fsync`ed after. A reader sees the old file or the new one; a symbolic link planted
//! at any name a write uses is refused or replaced, never written through; and a machine that stops
//! leaves either file whole.
//!
//! # Reading
//!
//! A read opens with `O_NONBLOCK`, so a FIFO cannot hang it, and accepts only a regular file, checked
//! before the open and again on the open descriptor. It reads at most a stated number of bytes.
//! Under [`Links::Refuse`] a symbolic link at the last component is refused too.
//!
//! # Locking
//!
//! [`lock`] takes `flock` on a `.lock` file in a directory, for a read-modify-write that two
//! processes must not interleave.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use nix::libc;

/// The mode every file written here is created with.
pub const FILE_MODE: u32 = 0o600;

/// The mode every directory made here is created with.
pub const DIRECTORY_MODE: u32 = 0o700;

/// The most a key or a configuration file may hold.
pub const SMALL: u64 = 64 * 1024;

/// Whether a read follows a symbolic link at the last component of its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Links {
    Follow,
    Refuse,
}

/// `path` and any missing parents, each made at `0700`. An existing directory is left as it is.
pub fn directory(path: &Path) -> Result<(), String> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(DIRECTORY_MODE)
        .create(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The same, and `path` itself tightened to `0700` when it is a real directory that is looser.
///
/// A symbolic link to a directory is somebody's choice, so it is followed and left alone.
pub fn own_directory(path: &Path) -> Result<(), String> {
    directory(path)?;
    let seen = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if seen.is_dir() && seen.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(DIRECTORY_MODE))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Whether anybody but its owner may list or enter the directory at `path`.
pub fn loose(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|seen| seen.permissions().mode() & 0o077 != 0)
}

/// Put `bytes` at `path`, replacing whatever was there, through a scratch file and a rename.
pub fn replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut scratch = Scratch::write(path, bytes)?;
    std::fs::rename(&scratch.path, path).map_err(|e| format!("{}: {e}", path.display()))?;
    scratch.landed = true;
    sync_directory(path)
}

/// Put `bytes` at `path` only if nothing is there. `Ok(false)` when something already was.
///
/// A hard link from the scratch file, which fails rather than replaces. A filesystem without hard
/// links (vfat on a USB stick) gets an `O_EXCL` create of the file itself instead.
pub fn create(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    let scratch = Scratch::write(path, bytes)?;
    match std::fs::hard_link(&scratch.path, path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(_) => {
            drop(scratch);
            return exclusive(path, bytes);
        }
    }
    drop(scratch);
    sync_directory(path)?;
    Ok(true)
}

/// The fallback for [`create`]: the target itself, opened `O_EXCL`.
fn exclusive(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    let mut file = match open_new(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(path);
        return Err(format!("{}: {e}", path.display()));
    }
    sync_directory(path)?;
    Ok(true)
}

/// A scratch file beside a target, removed again unless it landed.
struct Scratch {
    path: PathBuf,
    landed: bool,
}

impl Scratch {
    fn write(beside: &Path, bytes: &[u8]) -> Result<Scratch, String> {
        let name = beside
            .file_name()
            .ok_or_else(|| format!("{}: not a file name", beside.display()))?;
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).map_err(|e| format!("no randomness from the system: {e}"))?;
        let path = parent_of(beside).join(format!(
            ".{}.{:016x}.new",
            name.to_string_lossy(),
            u64::from_ne_bytes(random)
        ));
        let mut file = open_new(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let scratch = Scratch {
            path,
            landed: false,
        };
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("{}: {e}", scratch.path.display()))?;
        Ok(scratch)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.landed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// A new file at mode `0600` that refuses to exist already or to be a link.
fn open_new(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

fn parent_of(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// `fsync` the directory `path` is in, so a rename into it survives the machine stopping.
fn sync_directory(path: &Path) -> Result<(), String> {
    let directory = parent_of(path);
    match File::open(directory).and_then(|handle| handle.sync_all()) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => Ok(()),
        Err(e) => Err(format!("{}: {e}", directory.display())),
    }
}

/// What the regular file at `path` holds, at most `cap` bytes. `Ok(None)` when nothing is there.
pub fn read(path: &Path, cap: u64, links: Links) -> Result<Option<Vec<u8>>, String> {
    let looked = match links {
        Links::Follow => std::fs::metadata(path),
        Links::Refuse => std::fs::symlink_metadata(path),
    };
    match looked {
        Ok(seen) => regular(path, &seen)?,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    let mut flags = libc::O_NONBLOCK;
    if links == Links::Refuse {
        flags |= libc::O_NOFOLLOW;
    }
    let file = match OpenOptions::new().read(true).custom_flags(flags).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(format!("{}: {}", path.display(), LINK));
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let seen = file
        .metadata()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    regular(path, &seen)?;
    if seen.len() > cap {
        return Err(too_large(path, cap));
    }
    let mut bytes = Vec::with_capacity(seen.len() as usize);
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > cap {
        return Err(too_large(path, cap));
    }
    Ok(Some(bytes))
}

const LINK: &str = "a symbolic link, which is not followed here";

/// Refuse anything that is not a regular file, saying what it is instead.
fn regular(path: &Path, seen: &std::fs::Metadata) -> Result<(), String> {
    let kind = seen.file_type();
    let what = if kind.is_file() {
        return Ok(());
    } else if kind.is_symlink() {
        LINK
    } else if kind.is_dir() {
        "a directory, not a file"
    } else if kind.is_fifo() {
        "a FIFO, not a regular file"
    } else if kind.is_char_device() || kind.is_block_device() {
        "a device, not a regular file"
    } else {
        "not a regular file"
    };
    Err(format!("{}: {what}", path.display()))
}

fn too_large(path: &Path, cap: u64) -> String {
    format!(
        "{}: larger than {cap} bytes, so it was not read",
        path.display()
    )
}

/// An `flock` on a `.lock` file in a directory, released when this is dropped.
#[must_use = "the lock is released as soon as this is dropped"]
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

/// Take `directory`'s lock: exclusive for a write, shared for a read that must see one moment.
///
/// The directory must exist. The `.lock` file is made at `0600` and is never a link.
pub fn lock(directory: &Path, shared: bool) -> Result<Lock, String> {
    let path = directory.join(".lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ELOOP) => format!("{}: {LINK}", path.display()),
            _ => format!("{}: {e}", path.display()),
        })?;
    let seen = file
        .metadata()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    regular(&path, &seen)?;
    match shared {
        true => file.lock_shared(),
        false => file.lock(),
    }
    .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Lock { _file: file })
}

#[cfg(test)]
#[path = "private/tests.rs"]
mod tests;
