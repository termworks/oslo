//! A directory of sealed files, as one stream of bytes.
//!
//! # Why not tar
//!
//! Because this carries exactly one thing: a flat directory of files whose names oslo already
//! validates and whose contents it never looks inside. Tar brings permissions, ownership, symlinks,
//! device nodes and path traversal — every one of them a way for a hostile far end to write
//! somewhere it should not, in exchange for nothing this needs.
//!
//! ```text
//! OSLOBUNDLE1
//! per file:  name length (u32) │ name │ body length (u64) │ body
//! ```
//!
//! **Names are checked on the way in, not trusted.** A name with a `/` or a `..` in it is refused
//! rather than sanitised: the far end is another oslo and has no business sending one, so the
//! honest answer to a name that could escape the directory is to stop. So is a name with a control
//! or bidirectional-override character in it, which no secret may have.
//!
//! **Only regular files go out.** Each entry is looked at without following links; a link, a FIFO,
//! a device or a directory is passed over without being opened, and so are dot-files — the store's
//! `.lock` and the scratch files of a write in progress.

use oslo::private::{self, Links};
use std::path::Path;

const MAGIC: &[u8] = b"OSLOBUNDLE1\n";

/// Every file in `directory`, as one stream. A directory that is not there packs as empty.
pub fn pack(directory: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = MAGIC.to_vec();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(bytes);
    };
    let mut files: Vec<(String, std::path::PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .collect();
    // Sorted so that the same directory always packs to the same bytes, which makes a difference
    // something a person can diff rather than something that moves on its own.
    files.sort();

    for (name, path) in files {
        // The scratch files a write leaves behind belong to whoever is writing, not in a bundle.
        if name.starts_with('.') || name.ends_with(".new") || !safe(&name) {
            continue;
        }
        let Some(body) = private::read(&path, oslo::secrets::LARGEST, Links::Refuse)? else {
            continue;
        };
        let length = u32::try_from(name.len()).map_err(|_| format!("{name}: name too long"))?;
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&(body.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&body);
    }
    Ok(bytes)
}

/// The other way, into a directory of its own, made `0700`. Each file is created `0600` and never
/// over another, so a name sent twice is refused.
pub fn unpack(directory: &Path, bytes: &[u8]) -> Result<(), String> {
    private::directory(directory)?;
    if bytes.is_empty() {
        return Ok(());
    }
    let mut rest = bytes
        .strip_prefix(MAGIC)
        .ok_or("what arrived is not a bundle of secrets")?;

    while !rest.is_empty() {
        let (length, tail) = take(rest, 4)?;
        let length = u32::from_le_bytes(length.try_into().map_err(|_| bad())?) as usize;
        let (name, tail) = take(tail, length)?;
        let name = std::str::from_utf8(name).map_err(|_| "a name in the bundle is not text")?;
        let (size, tail) = take(tail, 8)?;
        let size = u64::from_le_bytes(size.try_into().map_err(|_| bad())?) as usize;
        let (body, tail) = take(tail, size)?;
        rest = tail;

        if !safe(name) {
            return Err(format!("{name:?}: not a name a bundle may carry"));
        }
        let path = directory.join(name);
        if !private::create(&path, body)? {
            return Err(format!("{name:?}: the bundle carries it twice"));
        }
    }
    Ok(())
}

/// A name that can only land inside the directory it is unpacked into, and prints as itself.
fn safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.contains('/')
        && !name.contains('\\')
        && !name.chars().any(oslo::secrets::name::refused)
        && name != "."
        && name != ".."
}

fn take(bytes: &[u8], how_many: usize) -> Result<(&[u8], &[u8]), String> {
    if bytes.len() < how_many {
        return Err(bad());
    }
    Ok(bytes.split_at(how_many))
}

fn bad() -> String {
    "the bundle of secrets is truncated or damaged".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(at: &Path, name: &str, body: &[u8]) {
        std::fs::create_dir_all(at).expect("dir");
        std::fs::write(at.join(name), body).expect("write");
    }

    #[test]
    fn a_directory_survives_the_round_trip() {
        let dir = tempfile::tempdir().expect("dir");
        let from = dir.path().join("from");
        write(&from, "one.sealed", b"first");
        write(&from, "two.sealed", &[0, 159, 146, 150]);

        let into = dir.path().join("into");
        unpack(&into, &pack(&from).expect("pack")).expect("unpack");
        assert_eq!(
            std::fs::read(into.join("one.sealed")).expect("read"),
            b"first"
        );
        assert_eq!(
            std::fs::read(into.join("two.sealed")).expect("read"),
            [0, 159, 146, 150]
        );
    }

    /// The same directory always packs to the same bytes.
    #[test]
    fn packing_is_stable() {
        let dir = tempfile::tempdir().expect("dir");
        let from = dir.path().join("from");
        write(&from, "b.sealed", b"two");
        write(&from, "a.sealed", b"one");
        assert_eq!(pack(&from).expect("pack"), pack(&from).expect("pack"));
    }

    #[test]
    fn an_empty_or_missing_directory_is_not_an_error() {
        let dir = tempfile::tempdir().expect("dir");
        let packed = pack(&dir.path().join("nothing-here")).expect("pack");
        let into = dir.path().join("into");
        unpack(&into, &packed).expect("unpack");
        assert_eq!(std::fs::read_dir(&into).expect("read").count(), 0);
    }

    /// **A hostile far end cannot write outside the directory.**
    #[test]
    fn a_name_that_would_escape_is_refused() {
        let dir = tempfile::tempdir().expect("dir");
        for bad in ["../escaped", "sub/dir", "..", "."] {
            let mut bytes = MAGIC.to_vec();
            bytes.extend_from_slice(&(bad.len() as u32).to_le_bytes());
            bytes.extend_from_slice(bad.as_bytes());
            bytes.extend_from_slice(&0u64.to_le_bytes());
            let into = dir.path().join("into");
            assert!(unpack(&into, &bytes).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_damaged_bundle_is_refused_rather_than_half_read() {
        let dir = tempfile::tempdir().expect("dir");
        let from = dir.path().join("from");
        write(&from, "one.sealed", b"first");
        let packed = pack(&from).expect("pack");

        let into = dir.path().join("into");
        assert!(unpack(&into, &packed[..packed.len() - 2]).is_err());
        assert!(unpack(&into, b"not a bundle at all").is_err());
    }

    /// Only regular files travel: a link to something outside, a FIFO and the store's own `.lock`
    /// are passed over without being opened, so packing neither hangs nor reads elsewhere.
    #[test]
    fn only_regular_files_are_packed() {
        let dir = tempfile::tempdir().expect("dir");
        let from = dir.path().join("from");
        write(&from, "one.sealed", b"first");
        write(&from, ".lock", b"");
        write(dir.path(), "outside", b"not the store's");
        std::os::unix::fs::symlink(dir.path().join("outside"), from.join("link.sealed"))
            .expect("symlink");
        nix::unistd::mkfifo(&from.join("ff.sealed"), nix::sys::stat::Mode::S_IRWXU)
            .expect("mkfifo");

        let into = dir.path().join("into");
        unpack(&into, &pack(&from).expect("pack")).expect("unpack");
        let mut landed: Vec<String> = std::fs::read_dir(&into)
            .expect("read")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        landed.sort();
        assert_eq!(landed, ["one.sealed"]);
    }

    fn one(name: &str) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.push(b'x');
        bytes
    }

    /// A name that would write to the terminal when listed is refused, and so is one sent twice.
    #[test]
    fn a_hostile_or_repeated_name_is_refused() {
        let dir = tempfile::tempdir().expect("dir");
        for bad in [
            "\x1b[2Jx.sealed",
            "line\nbreak.sealed",
            "rtl\u{202e}.sealed",
        ] {
            assert!(unpack(&dir.path().join("a"), &one(bad)).is_err(), "{bad:?}");
        }
        let mut twice = one("same.sealed");
        twice.extend_from_slice(&one("same.sealed")[MAGIC.len()..]);
        assert!(unpack(&dir.path().join("b"), &twice).is_err());
    }

    /// A half-written file is somebody's in-progress write, not part of the store.
    #[test]
    fn scratch_files_are_left_behind() {
        let dir = tempfile::tempdir().expect("dir");
        let from = dir.path().join("from");
        write(&from, "one.sealed", b"first");
        write(&from, "one.new", b"half");

        let into = dir.path().join("into");
        unpack(&into, &pack(&from).expect("pack")).expect("unpack");
        assert!(into.join("one.sealed").exists());
        assert!(!into.join("one.new").exists());
    }
}
