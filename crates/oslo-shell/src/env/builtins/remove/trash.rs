//! Moving a removal aside instead of destroying it.
//!
//! # The size cap is not arbitrary
//!
//! The trash directory is usually on a different filesystem from the file — `/tmp` almost always
//! is, and on most distributions it is `tmpfs`, which is RAM. `rename(2)` cannot cross a
//! filesystem, so a move to `/tmp` is a **copy** followed by an unlink: trashing a 4 GB file
//! copies 4 GB and then holds it in memory until the next reboot.
//!
//! `max_to_tmp` bounds that. Under it, the cost is not worth noticing; over it, the file is
//! destroyed as `rm` has always destroyed things. Where the trash happens to be on the same
//! filesystem the move is a plain rename and costs nothing at any size — but the cap is applied
//! either way, because a rule that changes with the mount table is a rule nobody can predict.
//!
//! # A directory is measured before it is moved
//!
//! Which means walking it. The walk stops as soon as the total passes the cap, so the expensive
//! case — a huge tree, which is destroyed anyway — is also the one that stops early.

use oslo_ui::settings::Rm;
use std::io;
use std::path::{Path, PathBuf};

/// Where removals go, and how large one may be to go there.
pub struct Trash {
    directory: PathBuf,
    /// Bytes, from `max_to_tmp` megabytes.
    limit: u64,
}

impl Trash {
    pub fn new(settings: &Rm) -> Trash {
        Trash {
            directory: PathBuf::from(&settings.trash),
            limit: settings.max_to_tmp.saturating_mul(1024 * 1024),
        }
    }

    /// Move `path` into the trash.
    ///
    /// `None` means "too big — destroy it", which is the caller's job rather than this module's:
    /// the caller already knows how to remove a thing, and a trash that also deletes would be two
    /// answers to one question.
    pub fn take(&self, path: &Path, shown: &str, directory: bool) -> Option<io::Result<PathBuf>> {
        // Already in the trash: removing it from there means removing it. With the trash at `/tmp`,
        // `rm x` in `/tmp` renamed `x` to `x.1` beside itself, and every retry added another `.1`.
        if self.holds(path) || size_over(path, directory, self.limit) {
            return None;
        }
        Some(self.move_aside(path, shown))
    }

    /// Whether `path` already sits inside the trash directory.
    ///
    /// The directory holding it is resolved, never `path` itself: a symlink operand is one entry,
    /// and resolving it would ask about wherever it points.
    fn holds(&self, path: &Path) -> bool {
        let Ok(trash) = self.directory.canonicalize() else {
            return false;
        };
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        parent
            .canonicalize()
            .is_ok_and(|parent| parent.starts_with(&trash))
    }

    fn move_aside(&self, path: &Path, shown: &str) -> io::Result<PathBuf> {
        std::fs::create_dir_all(&self.directory)?;
        let target = self.free_name(shown);
        match std::fs::rename(path, &target) {
            Ok(()) => Ok(target),
            // Across a filesystem boundary a rename cannot work, and copying is the only way.
            // Checked by errno rather than by comparing device numbers, because the kernel is the
            // authority on what counts as one filesystem and `st_dev` is not (bind mounts).
            Err(e) if e.raw_os_error() == Some(nix::libc::EXDEV) => {
                copy_across(path, &target)?;
                remove_any(path)?;
                Ok(target)
            }
            Err(e) => Err(e),
        }
    }

    /// A name in the trash that is not taken.
    ///
    /// Deleting two files called `notes.txt` from two directories must not have the second
    /// silently replace the first — that would be a data loss committed by the feature whose
    /// entire purpose is preventing one. Numbered rather than timestamped so the name stays
    /// readable: with no restore command yet, finding a file again means recognising it.
    fn free_name(&self, shown: &str) -> PathBuf {
        let base = Path::new(shown)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "removed".to_string());
        let first = self.directory.join(&base);
        if !exists(&first) {
            return first;
        }
        for n in 1..10_000 {
            let candidate = self.directory.join(format!("{base}.{n}"));
            if !exists(&candidate) {
                return candidate;
            }
        }
        self.directory.join(format!("{base}.full"))
    }
}

/// `symlink_metadata`, so a dangling symlink in the trash still counts as taken.
fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Whether `path` is larger than `limit`, stopping the walk as soon as the answer is yes.
fn size_over(path: &Path, directory: bool, limit: u64) -> bool {
    if !directory {
        return std::fs::symlink_metadata(path).is_ok_and(|m| m.len() > limit);
    }
    let mut total = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                pending.push(entry.path());
                continue;
            }
            total = total.saturating_add(meta.len());
            if total > limit {
                return true;
            }
        }
    }
    false
}

/// Copy a file, a symlink or a whole tree to `target`.
fn copy_across(from: &Path, target: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        // Copied as a link, not as what it points at. Following it would turn trashing a symlink
        // into copying whatever it aimed at, which could be anything at all.
        return std::os::unix::fs::symlink(std::fs::read_link(from)?, target);
    }
    if !meta.is_dir() {
        std::fs::copy(from, target)?;
        return Ok(());
    }
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        copy_across(&entry.path(), &target.join(entry.file_name()))?;
    }
    Ok(())
}

fn remove_any(path: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::Trash;
    use oslo_ui::settings::Rm;

    fn trash_at(dir: &std::path::Path) -> Trash {
        Trash::new(&Rm {
            to_tmp: true,
            max_to_tmp: 100,
            trash: dir.display().to_string(),
        })
    }

    /// **What is already in the trash is not moved again** — it is declined, so the caller
    /// destroys it. With the trash at `/tmp`, `rm x` in `/tmp` renamed `x` to `x.1` beside itself.
    #[test]
    fn what_is_already_in_the_trash_is_not_moved_again() {
        let bin = tempfile::tempdir().unwrap();
        let trash = trash_at(bin.path());
        let inside = bin.path().join("x");
        std::fs::write(&inside, "x").unwrap();
        assert!(trash.take(&inside, "x", false).is_none());
        assert!(inside.exists(), "declined, not moved");
        assert!(!bin.path().join("x.1").exists(), "and no copy beside it");

        let elsewhere = tempfile::tempdir().unwrap();
        let outside = elsewhere.path().join("y");
        std::fs::write(&outside, "y").unwrap();
        let moved = trash.take(&outside, "y", false).unwrap().unwrap();
        assert_eq!(
            moved,
            bin.path().join("y"),
            "anything else still goes to the trash"
        );
    }
}
