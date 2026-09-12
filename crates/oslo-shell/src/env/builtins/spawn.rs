//! Running an external program from inside a builtin.
//!
//! `command` and `exec` both have to reach a binary on `PATH` without going back through
//! [`crate::exec::simple`], whose whole job is the lookup order those two builtins exist to
//! bypass. The fork/exec mechanics are the same either way, so they live here once.
//!
//! Redirections are *not* applied here. A builtin runs with the shell's descriptors already
//! pointing where the command's redirections say (`crate::exec::redirect::RedirectGuard` is
//! applied by the caller before the builtin is entered), and a forked child inherits them.

use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, fork};
use oslo_base::error::{Result, ShellError};
use std::ffi::CString;
use std::path::{Path, PathBuf};

/// Turn user-controlled bytes into an argv entry, dropping any NUL.
///
/// argv entries are NUL-terminated, so an embedded NUL cannot reach `execv` under any encoding.
/// Dropping it reproduces the argument bash would have built; the one thing this must never do is
/// panic, which is what a `CString::new(..).unwrap()` on shell data would eventually do.
pub fn exec_cstring(bytes: &[u8]) -> CString {
    let stripped: Vec<u8> = bytes.iter().copied().filter(|b| *b != 0).collect();
    CString::new(stripped).unwrap_or_default()
}

/// `execv` the program, and if the kernel says it is not a binary, run it as a shell script.
///
/// Returns only when the program could not be started at all; on success this process is gone.
///
/// **`ENOEXEC` means "this is not a binary", not "this cannot run".** A file the kernel will not
/// exec — no `#!` line, or one naming an interpreter that is not there — is run by the shell
/// itself, with the path as `$0`. POSIX requires it, and bash, dash and zsh all do it.
///
/// One copy for the whole crate, because there were two exec sites and only one of them did this:
/// `./noshebang.sh` worked while `command ./noshebang.sh` reported "cannot execute" and
/// `exec ./noshebang.sh` killed the shell with `Exec format error`.
///
/// `environment` is `Some` only for `exec -c`, which starts the program with an empty one; the
/// re-exec has to clear it too, or a shebang-less script would be the one command `-c` did not
/// apply to.
///
/// Answers the errno of the attempt that failed, for a caller that reports it.
pub fn exec_or_interpret(
    c_path: &CString,
    c_args: &[CString],
    environment: Option<&[CString]>,
) -> nix::errno::Errno {
    let start = |path: &CString, args: &[CString]| match environment {
        Some(vars) => nix::unistd::execve(path, args, vars).err(),
        None => nix::unistd::execv(path, args).err(),
    };
    let failed = start(c_path, c_args).unwrap_or(nix::errno::Errno::UnknownErrno);
    if failed != nix::errno::Errno::ENOEXEC {
        return failed;
    }
    // Re-exec rather than interpret in place: the caller has already applied redirections and
    // joined the foreground group, and a fresh shell inherits both. Interpreting here would mean
    // running a script inside a process that is halfway through becoming something else.
    //
    // The magic link, not the resolved name: an install replaces the running binary and the name
    // then cannot be executed. See `oslo_base::exe`.
    let shell = oslo_base::exe::path();
    let c_shell = exec_cstring(std::os::unix::ffi::OsStrExt::as_bytes(shell.as_os_str()));
    // `argv[0]` is the shell, then the script, then whatever the caller passed after the command
    // name — so `$0` inside the script is the path it was invoked by, as a `#!` line would give it.
    let mut argv = vec![c_shell.clone(), c_path.clone()];
    argv.extend(c_args.iter().skip(1).cloned());
    start(&c_shell, &argv).unwrap_or(nix::errno::Errno::UnknownErrno)
}

/// Resolve a command word to a program on disk the way the shell does.
///
/// A word containing a slash is a path and is used as given — `PATH` is not consulted, and a
/// relative path stays relative to the working directory. Anything else is searched for on `PATH`.
pub fn resolve_program(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = Path::new(name);
        return path.is_file().then(|| path.to_path_buf());
    }
    // A name this directory hides is not on `$PATH` as far as anything asking here is concerned.
    // See `oslo_base::command`. This is the lowest of the `$PATH` searches — `command -v`, `which`
    // and the execution fallback all arrive here — and it has to agree with `type`, which reads the
    // same mask in `control::resolve::path_matches`.
    if oslo_base::command::hidden(name) {
        return None;
    }
    // **oslo does not see its own copies.** `macros::bin` writes every stored script into a
    // directory on `$PATH` so that bash, tmux and a `.desktop` file can run one; oslo has the
    // database and needs no copy, so its own files are passed over and the macro is answered from
    // the database instead — after the rest of `$PATH`, which is where a stored macro belongs.
    //
    // **Passed over, not stopped at.** Rejecting the one path `which` came back with was the first
    // attempt and it is wrong in a way that matters: it ends the search, so a stored `date` with a
    // copy early on `$PATH` beat `/usr/bin/date` — exactly the shadowing this whole design promises
    // cannot happen. Every candidate is walked in `$PATH` order and the first that is not ours is
    // the answer, which is also what makes a file somebody put in that directory by hand behave
    // like a file anywhere else.
    which::which_all(name)
        .ok()?
        .find(|path| !oslo_base::macros::bin::is_ours(path))
}

/// The status a shell reports when a command word could not be run at all.
///
/// 127 for "no such command", 126 for "found it and could not execute it" — the split every
/// script's `if [ $? -eq 127 ]` depends on.
pub const NOT_FOUND: i32 = 127;
pub const NOT_EXECUTABLE: i32 = 126;

/// Fork, exec `program` with `argv`, and wait for it.
///
/// `argv` is the full argument vector including argv\[0\]; the caller decides what argv\[0\] is,
/// which is what lets `command foo` keep reporting itself as `foo`.
pub fn run_external(program: &Path, argv: &[String], display_name: &str) -> Result<i32> {
    let c_path = exec_cstring(std::os::unix::ffi::OsStrExt::as_bytes(program.as_os_str()));
    let c_args: Vec<CString> = argv
        .iter()
        .map(|a| exec_cstring(&oslo_base::lossless::decode(a)))
        .collect();

    // SAFETY: the child touches only async-signal-safe calls (`sigaction`, `sigprocmask`,
    // `execv`, `write` via `eprintln`, `_exit`) before it replaces itself. oslo is single
    // threaded, so no other thread's lock can be inherited half-held.
    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            crate::exec::job::reset_signals_for_child();
            exec_or_interpret(&c_path, &c_args, None);
            crate::env::complain(
                argv,
                display_name,
                &format!("{display_name}: cannot execute"),
                "cannot be run",
                Some(
                    "the file is there but the kernel refused it: check the mode bits and the interpreter on its first line",
                ),
            );
            std::process::exit(NOT_EXECUTABLE);
        }
        Ok(ForkResult::Parent { child }) => Ok(wait_for_child(child)),
        Err(e) => Err(ShellError::ExecutionError(format!("Fork failed: {}", e))),
    }
}

/// Wait for a foreground child and turn its wait status into an exit status.
///
/// Mirrors [`crate::exec::simple`]: a signal death is `128 + signo`, a stop is reported rather
/// than waited on forever, and `EINTR` — a trapped signal arriving mid-wait — says nothing about
/// how the command ended, so it is retried.
fn wait_for_child(child: Pid) -> i32 {
    loop {
        match waitpid(child, Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Exited(_, code)) => return code,
            Ok(WaitStatus::Signaled(_, sig, _)) => return 128 + sig as i32,
            Ok(WaitStatus::Stopped(_, sig)) => return 128 + sig as i32,
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => return 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{exec_cstring, resolve_program};
    use std::ffi::CString;

    #[test]
    fn an_embedded_nul_is_dropped_not_fatal() {
        assert_eq!(exec_cstring(b"a\0b"), CString::new("ab").unwrap());
        assert_eq!(exec_cstring(b"plain"), CString::new("plain").unwrap());
    }

    /// A word with a slash is a path, not a `PATH` search — `./ls` must never find `/bin/ls`.
    #[test]
    fn a_slash_bearing_word_is_taken_as_a_path() {
        assert_eq!(
            resolve_program("/bin/sh").as_deref(),
            Some("/bin/sh".as_ref())
        );
        assert_eq!(resolve_program("./definitely-not-here-xyz"), None);
    }

    #[test]
    fn a_bare_word_is_searched_for_on_path() {
        assert!(resolve_program("sh").is_some());
        assert_eq!(resolve_program("definitely-not-a-command-xyz"), None);
    }
}
