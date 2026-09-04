//! Inotify ownership and event decoding for watch services.

use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify, WatchDescriptor};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

/// A decoded filesystem or watch-set event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    Write,
    Modify,
    Create,
    Delete,
    MoveFrom,
    MoveTo,
    Attribute,
    Open,
    Read,
    Overflow,
    Ignored,
    Unmount,
    Other,
}

/// One event with the watched directory restored from its descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub directory: Option<PathBuf>,
    pub name: Option<OsString>,
    pub kind: EventKind,
    pub is_directory: bool,
}

impl Event {
    pub fn path(&self) -> Option<PathBuf> {
        match (&self.directory, &self.name) {
            (Some(dir), Some(name)) => Some(dir.join(name)),
            (Some(dir), None) => Some(dir.clone()),
            _ => None,
        }
    }
}

/// One nonblocking inotify instance owning any number of directory watches.
pub struct EventSource {
    inotify: Inotify,
    directories: HashMap<WatchDescriptor, PathBuf>,
}

impl EventSource {
    pub fn open() -> io::Result<Self> {
        let inotify =
            Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC).map_err(errno)?;
        Ok(Self {
            inotify,
            directories: HashMap::new(),
        })
    }

    pub fn add(&mut self, path: &Path) -> io::Result<()> {
        let descriptor = self
            .inotify
            .add_watch(path, watched_flags())
            .map_err(errno)?;
        self.directories.insert(descriptor, path.to_path_buf());
        Ok(())
    }

    pub fn read(&mut self) -> io::Result<Vec<Event>> {
        let events = match self.inotify.read_events() {
            Ok(events) => events,
            Err(nix::errno::Errno::EAGAIN) => return Ok(Vec::new()),
            Err(error) => return Err(errno(error)),
        };
        let mut out = Vec::new();
        for raw in events {
            let directory = self.directories.get(&raw.wd).cloned();
            let kind = decode(raw.mask);
            if kind == EventKind::Ignored {
                self.directories.remove(&raw.wd);
            }
            out.push(Event {
                directory,
                name: raw.name,
                kind,
                is_directory: raw.mask.contains(AddWatchFlags::IN_ISDIR),
            });
        }
        Ok(out)
    }

    pub fn descriptor_count(&self) -> usize {
        self.directories.len()
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.inotify.as_fd()
    }
}

fn watched_flags() -> AddWatchFlags {
    AddWatchFlags::IN_CLOSE_WRITE
        | AddWatchFlags::IN_MODIFY
        | AddWatchFlags::IN_CREATE
        | AddWatchFlags::IN_DELETE
        | AddWatchFlags::IN_MOVED_FROM
        | AddWatchFlags::IN_MOVED_TO
        | AddWatchFlags::IN_ATTRIB
        | AddWatchFlags::IN_OPEN
        | AddWatchFlags::IN_ACCESS
        | AddWatchFlags::IN_DELETE_SELF
        | AddWatchFlags::IN_MOVE_SELF
        | AddWatchFlags::IN_UNMOUNT
}

fn decode(mask: AddWatchFlags) -> EventKind {
    for (flag, kind) in [
        (AddWatchFlags::IN_Q_OVERFLOW, EventKind::Overflow),
        (AddWatchFlags::IN_IGNORED, EventKind::Ignored),
        (AddWatchFlags::IN_UNMOUNT, EventKind::Unmount),
        (AddWatchFlags::IN_CLOSE_WRITE, EventKind::Write),
        (AddWatchFlags::IN_MODIFY, EventKind::Modify),
        (AddWatchFlags::IN_CREATE, EventKind::Create),
        (AddWatchFlags::IN_DELETE, EventKind::Delete),
        (AddWatchFlags::IN_MOVED_FROM, EventKind::MoveFrom),
        (AddWatchFlags::IN_MOVED_TO, EventKind::MoveTo),
        (AddWatchFlags::IN_ATTRIB, EventKind::Attribute),
        (AddWatchFlags::IN_OPEN, EventKind::Open),
        (AddWatchFlags::IN_ACCESS, EventKind::Read),
    ] {
        if mask.contains(flag) {
            return kind;
        }
    }
    EventKind::Other
}

fn errno(error: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

    #[test]
    fn one_source_owns_multiple_directories() {
        let root = tempfile::tempdir().expect("tempdir");
        let a = root.path().join("a");
        let b = root.path().join("b");
        std::fs::create_dir_all(&a).expect("a");
        std::fs::create_dir_all(&b).expect("b");
        let mut source = EventSource::open().expect("source");
        source.add(&a).expect("watch a");
        source.add(&b).expect("watch b");
        assert_eq!(source.descriptor_count(), 2);

        std::fs::write(b.join("note"), "x").expect("write");
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let events = source.read().expect("events");
            if events
                .iter()
                .any(|event| event.path() == Some(b.join("note")))
            {
                break;
            }
            assert!(Instant::now() < deadline, "event did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn directory_changes_keep_their_distinct_event_kinds() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut source = EventSource::open().expect("source");
        source.add(root.path()).expect("watch root");
        let first = root.path().join("first");
        let second = root.path().join("second");
        std::fs::write(&first, "x").expect("create");
        std::fs::rename(&first, &second).expect("move");
        std::fs::remove_file(&second).expect("delete");

        let wanted = [
            EventKind::Create,
            EventKind::MoveFrom,
            EventKind::MoveTo,
            EventKind::Delete,
        ];
        let mut seen = HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !wanted.iter().all(|kind| seen.contains(kind)) && Instant::now() < deadline {
            for event in source.read().expect("events") {
                seen.insert(event.kind);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        for kind in wanted {
            assert!(seen.contains(&kind), "missing {kind:?}: {seen:?}");
        }
    }

    #[test]
    fn control_masks_are_decoded_before_content_masks() {
        for (mask, expected) in [
            (AddWatchFlags::IN_Q_OVERFLOW, EventKind::Overflow),
            (AddWatchFlags::IN_IGNORED, EventKind::Ignored),
            (AddWatchFlags::IN_UNMOUNT, EventKind::Unmount),
        ] {
            assert_eq!(decode(mask | AddWatchFlags::IN_MODIFY), expected);
        }
    }
}
