//! Finding and running an external program.
//!
//! Split out of [`super`] so the dispatcher above stays about *order* — alias, function,
//! builtin, PATH — and this file is about the last of those: where a command word points, what
//! status it leaves behind when it points at something unrunnable, and the fork/exec/wait that
//! runs it when it does.

use crate::env::Environment;
use crate::exec::job;
use crate::exec::redirect::RedirectGuard;
use crate::exec::simple::report_redirect_failure;
use nix::sys::signal::Signal;
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, fork};
use oslo_base::ast::Redirection;
use oslo_base::error::{Result, ShellError};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// What a command word resolved to.
///
/// The three failure cases are kept apart because a shell reports them with different exit
/// statuses, and callers of a script read those numbers: 127 means "no such command, maybe a
/// typo or a missing package", 126 means "it is there and I could not run it". oslo used to
/// collapse both onto 127 — and, for a directory, onto a silent `cd` with status 0 (PLAN R5.13).
pub(crate) enum Lookup {
    /// An executable file, already resolved to a path.
    Program(PathBuf),
    /// The word names a directory. Status 126, unless autocd is on.
    Directory,
    /// The word names something that exists and is not executable. Status 126.
    NotExecutable,
    /// Nothing of that name exists on PATH. Status 127.
    NotFound,
}

/// Resolve a command word the way `execvp` would.
///
/// A word containing a slash is a *path*, not a PATH search — POSIX 2.9.1.1 — so it is stat'd
/// directly and its failure mode reported precisely. Only a bare word goes through PATH, where
/// anything that is not an executable file is simply skipped and the word is "not found",
/// exactly as bash reports it.
///
/// The bare-word search goes through [`hash_lookup`](crate::env::builtins::hash_lookup) rather
/// than `which` directly: that is what fills the `hash` table, so `ls; hash` lists `ls` the way
/// bash does. Nothing else changes — a hit is a path `which` would have returned, and the table
/// is dropped whenever `PATH` is assigned or `hash -r` runs.
pub(crate) fn look_up_command(name: &str) -> Lookup {
    if name.contains('/') {
        return classify_path(Path::new(name));
    }
    match crate::env::builtins::hash_lookup(name) {
        Some(path) => Lookup::Program(path),
        None => Lookup::NotFound,
    }
}

/// Classify a path operand: does it exist, is it a directory, may this process execute it?
fn classify_path(path: &Path) -> Lookup {
    // `metadata` follows symlinks, as `execve` does: a symlink to an executable is executable,
    // and a dangling one is "not found" rather than "not executable".
    match std::fs::metadata(path) {
        Err(_) => Lookup::NotFound,
        Ok(md) if md.is_dir() => Lookup::Directory,
        // `access(2)` rather than the mode bits, for the same reason `test -x` uses it: the mode
        // alone answers wrongly for root, for a `noexec` mount, and whenever an ACL is involved.
        Ok(_) if nix::unistd::access(path, nix::unistd::AccessFlags::X_OK).is_ok() => {
            Lookup::Program(path.to_path_buf())
        }
        Ok(_) => Lookup::NotExecutable,
    }
}

/// Fork, apply redirections in the child, and exec `path`.
pub(crate) fn run_external(
    env: &mut Environment,
    path: &Path,
    cmd_name: &str,
    words: &[String],
    redirections: &[Redirection],
) -> Result<i32> {
    // Both conversions take the raw bytes: a resolved path is an `OsStr`, not necessarily UTF-8
    // (a PATH entry can be any byte string), and `to_str().unwrap()` aborted the shell on one.
    let c_path = exec_cstring(path.as_os_str().as_bytes());
    // The bytes a glob read, not their UTF-8 stand-ins: see `oslo_base::lossless`.
    let c_args: Vec<CString> = words
        .iter()
        .map(|w| exec_cstring(&oslo_base::lossless::decode(w)))
        .collect();

    unsafe {
        match fork() {
            Ok(ForkResult::Child) => {
                // R7.1: a foreground command is a job of its own, so it leads its own process
                // group. Done first: the parent is about to hand the terminal to that group, and
                // until this call lands the child is a background process as far as the tty
                // driver is concerned.
                job::join_foreground_group(None);
                // Before anything else, and in particular before `execv`: the program about to
                // replace this process must not inherit the shell's own signal policy.
                crate::exec::job::reset_signals_for_child();

                // Nothing in this process will ever restore a descriptor — `execv` replaces the
                // whole table — so the guard saves no copies for the new program to inherit.
                let mut guard = RedirectGuard::for_exec();
                if let Err(e) = guard.apply(env, redirections) {
                    std::process::exit(report_redirect_failure(&env.origin(), &e));
                }

                // Including the shell-script fallback for a file with no `#!` line; see there.
                crate::env::builtins::spawn::exec_or_interpret(&c_path, &c_args, None);

                eprintln!("oslo: exec failed for {}", cmd_name);
                std::process::exit(126);
            }
            Ok(ForkResult::Parent { child }) => {
                // `None` when job control is off, which is when the child is still in the
                // shell's own group and there is no terminal to hand anywhere.
                let pgid = job::place_foreground_child(child, None);
                // R7.1: while the job runs, it owns the terminal — that is what makes Ctrl-C
                // reach it instead of the shell, and what lets it read from the tty at all.
                // Only when a config asked for it, and `0` — the default — means the sentinel is
                // never even forked. See [`job::sentinel`].
                let escape = oslo_ui::settings::current().misc.interrupt_escape;
                let watching = escape.after > 0;
                if let Some(pgid) = pgid {
                    job::give_terminal_to(pgid);
                    // And a watcher inside that group, because the shell is now outside it and
                    // will not see a Ctrl-C at all.
                    if watching {
                        job::watch(job::Orders {
                            pgid: pgid.as_raw(),
                            after: escape.after as u32,
                            signal: escape_signal(escape.action),
                            notify: escape.notify,
                        });
                    }
                }
                let status = wait_for_child(child, cmd_name, words);
                if pgid.is_some() && watching {
                    job::stand_down();
                    // Whatever the watcher did, said in the shell's own voice — and a hook, so a
                    // config can act on it. Drained here because the shell was inside `waitpid`
                    // for the whole time the watcher was awake.
                    report_escalations(cmd_name, words);
                }
                // Taken back with SIGTTOU blocked: at this moment the shell is not the foreground
                // group, so an unguarded `tcsetpgrp` would stop the shell itself.
                //
                // A status under 128 is a program that exited on its own, so whatever it did to the
                // terminal it meant — `stty` is exactly such a program. Above that it was killed or
                // stopped and had no chance to tidy.
                job::reclaim_terminal(job::left_it_deliberately(status));
                Ok(status)
            }
            Err(e) => Err(ShellError::ExecutionError(format!("Fork failed: {}", e))),
        }
    }
}

/// Wait for a foreground child and turn its wait status into an exit status.
///
/// `WUNTRACED` is not optional now that children are started with SIGTSTP at `SIG_DFL`
/// ([`crate::exec::job::reset_signals_for_child`]). Ctrl-Z stops the child, and a plain
/// `waitpid` — which only reports *termination* — would then block the shell forever on a
/// process nobody can resume, turning a suspend into a hang. Reporting the stop instead returns
/// the prompt; the process stays stopped until job control (`fg`/`bg`) can adopt it.
///
/// `EINTR` is retried rather than reported: a trapped signal arriving mid-wait says nothing
/// about how the command ended.
///
/// A non-interactive bash *does* block on a stopped child — `bash -c 'sh -c "kill -STOP $$"'`
/// never returns — so this is a deliberate divergence from the oracle, in the one direction
/// where the oracle's behaviour is indistinguishable from a deadlock.
fn wait_for_child(child: Pid, cmd_name: &str, words: &[String]) -> i32 {
    loop {
        match waitpid(child, Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Exited(_, code)) => {
                crate::exec::argv::note_signal(None);
                return code;
            }
            // A shell reports a signal death as 128 + the signal number, which is how `$?` tells
            // `kill -9` (137) apart from an exit status of 9.
            Ok(WaitStatus::Signaled(_, sig, _)) => {
                // Recorded, not just folded into the status: `128 + n` cannot tell a program
                // killed by signal `n` from one that called `exit(128 + n)`, and a caller that
                // captured this command's output is about to be asked which it was.
                crate::exec::argv::note_signal(Some(sig as i32));
                // **R7.2: the Ctrl-C the shell never saw.** The terminal sends SIGINT to the
                // foreground *child*, and a shell waiting on one is not in that group — so the
                // interrupt machinery hears nothing, and the only evidence that a key was pressed
                // is this wait status. Without noting it, `while true; do sleep 1; done` runs
                // forever under a keyboard full of `^C`, and `sleep 5; echo hi` still prints,
                // because the next command boundary has nothing to poll.
                //
                // **Interactive only, and that is the whole of the difference.** A script shares
                // its process group with the terminal, so a real Ctrl-C reaches the *shell* too
                // and its own handler sees it — there is nothing to infer. Inferring anyway meant
                // a child that merely died by SIGINT of its own accord, `kill -INT $$` or a
                // program re-raising it after cleanup, silently abandoned the rest of the script:
                //
                // ```text
                // echo before                    bash, dash: before / after=130 / rc=0
                // sh -c 'kill -INT $$'           oslo, before this: before / rc=130
                // echo "after=$?"
                // ```
                if matches!(sig, Signal::SIGINT | Signal::SIGQUIT)
                    && crate::exec::pipeline::is_interactive()
                {
                    job::note_interrupt();
                }
                return 128 + sig as i32;
            }
            Ok(WaitStatus::Stopped(_, sig)) => {
                remember_stopped(child, cmd_name, words);
                return 128 + sig as i32;
            }
            Ok(_) => continue,
            // A SIGTERM or SIGHUP with an EXIT trap set ends the *wait*, not just this call: the
            // shell is dying, and sitting out the rest of a `sleep 20` first would leave the
            // cleanup that long undone. The signal stands for the command boundary just after
            // this to act on — see `job::fatal_signal_waiting`.
            Err(nix::errno::Errno::EINTR) => match crate::exec::job::fatal_signal_waiting() {
                Some(signum) => return 128 + signum,
                None => continue,
            },
            Err(_) => return 1,
        }
    }
}

/// Park a suspended foreground command in the job table so `fg` and `bg` can reach it.
///
/// R7.2: this is where [`crate::exec::job::JobState::Stopped`] comes from for a single command.
/// Without a table entry the process stayed stopped with nothing in the shell able to name it,
/// which made Ctrl-Z a way of leaking a process rather than parking one.
///
/// The notice is bash's `[1]+  Stopped   cmd`; the older `oslo: cmd: stopped (SIGTSTP)` wording is
/// kept for a shell without job control, where there is no job number to quote and no `fg` that
/// could act on it.
fn remember_stopped(child: Pid, cmd_name: &str, words: &[String]) {
    if !job::job_control_active() {
        eprintln!("oslo: {}: stopped", cmd_name);
        return;
    }
    let label = words.join(" ");
    let line = job::with_jobs(|jobs| {
        let id = jobs.add_stopped(child, vec![child], label);
        // Reported here, so the reaper must not repeat it at the next command boundary.
        if let Some(entry) = jobs.get_mut(id) {
            entry.notified = true;
        }
        jobs.get(id).map(|entry| job::describe(entry, '+'))
    });
    if let Some(line) = line {
        eprintln!("{}", line);
    }
}

/// The signal a configured escape action sends.
///
/// Here rather than on the settings type because a signal number is the shell's vocabulary, not a
/// configuration one — `oslo-ui` has no business knowing what `SIGSTOP` is.
fn escape_signal(action: oslo_ui::settings::EscapeAction) -> i32 {
    use oslo_ui::settings::EscapeAction;
    match action {
        EscapeAction::Stop => nix::libc::SIGSTOP,
        EscapeAction::Kill => nix::libc::SIGKILL,
        EscapeAction::Hup => nix::libc::SIGHUP,
        EscapeAction::Quit => nix::libc::SIGQUIT,
    }
}

/// Say what the watcher did, and let a config act on it.
///
/// **The shell says it, not the watcher.** A stopped job otherwise looks exactly like one somebody
/// suspended with Ctrl-Z, and "I stopped this for you because you asked three times" is a different
/// sentence from "you pressed Ctrl-Z". The watcher reports the fact; the wording and the hook
/// belong here, where there is an allocator and a hook table.
fn report_escalations(cmd_name: &str, words: &[String]) {
    for event in job::take_events() {
        let action = match event.signal {
            s if s == nix::libc::SIGSTOP => "stopped",
            s if s == nix::libc::SIGKILL => "killed",
            s if s == nix::libc::SIGHUP => "hung up",
            s if s == nix::libc::SIGQUIT => "quit",
            _ => "signalled",
        };
        eprintln!(
            "oslo: {cmd_name}: {action} after {} interrupts",
            event.presses
        );
        oslo_base::hooks::fire_at_here(
            oslo_base::hooks::at::JOB_ESCALATED,
            &[
                ("pgid", &event.pgid.to_string()),
                ("signal", &event.signal.to_string()),
                ("action", action),
                ("presses", &event.presses.to_string()),
                ("text", &words.join(" ")),
            ],
        );
    }
}

/// Turn user-controlled bytes into an argv entry for `execv`, dropping any NUL bytes.
///
/// argv entries are NUL-terminated, so an embedded NUL cannot reach `execv` under any encoding —
/// the only question is what the shell does about it. bash keeps NULs out of shell strings in the
/// first place (command substitution and `read` both drop them), so dropping here reproduces the
/// argument bash would have built. What it must never do is what the previous
/// `CString::new(..).unwrap()` did: kill the whole shell, exit 101, over one byte of input data.
fn exec_cstring(bytes: &[u8]) -> CString {
    let stripped: Vec<u8> = bytes.iter().copied().filter(|b| *b != 0).collect();
    // NUL-free by construction, so this cannot fail; the fallback keeps the exec path free of
    // panicking calls even if that ever stops being true.
    CString::new(stripped).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{Lookup, exec_cstring, look_up_command};
    use std::ffi::{CString, OsStr};
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn plain_argument_is_unchanged() {
        assert_eq!(exec_cstring(b"hello"), CString::new("hello").unwrap());
        assert_eq!(exec_cstring(b""), CString::new("").unwrap());
    }

    #[test]
    fn embedded_nul_is_dropped_not_fatal() {
        assert_eq!(exec_cstring(b"a\0b"), CString::new("ab").unwrap());
        assert_eq!(exec_cstring(b"\0\0"), CString::new("").unwrap());
        assert_eq!(exec_cstring(b"a\0"), CString::new("a").unwrap());
        assert_eq!(exec_cstring(b"\0a"), CString::new("a").unwrap());
    }

    /// A PATH entry — and therefore a resolved binary path — need not be UTF-8.
    #[test]
    fn non_utf8_path_survives() {
        let raw = b"/b\xffn/echo";
        assert_eq!(
            exec_cstring(OsStr::from_bytes(raw).as_bytes()).as_bytes(),
            raw
        );
    }

    /// The four outcomes a command word can have. `/tmp` and `/etc/hosts` stand in for "a
    /// directory" and "a file nobody may execute"; both exist on every unix oslo targets.
    #[test]
    fn path_operands_are_classified_by_why_they_cannot_run() {
        assert!(matches!(look_up_command("/bin/sh"), Lookup::Program(_)));
        assert!(matches!(look_up_command("/tmp"), Lookup::Directory));
        assert!(matches!(
            look_up_command("/etc/hosts"),
            Lookup::NotExecutable
        ));
        assert!(matches!(
            look_up_command("/nonexistent/oslo-test"),
            Lookup::NotFound
        ));
    }

    /// A bare word is a PATH search, so a directory *named* like one is simply not found —
    /// which is what makes autocd a separate decision rather than a lookup result.
    #[test]
    fn a_bare_word_never_resolves_to_a_directory() {
        assert!(matches!(look_up_command("sh"), Lookup::Program(_)));
        assert!(matches!(
            look_up_command("oslo-no-such-command"),
            Lookup::NotFound
        ));
    }
}
