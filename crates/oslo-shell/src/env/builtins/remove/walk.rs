//! Removing an operand one entry at a time, through descriptors rather than paths.
//!
//! # Why not `remove_dir_all`
//!
//! `std::fs::remove_dir_all` is one call and answers one error, and both of those are wrong for
//! `rm`. A tree with a single unreadable file underneath it came back as
//!
//! ```text
//! oslo: rm: cannot remove 'noperm': Permission denied
//! ```
//!
//! which names a directory that is perfectly readable, and stops — leaving every entry it had not
//! reached yet in place. `rm -rf noperm` on GNU names `noperm/inner/f`, removes everything else,
//! and exits 1. The difference matters most in exactly the case people reach for `rm -rf`: a build
//! tree with one root-owned file in it, where "it failed" and "it failed *here*, and the rest is
//! gone" are different problems.
//!
//! So the walk is oslo's own: it reports the path that actually failed, carries on to the entries
//! that can still go, prints a `-v` line per entry, prompts per entry under `-i`, and can be
//! stopped with a Ctrl-C part-way through.
//!
//! # Why every operation names a descriptor and a filename
//!
//! The first version of this walk did all of that by path — `read_dir("a/b")`,
//! `remove_file("a/b/c")` — and was therefore open to the oldest race there is. Between deciding
//! that `a/b` is a directory and reading it, anything that can write to `a` may replace `a/b` with
//! a symlink; every later path-based call then resolves through the link, and `rm -r` empties
//! somewhere it was never pointed at. Not theoretical: swapping the directory while the walk sat
//! on an `-i` prompt deleted a file outside the tree on the first attempt.
//!
//! The fix is the one `std` and GNU's `rm` both use: hold an **open descriptor** for each
//! directory and reach everything inside it with `openat`, `fstatat`, `unlinkat` and `fdopendir`.
//! A descriptor still refers to the directory that was opened even if the name now points
//! somewhere else, so a filename is resolved once, by the kernel, against something that cannot be
//! substituted. `O_NOFOLLOW` on the descent turns the attack into a plain error.
//!
//! The operand itself is still named by path, because that is the name the user typed and there is
//! no earlier descriptor to anchor it to. GNU has the same exposure in the same place.
//!
//! # Three more things the walk has to get right
//!
//! * **A symlink is one entry.** Never descended into, only unlinked — `is_dir` is asked of an
//!   `fstatat` that does not follow links.
//! * **An explicit stack, not recursion.** Depth is whatever the filesystem holds, and a tree deep
//!   enough to overflow the Rust stack is a tree someone can build on purpose. One descriptor is
//!   held per level currently open, which is the same cost `std` pays.
//! * **A parent whose child failed says nothing.** It cannot be removed either, but the `ENOTEMPTY`
//!   is a consequence of a failure already reported, and printing one per level buries the line
//!   that says what is actually wrong.

mod ask;
pub use ask::confirm;
use ask::{ask_at, ask_path};

use nix::dir::Dir;
use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstatat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use std::ffi::{OsStr, OsString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::rc::Rc;

/// How one operand should be taken apart.
pub struct Walk {
    /// Where a diagnostic says it came from — `env.origin()`, so a failure inside a script names
    /// the script and the line rather than the shell.
    pub origin: String,
    /// `-f`: missing entries are not errors, and nothing is ever prompted for.
    pub force: bool,
    /// `-i`: prompt before each entry, and before descending into each directory.
    pub interactive: bool,
    /// `-r`: descend. Without it a directory operand is removed only if it is empty.
    pub recursive: bool,
    /// `-v`: name each entry as it goes.
    pub verbose: bool,
}

/// What became of an operand.
pub struct Outcome {
    /// Whether anything under it could not be removed.
    pub failed: bool,
    /// Whether a Ctrl-C stopped the walk part-way.
    pub interrupted: bool,
}

/// A directory the walk currently has open, shared by every step naming something inside it.
///
/// `Rc` rather than a plain field so the descriptor lives exactly as long as the steps that need
/// it: the last child popped off the stack drops the last handle, and the level closes.
type Level = Rc<OwnedFd>;

/// One item of work, each naming a filename *within* an open directory.
enum Step {
    /// Remove `name`, descending into it first when it is a directory.
    Enter {
        parent: Level,
        name: OsString,
        shown: String,
    },
    /// Remove the directory `name`, whose contents have now been dealt with. `before` is the
    /// failure count from when it was entered.
    Leave {
        parent: Level,
        name: OsString,
        shown: String,
        before: usize,
    },
}

/// Remove `root`, and everything under it when `recursive`.
pub fn remove_tree(root: &Path, shown: &str, walk: &Walk) -> Outcome {
    // The operand is named by path — see the module docs — and only what is *inside* it is reached
    // through a descriptor.
    let meta = match std::fs::symlink_metadata(root) {
        Ok(meta) => meta,
        // The caller stats and reports before calling; reaching here means it went in between.
        Err(_) => return done(usize::from(!walk.force), false),
    };

    if !meta.is_dir() {
        if !ask_path(walk, shown, describe(&meta), root) {
            return done(0, false);
        }
        return done(
            report(std::fs::remove_file(root), shown, false, walk),
            false,
        );
    }

    if !walk.recursive {
        if !ask_path(walk, shown, "directory", root) {
            return done(0, false);
        }
        return done(report(std::fs::remove_dir(root), shown, true, walk), false);
    }

    // Held for the whole descent — see `Descriptors`.
    let _budget = Descriptors::widened();

    let level = match open_dir(None, root, walk, shown) {
        Ok(level) => level,
        // **A directory that cannot be read may still be empty**, and an empty one in a writable
        // parent unlinks: the mode on the directory itself says nothing about the parent's. GNU rm
        // falls back to `rmdir` here, and without it `rm -rf` left every mode-000 or mode-111
        // directory on disk and returned 1 — test fixtures, extracted archives, and this project's
        // own sandbox cleanup, which is where it was found. If the directory has anything in it the
        // unlink fails and the original error is reported, which is what GNU rm says too.
        Err(e) => match std::fs::remove_dir(root) {
            Ok(()) => return done(0, false),
            Err(_) => {
                report_unopenable(walk, shown, e);
                return done(1, false);
            }
        },
    };

    let mut failures = 0usize;
    let mut stack = Vec::new();
    match children(&level, shown) {
        Ok(entries) => {
            // The operand gets the same descend prompt its subdirectories do — asked here rather
            // than in `visit` only because the operand has no parent descriptor to be reached from.
            if !entries.is_empty()
                && walk.interactive
                && !walk.force
                && !confirm(&walk.origin, &format!("descend into directory '{shown}'"))
            {
                return done(0, false);
            }
            queue(&mut stack, entries);
        }
        Err(e) => {
            complain(walk, shown, e);
            failures += 1;
        }
    }

    let interrupted = drain(&mut stack, walk, &mut failures);

    // The operand goes last, and only if everything under it did. Still by path, and still safe:
    // `remove_dir` will not follow a symlink, so the worst a swap can do here is fail.
    if !interrupted && failures == 0 && ask_path(walk, shown, "directory", root) {
        failures += report(std::fs::remove_dir(root), shown, true, walk);
    }

    done(failures, interrupted)
}

/// The soft descriptor limit, raised to the hard one for as long as a descent needs it.
///
/// # Why this is here at all
///
/// Traversing by descriptor costs one descriptor per level currently open, and there is no way
/// around that for a walk that happens *inside* the shell: the alternative is `fchdir`, which is
/// how GNU's `rm` keeps the cost at one — and which a builtin cannot use, because the working
/// directory it would be moving is the shell's own. `std::fs::remove_dir_all` pays the same cost
/// and fails the same way, measured: a 1500-deep tree under `ulimit -n 1024` defeats both, and
/// removes nothing.
///
/// Raising the *soft* limit to the hard one is something any process may do, and is what makes the
/// difference between "no" and "yes" for every depth anyone will really meet — a typical hard
/// limit is half a million. It is put back on the way out, so `ulimit -n` reports what it always
/// did and nothing the shell runs afterwards can tell this happened. Nothing is spawned during a
/// walk, so there is no child to inherit the raised limit in between.
struct Descriptors(Option<rlimit::Rlimit>);

/// The pair `getrlimit` answers, kept in its own type so the guard reads as what it restores.
mod rlimit {
    pub type Rlimit = (nix::sys::resource::rlim_t, nix::sys::resource::rlim_t);
}

impl Descriptors {
    fn widened() -> Descriptors {
        use nix::sys::resource::{Resource, getrlimit, setrlimit};
        let Ok((soft, hard)) = getrlimit(Resource::RLIMIT_NOFILE) else {
            return Descriptors(None);
        };
        if soft >= hard {
            return Descriptors(None);
        }
        match setrlimit(Resource::RLIMIT_NOFILE, hard, hard) {
            Ok(()) => Descriptors(Some((soft, hard))),
            Err(_) => Descriptors(None),
        }
    }
}

impl Drop for Descriptors {
    fn drop(&mut self) {
        use nix::sys::resource::{Resource, setrlimit};
        if let Some((soft, hard)) = self.0 {
            let _ = setrlimit(Resource::RLIMIT_NOFILE, soft, hard);
        }
    }
}

/// Open a directory without following a link, reporting and answering `Err` if it will not open.
fn open_dir(parent: Option<RawFd>, path: &Path, walk: &Walk, shown: &str) -> Result<Level, Errno> {
    let _ = (walk, shown);
    // **`O_NOFOLLOW` is the whole defence.** If the name became a symlink since it was stat-ed,
    // the open fails with `ELOOP` rather than landing somewhere else.
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    match openat(parent, path, flags, Mode::empty()) {
        // SAFETY: `openat` answers a descriptor it does not keep, so this is its only owner.
        Ok(fd) => Ok(Rc::new(unsafe { OwnedFd::from_raw_fd(fd) })),
        // Not reported here any more: the caller first tries to remove the directory anyway, and a
        // directory that then unlinks was never a failure to tell anyone about.
        Err(e) => Err(e),
    }
}

/// Say that a directory could not be opened, once the caller has established it cannot go either.
fn report_unopenable(walk: &Walk, shown: &str, e: Errno) {
    complain(walk, shown, e);
    // **The one error whose cause is not in the message.** `Too many open files` under a
    // path several thousand components long says nothing about why, and the why is the
    // only actionable part: the tree is deeper than the descriptor limit, and `Descriptors`
    // has already raised it as far as it is allowed to.
    if e == Errno::EMFILE || e == Errno::ENFILE {
        eprintln!(
            "{}rm: this tree is deeper than the open-file limit allows; \
             raise the hard limit (`ulimit -Hn`) or remove it in parts",
            walk.origin
        );
    }
}

/// Work the stack down to nothing, or until a Ctrl-C says to stop. `true` if it was stopped.
fn drain(stack: &mut Vec<Step>, walk: &Walk, failures: &mut usize) -> bool {
    while let Some(step) = stack.pop() {
        // Between entries, which is the only place a builtin can be stopped: it runs in the shell
        // process, so the keystroke sets a flag that nothing would otherwise look at until the
        // whole `rm` had finished. Peeked rather than taken — see `job::interrupt_waiting`.
        if crate::exec::job::interrupt_waiting() {
            return true;
        }
        match step {
            Step::Enter {
                parent,
                name,
                shown,
            } => visit(stack, &parent, &name, &shown, walk, failures),
            Step::Leave {
                parent,
                name,
                shown,
                before,
            } => {
                // Something under it could not go, so it cannot either — and the entry that
                // actually failed has already said so.
                if *failures > before {
                    continue;
                }
                if !ask_at(walk, &shown, "directory", &parent, &name) {
                    continue;
                }
                let gone = unlinkat(
                    Some(parent.as_raw_fd()),
                    name.as_os_str(),
                    UnlinkatFlags::RemoveDir,
                );
                *failures += report_nix(gone, &shown, true, walk);
            }
        }
    }
    false
}

/// Deal with one name inside an open directory.
fn visit(
    stack: &mut Vec<Step>,
    parent: &Level,
    name: &OsString,
    shown: &str,
    walk: &Walk,
    failures: &mut usize,
) {
    let at = Some(parent.as_raw_fd());
    let stat = match fstatat(at, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::ENOENT) if walk.force => return,
        Err(e) => {
            complain(walk, shown, e);
            *failures += 1;
            return;
        }
    };

    // A symlink is an entry, never a way down: this is the test that keeps `rm -r` inside the tree
    // it was pointed at.
    let directory = kind_bits(&stat) == SFlag::S_IFDIR.bits();
    if !directory || !walk.recursive {
        let kind = if directory {
            "directory"
        } else {
            kind_of(&stat)
        };
        if !ask_at(walk, shown, kind, parent, name) {
            return;
        }
        let how = if directory {
            UnlinkatFlags::RemoveDir
        } else {
            UnlinkatFlags::NoRemoveDir
        };
        let gone = unlinkat(at, name.as_os_str(), how);
        *failures += report_nix(gone, shown, directory, walk);
        return;
    }

    let level = match open_dir(at, Path::new(name), walk, shown) {
        Ok(level) => level,
        // The same fallback as the operand above, for a directory found part-way down: unreadable
        // does not mean non-empty, and an empty one still unlinks from its parent.
        Err(_) if unlinkat(at, name.as_os_str(), UnlinkatFlags::RemoveDir).is_ok() => return,
        Err(e) => {
            report_unopenable(walk, shown, e);
            *failures += 1;
            return;
        }
    };
    let entries = match children(&level, shown) {
        Ok(entries) => entries,
        Err(e) => {
            complain(walk, shown, e);
            *failures += 1;
            return;
        }
    };

    // The descend prompt is asked only when there is something to descend into; an empty directory
    // gets the one question that applies to it, from the `Leave` step below.
    if !entries.is_empty()
        && walk.interactive
        && !walk.force
        && !confirm(&walk.origin, &format!("descend into directory '{shown}'"))
    {
        return;
    }

    stack.push(Step::Leave {
        parent: Rc::clone(parent),
        name: name.clone(),
        shown: shown.to_string(),
        before: *failures,
    });
    queue(stack, entries);
}

/// Push children so that popping yields them in the order the directory listed them.
fn queue(stack: &mut Vec<Step>, entries: Vec<Step>) {
    for entry in entries.into_iter().rev() {
        stack.push(entry);
    }
}

/// The entries of an open directory, as steps that name it as their parent.
///
/// Read through `fdopendir` on a duplicate of the descriptor, because `Dir` closes what it is
/// given and the level's own handle has `unlinkat` calls still to come.
fn children(level: &Level, shown: &str) -> Result<Vec<Step>, Errno> {
    let copy: RawFd = nix::unistd::dup(level.as_raw_fd())?;
    let mut dir = Dir::from_fd(copy)?;
    let stem = shown.trim_end_matches('/');
    let mut entries = Vec::new();
    for entry in dir.iter() {
        let entry = entry?;
        let raw = entry.file_name().to_bytes();
        if raw == b"." || raw == b".." {
            continue;
        }
        let name = OsStr::from_bytes(raw).to_os_string();
        // Lossy only for the *message*: every operation uses the bytes above, so a name that is
        // not valid UTF-8 is still removed correctly and merely printed with a replacement char.
        entries.push(Step::Enter {
            parent: Rc::clone(level),
            shown: format!("{stem}/{}", name.to_string_lossy()),
            name,
        });
    }
    Ok(entries)
}

/// The file-type half of a `stat`'s mode.
fn kind_bits(stat: &FileStat) -> nix::libc::mode_t {
    stat.st_mode & SFlag::S_IFMT.bits()
}

/// Say what happened, and answer 1 when it failed so callers can add it up.
fn report(removed: std::io::Result<()>, shown: &str, directory: bool, walk: &Walk) -> usize {
    match removed {
        Ok(()) => {
            announce(shown, directory, walk);
            0
        }
        Err(e) => {
            eprintln!(
                "{}rm: cannot remove {}: {}",
                walk.origin,
                oslo_base::shown::quoted(shown),
                oslo_base::error::reason(&e)
            );
            1
        }
    }
}

/// The same, for the `nix` calls that answer an `Errno`.
fn report_nix(removed: Result<(), Errno>, shown: &str, directory: bool, walk: &Walk) -> usize {
    match removed {
        Ok(()) => {
            announce(shown, directory, walk);
            0
        }
        Err(e) => {
            complain(walk, shown, e);
            1
        }
    }
}

fn announce(shown: &str, directory: bool, walk: &Walk) {
    if !walk.verbose {
        return;
    }
    if directory {
        println!("removed directory '{shown}'");
    } else {
        println!("removed '{shown}'");
    }
}

fn complain(walk: &Walk, shown: &str, e: Errno) {
    eprintln!(
        "{}rm: cannot remove {}: {}",
        walk.origin,
        oslo_base::shown::quoted(shown),
        e.desc()
    );
}

fn done(failures: usize, interrupted: bool) -> Outcome {
    Outcome {
        failed: failures > 0,
        interrupted,
    }
}

/// What `rm` calls this kind of entry when it asks about it, from a `stat`.
fn kind_of(stat: &FileStat) -> &'static str {
    let bits = kind_bits(stat);
    if bits == SFlag::S_IFLNK.bits() {
        "symbolic link"
    } else if bits == SFlag::S_IFDIR.bits() {
        "directory"
    } else if bits == SFlag::S_IFIFO.bits() {
        "fifo"
    } else if bits == SFlag::S_IFSOCK.bits() {
        "socket"
    } else if bits == SFlag::S_IFCHR.bits() {
        "character special file"
    } else if bits == SFlag::S_IFBLK.bits() {
        "block special file"
    } else if stat.st_size == 0 {
        "regular empty file"
    } else {
        "regular file"
    }
}

/// The same, for the operand, which the caller already holds a `Metadata` for.
///
/// GNU's wording, because a prompt is something people answer by reading it, and "regular empty
/// file" is the difference between deleting a stub and deleting a day's work.
pub fn describe(meta: &std::fs::Metadata) -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    let kind = meta.file_type();
    if kind.is_symlink() {
        "symbolic link"
    } else if kind.is_dir() {
        "directory"
    } else if kind.is_fifo() {
        "fifo"
    } else if kind.is_socket() {
        "socket"
    } else if kind.is_char_device() {
        "character special file"
    } else if kind.is_block_device() {
        "block special file"
    } else if meta.len() == 0 {
        "regular empty file"
    } else {
        "regular file"
    }
}

#[cfg(test)]
#[path = "walk/tests.rs"]
mod tests;
