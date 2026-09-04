//! Directory-watch installation, recursion, and rebuilds.

use super::event::{Event, EventKind, EventSource};
use super::pattern::PatternSet;
use std::collections::HashSet;
use std::io;
use std::os::fd::BorrowedFd;
use std::path::{Path, PathBuf};

pub struct WatchSet {
    patterns: PatternSet,
    source: EventSource,
    installed: HashSet<PathBuf>,
}

impl WatchSet {
    pub fn open(root: &Path, patterns: &[String]) -> io::Result<Self> {
        let patterns = PatternSet::compile(root, patterns)?;
        let source = EventSource::open()?;
        let mut set = Self {
            patterns,
            source,
            installed: HashSet::new(),
        };
        set.install_all()?;
        Ok(set)
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.source.as_fd()
    }

    pub fn read(&mut self) -> io::Result<bool> {
        let mut dirty = false;
        let events = self.source.read()?;
        for event in events {
            dirty |= self.observe(&event)?;
        }
        Ok(dirty)
    }

    fn observe(&mut self, event: &Event) -> io::Result<bool> {
        if matches!(
            event.kind,
            EventKind::Overflow | EventKind::Ignored | EventKind::Unmount
        ) {
            self.rebuild()?;
            return Ok(true);
        }
        let Some(path) = event.path() else {
            return Ok(false);
        };
        if matches!(
            event.kind,
            EventKind::Open | EventKind::Read | EventKind::Other
        ) {
            return Ok(false);
        }
        if event.is_directory && matches!(event.kind, EventKind::Create | EventKind::MoveTo) {
            self.install_new_tree(&path)?;
        }
        Ok(self.patterns.matches(&path))
    }

    fn rebuild(&mut self) -> io::Result<()> {
        self.source = EventSource::open()?;
        self.installed.clear();
        self.install_all()
    }

    fn install_all(&mut self) -> io::Result<()> {
        let roots: Vec<_> = self.patterns.roots().cloned().collect();
        for root in roots {
            if let Some(parent) = root.path.parent() {
                self.install_nearest(parent)?;
            }
            self.install_nearest(&root.path)?;
            if root.recursive && root.path.is_dir() {
                self.install_tree(&root.path)?;
            }
        }
        Ok(())
    }

    fn install_nearest(&mut self, path: &Path) -> io::Result<()> {
        let mut current = path;
        while !current.is_dir() {
            let Some(parent) = current.parent() else {
                return Ok(());
            };
            current = parent;
        }
        self.install(current)
    }

    fn install_new_tree(&mut self, path: &Path) -> io::Result<()> {
        let roots: Vec<_> = self.patterns.roots().cloned().collect();
        for root in roots {
            if path == root.path {
                self.install(path)?;
            }
            if root.recursive && path.starts_with(&root.path) {
                self.install_tree(path)?;
            }
        }
        Ok(())
    }

    fn install_tree(&mut self, root: &Path) -> io::Result<()> {
        self.install(root)?;
        let Ok(entries) = std::fs::read_dir(root) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                self.install_tree(&path)?;
            }
        }
        Ok(())
    }

    fn install(&mut self, path: &Path) -> io::Result<()> {
        if self.installed.insert(path.to_path_buf()) {
            if let Err(error) = self.source.add(path) {
                self.installed.remove(path);
                if error.kind() != io::ErrorKind::NotFound {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn await_dirty(set: &mut WatchSet) -> bool {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if set.read().expect("read") {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn future_recursive_directories_are_installed() {
        let root = tempfile::tempdir().expect("root");
        let patterns = vec!["src/**/*.rs".to_string()];
        let mut set = WatchSet::open(root.path(), &patterns).expect("set");
        let nested = root.path().join("src/new/deep");
        std::fs::create_dir_all(&nested).expect("directories");
        let _ = await_dirty(&mut set);
        std::fs::write(nested.join("lib.rs"), "x").expect("file");
        assert!(await_dirty(&mut set));
    }

    #[test]
    fn symlinked_directories_are_not_traversed() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::create_dir(root.path().join("src")).expect("src");
        symlink(outside.path(), root.path().join("src/link")).expect("link");
        let patterns = vec!["src/**/*.rs".to_string()];
        let mut set = WatchSet::open(root.path(), &patterns).expect("set");
        std::fs::write(outside.path().join("lib.rs"), "x").expect("file");
        assert!(!await_dirty(&mut set));
    }
}
