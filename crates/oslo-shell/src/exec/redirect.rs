use crate::env::Environment;
use crate::expand::{expand_word, expand_word_to_string};
use nix::fcntl::{FcntlArg, FdFlag, fcntl};
use nix::unistd::dup2;
use oslo_base::ast::{RedirectKind, Redirection};
use oslo_base::error::{Result, ShellError, reason};
use oslo_base::shown::shown;
use std::fs::{File, OpenOptions};
use std::os::fd::RawFd;

mod heredoc;
mod varfd;

use heredoc::heredoc_body;
#[cfg(test)]
use heredoc::unlinked_temp_file;

/// Lowest descriptor number a saved copy may occupy.
///
/// bash reserves everything from 10 up for its own bookkeeping and so does this shell: a script
/// that writes `exec 3>log` or `cmd 2>&5` is entitled to descriptors 3..9, and a saved copy parked
/// there would be visible to — and clobberable by — the script that the save exists to protect.
///
/// Public because it is not only saves that have to stay clear of that range: anything the shell
/// keeps open for its own lifetime — the control socket in `oslo-runtime` among them — is
/// clobberable while it sits on a number a script may redirect.
pub const SAVE_FD_FLOOR: RawFd = 10;

/// Restores the descriptors a set of redirections overwrote, when it is dropped.
///
/// Each entry is the descriptor that was redirected plus, if it was open beforehand, a private
/// copy of what it used to point at. `None` means the descriptor was *closed* before the
/// redirection, and the only faithful restore is to close it again.
pub struct RedirectGuard {
    saved_fds: Vec<(RawFd, Option<RawFd>)>,
    /// When false, `apply` skips saving entirely and `Drop` has nothing to undo.
    save: bool,
}

impl Default for RedirectGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl RedirectGuard {
    pub fn new() -> Self {
        Self {
            saved_fds: Vec::new(),
            save: true,
        }
    }

    /// A guard for a forked child that is about to `execv`, where nothing is ever restored.
    ///
    /// The child's descriptor table dies with the `exec`, so saving copies of it only burns
    /// syscalls — and, if the shell ever stops calling `std::process::exit` on the failure path,
    /// risks handing the new program descriptors it was never meant to see.
    pub fn for_exec() -> Self {
        Self {
            saved_fds: Vec::new(),
            save: false,
        }
    }

    pub fn apply(&mut self, env: &mut Environment, redirections: &[Redirection]) -> Result<()> {
        for redir in redirections {
            let target_words = expand_word(env, &redir.target)?;
            // **A filename is one word or it is a mistake.** `f="a b"; echo hi > $f` used to join
            // the fields with a space and create a file literally called `a b`, reporting success;
            // `> $LOGFILES` wrote garbage somewhere and the intended file was never written. A
            // heredoc is exempt: its `target` is the delimiter, not a path.
            if !matches!(
                redir.kind,
                RedirectKind::Heredoc | RedirectKind::HeredocStrip
            ) && target_words.len() != 1
            {
                return Err(ShellError::ExecutionError(format!(
                    "{}: ambiguous redirect",
                    name_of(&redir.target, &target_words)
                )));
            }
            let target_str = target_words.join(" ");

            let target_fd = redir.fd.unwrap_or(match redir.kind {
                RedirectKind::Input
                | RedirectKind::DupInput
                | RedirectKind::Heredoc
                | RedirectKind::HeredocStrip
                | RedirectKind::ReadWrite => 0,
                _ => 1,
            });

            // `{name}>file`: the shell picks the descriptor, and it outlives the command.
            if let Some(name) = &redir.fd_var {
                varfd::apply(env, redir, name, &target_str)?;
                continue;
            }

            // A closed source fails here, before a saved copy of the target can take its number.
            if let Some(source) =
                moved_descriptor(&target_str).or_else(|| target_str.parse::<RawFd>().ok())
                && matches!(redir.kind, RedirectKind::DupInput | RedirectKind::DupOutput)
                && fcntl(source, FcntlArg::F_GETFD).is_err()
            {
                return Err(bad_descriptor(&source.to_string()));
            }

            if self.save {
                self.saved_fds.push((target_fd, save_fd(target_fd)));
            }

            match open_target(env, redir, &target_str)? {
                Some(file) => install(file, target_fd)?,
                None => self.duplicate(env, redir, target_fd, &target_str)?,
            }
        }
        Ok(())
    }

    /// `n<&m`, `n>&m`, `n>&-` and the move forms `n>&m-` / `n<&m-`.
    fn duplicate(
        &mut self,
        env: &mut Environment,
        redir: &Redirection,
        target_fd: RawFd,
        target_str: &str,
    ) -> Result<()> {
        if target_str == "-" {
            let _ = nix::unistd::close(target_fd);
        } else if let Some(src_fd) = moved_descriptor(target_str) {
            if self.save && src_fd != target_fd {
                self.saved_fds.push((src_fd, save_fd(src_fd)));
            }
            dup2(src_fd, target_fd).map_err(|_| bad_descriptor(&src_fd.to_string()))?;
            if src_fd != target_fd {
                let _ = nix::unistd::close(src_fd);
            }
        } else if let Ok(src_fd) = target_str.parse::<RawFd>() {
            // Report the descriptor number the script named, the way bash does; the raw errno
            // text ("EBADF: Bad file descriptor") names nothing the author can act on.
            dup2(src_fd, target_fd).map_err(|_| bad_descriptor(target_str))?;
        } else if redir.kind == RedirectKind::DupOutput && redir.fd.is_none() {
            // **`>&file` is `&>file`.** With no descriptor in front of it and a target that is not
            // a number, `>&` names a file and means "both streams" — the spelling
            // `cmd >&/dev/null` is everywhere.
            let file = open_for_output(target_str, env.noclobber())?;
            if self.save {
                self.saved_fds.push((2, save_fd(2)));
            }
            install(file, target_fd)?;
            // Standard error joins whatever standard output now is, rather than being opened a
            // second time: one file description means one write offset, so the two streams
            // interleave instead of overwriting each other.
            dup2(target_fd, 2)?;
        } else {
            return Err(ShellError::ExecutionError(format!(
                "{}: ambiguous redirect",
                shown(target_str)
            )));
        }
        Ok(())
    }
}

/// The `m` of a move, `n>&m-`.
fn moved_descriptor(target: &str) -> Option<RawFd> {
    target.strip_suffix('-')?.parse().ok()
}

fn bad_descriptor(named: &str) -> ShellError {
    ShellError::ExecutionError(format!("{}: Bad file descriptor", shown(named)))
}

/// The file a redirection opens, or `None` for the duplicating kinds, which open nothing.
fn open_target(
    env: &mut Environment,
    redir: &Redirection,
    target_str: &str,
) -> Result<Option<File>> {
    let failed = |e: std::io::Error| {
        ShellError::ExecutionError(format!("{}: {}", shown(target_str), reason(&e)))
    };
    let path = oslo_base::lossless::to_os(target_str);
    let file = match redir.kind {
        RedirectKind::Input => File::open(path).map_err(failed)?,
        // `>` and `>|` differ in exactly one situation, and only when `set -C` is on.
        RedirectKind::Output => open_for_output(target_str, env.noclobber())?,
        RedirectKind::Clobber => open_for_output(target_str, false)?,
        RedirectKind::Append => OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(failed)?,
        RedirectKind::ReadWrite => OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(failed)?,
        RedirectKind::DupInput | RedirectKind::DupOutput => return Ok(None),
        RedirectKind::Heredoc | RedirectKind::HeredocStrip => {
            let mut content = match &redir.heredoc_content {
                // Not `expand_word`: a body is one document, not an argument list, so field
                // splitting would collapse its whitespace and globbing would turn a line reading
                // `*` into a directory listing.
                Some(word) if redir.here_string => expand_word_to_string(env, word)?,
                Some(word) => crate::expand::join::expand_heredoc_to_string(env, word)?,
                None => String::new(),
            };
            // A here-string's newline goes on *after* expansion. `<<< "$(cmd)"` has to lose the
            // trailing newlines command substitution strips before this one is added.
            if redir.here_string {
                content.push('\n');
            }
            heredoc_body(&content)?
        }
    };
    Ok(Some(file))
}

/// Point `target_fd` at `file`, and make sure the descriptor survives `file` going out of scope.
///
/// `dup2` plus a drop looks like it works and does not. `open(2)` hands back the lowest free
/// descriptor, so in a shell that has never redirected anything `exec 3>log` opens the file *on
/// fd 3* — the very number the script asked for. `dup2(3, 3)` is then a POSIX no-op, the `File`
/// drops at the end of the statement, and fd 3 closes: `echo x >&3` on the next line reports
/// "Bad file descriptor". `{ echo x >&3; } 3>log` fails the same way; nothing about this is
/// specific to `exec`.
///
/// Taking the descriptor out of the `File` is what fixes it: after `into_raw_fd` nothing runs a
/// destructor, so this function owns the close and can order it after the `dup2` instead of
/// before. When `open` already landed on the wanted number there is nothing to duplicate at all.
///
/// `mem::forget` would have been the shorter spelling and the wrong one. Rust opens files
/// `O_CLOEXEC`, and the old code got child inheritance right only *because* `dup2` clears
/// `FD_CLOEXEC` on the descriptor it creates. In the no-op case there is no new descriptor, so a
/// forgotten `File` would keep the flag and hide fd 3 from every program the script `exec`s — the
/// same corpus case failing for the opposite reason. Clearing it explicitly says so rather than
/// depending on a side effect of a call that may not happen.
fn install(file: File, target_fd: RawFd) -> Result<()> {
    use std::os::fd::IntoRawFd;

    let raw = file.into_raw_fd();
    if raw != target_fd {
        // `raw` stays open across this call, so the descriptor is never unowned: a `close` first
        // would hand the number back to the kernel, and in a threaded process anything could
        // claim it before the `dup2` that was meant to reuse it.
        if let Err(e) = dup2(raw, target_fd) {
            let _ = nix::unistd::close(raw);
            return Err(e.into());
        }
        let _ = nix::unistd::close(raw);
    }
    // A redirection is inherited by the command it applies to, and for `exec` that command is a
    // program which replaces this process.
    fcntl(target_fd, FcntlArg::F_SETFD(FdFlag::empty()))?;
    Ok(())
}

/// What to call a redirection target that turned out not to be one filename.
///
/// bash names the *word* — `$f: ambiguous redirect` — and that is the useful half of the message:
/// the expansion of a target that expanded to nothing is nothing, so reporting it says
/// `: ambiguous redirect` and leaves the author no idea which name was empty. Only the single
/// variable reference is spelled back, because that is the shape this mistake almost always has;
/// anything else falls back to what it expanded to, which for several fields is informative.
fn name_of(target: &oslo_base::ast::Word, expanded: &[String]) -> String {
    use oslo_base::ast::WordPart;
    match target.parts.as_slice() {
        [WordPart::Variable { name, .. }] => format!("${}", name),
        _ => expanded.join(" "),
    }
}

/// Open the target of `>` or `>|`, honouring `set -C` when `refuse_existing` says to.
///
/// With noclobber off this is a plain create-or-truncate. With it on, `>` must not destroy a file
/// that is already there — the whole point of the option is that a mistyped `> notes` cannot eat
/// the file. `create_new` is what enforces it, rather than a `Path::exists` test followed by an
/// open: between the test and the open another process can create the file, and the check that
/// loses that race silently truncates exactly the data the option exists to protect.
///
/// Two things noclobber deliberately does *not* refuse:
///
/// * anything that is not a regular file. `cmd > /dev/null` and `cmd > /dev/tty` have nothing to
///   overwrite, and POSIX scopes the restriction to regular files for that reason.
/// * `>|`, which is the escape hatch — see the caller.
fn open_for_output(path: &str, refuse_existing: bool) -> Result<File> {
    // Opened by its real bytes, reported by its text; see `oslo_base::lossless`.
    let real = oslo_base::lossless::to_os(path);
    if !refuse_existing {
        return OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&real)
            .map_err(|e| ShellError::ExecutionError(format!("{}: {}", shown(path), reason(&e))));
    }

    match OpenOptions::new().write(true).create_new(true).open(&real) {
        Ok(file) => Ok(file),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // `symlink_metadata` is wrong here: `> link-to-dev-null` follows the link in every
            // shell, so the question is what the *target* is.
            let regular = std::fs::metadata(&real).is_ok_and(|m| m.is_file());
            if regular {
                return Err(ShellError::ExecutionError(format!(
                    "{}: cannot overwrite existing file",
                    shown(path)
                )));
            }
            OpenOptions::new()
                .write(true)
                .open(&real)
                .map_err(|e| ShellError::ExecutionError(format!("{}: {}", shown(path), reason(&e))))
        }
        Err(e) => Err(ShellError::ExecutionError(format!(
            "{}: {}",
            shown(path),
            reason(&e)
        ))),
    }
}

/// Stash whatever `target_fd` currently points at, out of the way of the script and of `exec`.
///
/// `dup(2)` is the obvious call and the wrong one twice over. It hands back the *lowest* free
/// descriptor — 3 or 4 in a fresh shell — which puts the shell's private copy of stdout squarely
/// in the range a script addresses by number, so `2>&3` inside a redirected group silently
/// succeeds where it should fail. And a plain `dup` never sets `FD_CLOEXEC`, so every child the
/// shell forks inherits those copies. `F_DUPFD_CLOEXEC` with a floor fixes both.
///
/// `None` means there was nothing to save: `target_fd` was already closed, which is a state the
/// restore path has to be able to reproduce.
pub(crate) fn save_fd(target_fd: RawFd) -> Option<RawFd> {
    fcntl(target_fd, FcntlArg::F_DUPFD_CLOEXEC(SAVE_FD_FLOOR)).ok()
}

impl Drop for RedirectGuard {
    fn drop(&mut self) {
        // Reverse order: `cmd >a >b` saved the pre-`a` state first, and unwinding b-then-a is what
        // puts the descriptor back where the command found it.
        for (target_fd, saved_fd) in self.saved_fds.drain(..).rev() {
            match saved_fd {
                Some(saved_fd) => {
                    let _ = dup2(saved_fd, target_fd);
                    let _ = nix::unistd::close(saved_fd);
                }
                // Nothing was open here before, so restoring means closing. Skipping this is how
                // `cmd 5>file` used to leak descriptor 5 into every command the shell ran next.
                None => {
                    let _ = nix::unistd::close(target_fd);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    /// On Linux `heredoc_body` always takes the memfd path, so the fallback is only ever reached
    /// on a kernel or sandbox this test suite does not run under. Exercise it directly.
    #[test]
    fn the_temp_file_fallback_is_readable_and_anonymous() {
        let mut file = unlinked_temp_file().expect("temp file");
        file.write_all(b"body").unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        let mut got = String::new();
        file.read_to_string(&mut got).unwrap();
        assert_eq!(got, "body");

        // Unlinked at creation, so it leaves nothing behind even if the shell is killed.
        assert_eq!(file.metadata().unwrap().nlink(), 0);
    }

    #[test]
    fn a_body_larger_than_a_pipe_buffer_is_fully_readable() {
        let content = "y".repeat(1 << 20);
        let mut file = heredoc_body(&content).expect("heredoc body");

        let mut got = String::new();
        file.read_to_string(&mut got).unwrap();
        assert_eq!(got.len(), content.len());
    }

    /// The descriptor a script can name is 0..9. Anything the shell keeps for itself has to sit
    /// above that, or `2>&3` inside a redirected group hits the shell's own copy of stdout.
    #[test]
    fn a_saved_copy_lands_out_of_the_range_a_script_can_name() {
        let saved = save_fd(1).expect("stdout is open");
        assert!(
            saved >= SAVE_FD_FLOOR,
            "saved copy landed on fd {saved}, inside the range a script addresses by number"
        );
        let _ = nix::unistd::close(saved);
    }

    /// Without `FD_CLOEXEC` every child the shell forks inherits a duplicate of the shell's
    /// original stdout and stderr, and can read or scribble on them behind the redirection.
    #[test]
    fn a_saved_copy_does_not_survive_exec() {
        use nix::fcntl::FdFlag;

        let saved = save_fd(1).expect("stdout is open");
        let flags = FdFlag::from_bits_truncate(fcntl(saved, FcntlArg::F_GETFD).expect("F_GETFD"));
        assert!(
            flags.contains(FdFlag::FD_CLOEXEC),
            "saved copy on fd {saved} is not close-on-exec"
        );
        let _ = nix::unistd::close(saved);
    }

    /// A descriptor that was closed before the redirection has no copy to restore, and leaving it
    /// open is not "no restore" — it is a leak that outlives the command (`cmd 5>file`).
    #[test]
    fn a_descriptor_with_no_saved_copy_is_closed_on_restore() {
        // A private descriptor, not one of the shell's: these tests share a process, and a test
        // that reached for a fixed number like 5 would race whatever else is running.
        let opened = save_fd(1).expect("stdout is open");

        let mut guard = RedirectGuard::new();
        guard.saved_fds.push((opened, None));
        drop(guard);

        assert_eq!(
            fcntl(opened, FcntlArg::F_GETFD),
            Err(nix::errno::Errno::EBADF),
            "fd {opened} was left open with nothing to restore it to"
        );
    }

    /// `set -C` is the difference between `>` and `>|`, and it applies to regular files only.
    #[test]
    fn noclobber_refuses_only_an_existing_regular_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fresh = dir.path().join("fresh");
        let fresh = fresh.to_str().unwrap();

        // Nothing there yet: `>` creates it even under noclobber.
        open_for_output(fresh, true).expect("creating a new file is always allowed");
        // Now it exists, so `>` must refuse and `>|` must not.
        let err = open_for_output(fresh, true).expect_err("noclobber must refuse");
        assert!(err.to_string().contains("cannot overwrite"), "{err}");
        open_for_output(fresh, false).expect("`>|` overrides noclobber");

        // Not a regular file, so there is nothing for the option to protect.
        open_for_output("/dev/null", true).expect("/dev/null is exempt");
    }

    /// The refusal must not be a read-then-open: the file has to be created by the same syscall
    /// that would have truncated it, or a racing creator's data is destroyed anyway.
    #[test]
    fn noclobber_truncates_nothing_when_it_refuses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data");
        std::fs::write(&path, b"payload").expect("write");
        let _ = open_for_output(path.to_str().unwrap(), true).expect_err("noclobber must refuse");
        assert_eq!(std::fs::read(&path).expect("read"), b"payload");
    }

    /// R11.C1, exactly. `open` returns the lowest free descriptor, so `3>file` in a clean shell
    /// gets a `File` that already *is* fd 3 — and `dup2(3, 3)` followed by a drop closes the
    /// redirection it just made. Handing `install` a file whose descriptor is the target is that
    /// case with the timing taken out of it.
    #[test]
    fn installing_a_file_onto_its_own_descriptor_leaves_it_open() {
        use nix::fcntl::FdFlag;

        let file = File::open("/dev/null").expect("/dev/null");
        let target = file.as_raw_fd();
        install(file, target).expect("install");

        let flags = fcntl(target, FcntlArg::F_GETFD)
            .expect("the descriptor the redirection installed was closed");
        // Not close-on-exec: `exec 3>log; exec prog` has to hand fd 3 to `prog`. The old code got
        // this right only as a side effect of `dup2`, which the no-op case never reached.
        assert!(
            !FdFlag::from_bits_truncate(flags).contains(FdFlag::FD_CLOEXEC),
            "fd {target} is close-on-exec, so an exec'd child cannot see it"
        );
        let _ = nix::unistd::close(target);
    }

    /// The ordinary case: the file has to be moved onto a descriptor it did not open on, and the
    /// content has to survive the move.
    #[test]
    fn installing_a_file_elsewhere_moves_its_contents() {
        use std::io::Read;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data");
        std::fs::write(&path, b"payload").expect("write");

        // A number this process is not using. `install` overwrites whatever is on the target, so
        // the placeholder does not have to be — and must not be — closed first: closing it would
        // hand the number back and let a sibling test claim it.
        let placeholder = File::open("/dev/null").expect("/dev/null");
        let target = fcntl(
            placeholder.as_raw_fd(),
            FcntlArg::F_DUPFD_CLOEXEC(SAVE_FD_FLOOR),
        )
        .expect("scratch descriptor");

        let file = File::open(&path).expect("open");
        assert_ne!(
            file.as_raw_fd(),
            target,
            "the two-descriptor case is what this covers"
        );
        install(file, target).expect("install");

        let mut installed = unsafe { <File as std::os::fd::FromRawFd>::from_raw_fd(target) };
        let mut got = String::new();
        installed.read_to_string(&mut got).expect("read");
        assert_eq!(
            got, "payload",
            "fd {target} is not the file that was installed"
        );
    }

    /// The pre-exec child never restores anything, so it must not spend syscalls building a save
    /// list — and must not leave copies of the shell's descriptors lying around either.
    #[test]
    fn the_exec_guard_saves_nothing() {
        let mut guard = RedirectGuard::for_exec();
        let mut env = Environment::new();
        guard.apply(&mut env, &[]).expect("no redirections");
        assert!(guard.saved_fds.is_empty());
    }
}
