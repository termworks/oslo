//! `exec` — replace the shell process, or make the current redirections permanent.
//!
//! Two builtins wearing one name, told apart by whether a command word follows the options:
//!
//! * `exec cmd args…` never returns. The shell's process image is replaced, so the descriptors,
//!   the pid and the parent's `wait` all belong to `cmd` afterwards.
//! * `exec > "$log" 2>&1` — no command word — applies its redirections to *the shell itself*,
//!   permanently. This is the standard "log everything from here on" prologue, and the one form
//!   whose implementation is not in this file: a builtin never sees its own redirections, so the
//!   permanence is decided by the caller, which asks [`makes_redirections_permanent`] whether to
//!   build a restoring [`crate::exec::redirect::RedirectGuard`] or a non-restoring one.

use crate::env::builtins::spawn::{NOT_EXECUTABLE, NOT_FOUND, exec_cstring, resolve_program};
use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::{Result, ShellError};
use std::ffi::CString;

/// `exec` with its options stripped off.
struct Invocation<'a> {
    /// `-c`: run the command with an empty environment.
    clear_env: bool,
    /// `-l`: pass argv\[0\] with a leading `-`, the convention a login shell looks for.
    login: bool,
    /// `-a name`: what to pass as argv\[0\] instead of the command word.
    argv0: Option<&'a str>,
    /// The command word and its arguments; empty for the redirection-only form.
    operands: &'a [String],
}

/// Split `exec`'s own options from the command it is being asked to run.
///
/// Option parsing stops at the first word that is not an option, so `exec ls -l` runs `ls` with
/// `-l` rather than trying to interpret `-l` as `exec`'s own login flag.
fn parse(args: &[String]) -> std::result::Result<Invocation<'_>, String> {
    let mut inv = Invocation {
        clear_env: false,
        login: false,
        argv0: None,
        operands: &[],
    };

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if arg.len() < 2 || !arg.starts_with('-') {
            break;
        }
        let mut chars = arg[1..].chars();
        while let Some(c) = chars.next() {
            match c {
                'c' => inv.clear_env = true,
                'l' => inv.login = true,
                'a' => {
                    // `-a` takes a value, either glued on (`-aname`) or as the next word.
                    let rest: String = chars.by_ref().collect();
                    if !rest.is_empty() {
                        inv.argv0 = Some(&arg[arg.len() - rest.len()..]);
                    } else {
                        i += 1;
                        match args.get(i) {
                            Some(name) => inv.argv0 = Some(name),
                            None => return Err("-a: option requires an argument".to_string()),
                        }
                    }
                }
                other => return Err(format!("-{}: invalid option", other)),
            }
        }
        i += 1;
    }

    inv.operands = &args[i.min(args.len())..];
    Ok(inv)
}

/// Whether this command is the form of `exec` whose redirections outlive it.
///
/// Called by the dispatcher *before* the redirections are applied, because the decision is which
/// kind of guard to build: with a command word the process is about to be replaced and nothing is
/// ever restored anyway, and without one POSIX says the redirections affect the current shell
/// from then on. Anything that fails to parse as `exec` options is not this form — the builtin
/// will report the bad option itself, and its redirections should behave normally.
pub fn makes_redirections_permanent(cmd_name: &str, words: &[String]) -> bool {
    cmd_name.trim() == "exec"
        && words.first().map(String::as_str) == Some("exec")
        && matches!(parse(words), Ok(inv) if inv.operands.is_empty())
}

/// Why `exec` could not run `name`.
///
/// **A name with a `/` in it was pointed at something in particular**, so the answer is the
/// system's: `exec /etc` said `not found` about a directory that is plainly there, and
/// `exec /var/empty/x` said it about a path whose parent is unreadable. Both are the shell
/// guessing when it could have asked. A bare word really was searched for and not found, and keeps
/// the wording — and the `exec: ` prefix — that bash gives it.
fn unavailable(name: &str) -> String {
    if !name.contains('/') {
        return format!("exec: {name}: not found");
    }
    let reason = match std::fs::metadata(name) {
        Err(e) => oslo_base::error::reason(&e),
        Ok(meta) if meta.is_dir() => "Is a directory".to_string(),
        // It is there and it is a file, so what stopped `resolve_program` was the execute bit.
        Ok(_) => "Permission denied".to_string(),
    };
    format!("{name}: {reason}")
}

/// `exec [-cl] [-a name] [command [args…]]`.
pub fn builtin_exec(_env: &mut Environment, args: &[String]) -> Result<i32> {
    let inv = match parse(args) {
        Ok(inv) => inv,
        Err(msg) => {
            eprintln!("{}exec: {}", origin_now(), msg);
            eprintln!("exec: usage: exec [-cl] [-a name] [command [arguments ...]]");
            return Ok(2);
        }
    };

    if inv.operands.is_empty() {
        // Redirection-only form. The redirections were applied before this builtin was entered
        // and — if the dispatcher honoured `makes_redirections_permanent` — will not be undone.
        return Ok(0);
    }

    let name = &inv.operands[0];
    let Some(program) = resolve_program(name) else {
        eprintln!("{}{}", origin_now(), unavailable(name));
        // POSIX: a non-interactive shell exits when `exec` cannot find its command. Signalling
        // the exit rather than returning a status is what stops the rest of the script running
        // with descriptors that were set up for a program that never started.
        return Err(ShellError::Exit(NOT_FOUND));
    };

    let mut argv0 = inv.argv0.unwrap_or(name.as_str()).to_string();
    if inv.login {
        argv0.insert(0, '-');
    }

    let c_path = exec_cstring(std::os::unix::ffi::OsStrExt::as_bytes(program.as_os_str()));
    let mut c_args = vec![exec_cstring(argv0.as_bytes())];
    c_args.extend(inv.operands[1..].iter().map(|a| exec_cstring(a.as_bytes())));

    // **The last chance anything has to be written down.** `exec` is an ordinary way out of an
    // interactive shell — `exec $SHELL` after editing a config is how most people restart one — but
    // it is the only one that does not return to the loop, so it never reaches the barrier in
    // `settle_stores`. Without this, whatever the writer thread still holds is replaced along with
    // the process image, and the session loses its tail. See `oslo_base::track::writer`.
    oslo_base::track::writer::settle();

    // The program replacing this one must not inherit the shell's signal policy: the REPL ignores
    // SIGTSTP and friends so job-control keystrokes cannot stop the shell, and an ignored
    // disposition survives `exec`.
    crate::exec::job::reset_signals_for_child();

    // Via the shared policy, so a script with no `#!` line is interpreted rather than refused:
    // `exec ./noshebang.sh` used to print `Exec format error` and end the shell.
    let empty: [CString; 0] = [];
    let failure = Some(super::spawn::exec_or_interpret(
        &c_path,
        &c_args,
        inv.clear_env.then_some(empty.as_slice()),
    ));

    // Only reachable when the exec failed; on success this process no longer exists.
    //
    // Named the same way [`unavailable`] names it, so `exec /etc` and `exec /etc/passwd` do not
    // disagree about whether a path gets an `exec: ` in front of it. bash draws the line in the
    // same place: the builtin puts its name on a *word* it searched for and failed to find, and
    // stays out of the way when the system is reporting on a path.
    let label = if name.contains('/') {
        name.to_string()
    } else {
        format!("exec: {name}")
    };
    eprintln!("{}{label}: {}", origin_now(), failure_text(failure));
    Err(ShellError::Exit(NOT_EXECUTABLE))
}

fn failure_text(err: Option<nix::errno::Errno>) -> String {
    match err {
        Some(e) => e.desc().to_string(),
        None => "exec returned".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{makes_redirections_permanent, parse};

    fn words(argv: &[&str]) -> Vec<String> {
        argv.iter().map(|s| s.to_string()).collect()
    }

    /// The whole-script redirect. Nothing follows the options, so the redirections belong to the
    /// shell from here on and the caller must not build a restoring guard.
    #[test]
    fn the_redirection_only_form_is_permanent() {
        assert!(makes_redirections_permanent("exec", &words(&["exec"])));
        assert!(makes_redirections_permanent(
            "exec",
            &words(&["exec", "-c"])
        ));
    }

    /// With a command word the process is replaced; the guard question does not arise, and
    /// answering "permanent" would leak the redirection if the exec failed in an interactive
    /// shell.
    #[test]
    fn the_replace_form_is_not_permanent() {
        assert!(!makes_redirections_permanent(
            "exec",
            &words(&["exec", "ls"])
        ));
        assert!(!makes_redirections_permanent(
            "exec",
            &words(&["exec", "-a", "sh", "bash"])
        ));
        // A different builtin that happens to be called with no arguments is not `exec`.
        assert!(!makes_redirections_permanent("true", &words(&["true"])));
    }

    #[test]
    fn options_stop_at_the_command_word() {
        let argv = words(&["exec", "-l", "ls", "-a", "-c"]);
        let inv = parse(&argv).expect("parses");
        assert!(inv.login);
        assert!(!inv.clear_env, "-c after the command word belongs to ls");
        assert_eq!(inv.operands, &argv[2..]);
    }

    #[test]
    fn argv0_is_accepted_glued_or_separate() {
        let argv = words(&["exec", "-abusybox", "sh"]);
        assert_eq!(parse(&argv).unwrap().argv0, Some("busybox"));
        let argv = words(&["exec", "-a", "busybox", "sh"]);
        let inv = parse(&argv).unwrap();
        assert_eq!(inv.argv0, Some("busybox"));
        assert_eq!(inv.operands.len(), 1);
    }

    #[test]
    fn a_double_dash_ends_the_options() {
        let argv = words(&["exec", "--", "-weird-name"]);
        let inv = parse(&argv).expect("parses");
        assert_eq!(inv.operands, &argv[2..]);
    }

    #[test]
    fn an_unknown_option_is_reported() {
        assert!(parse(&words(&["exec", "-Z"])).is_err());
        assert!(parse(&words(&["exec", "-a"])).is_err());
    }

    /// **`exec` settles the history writer first, and a forked child has no writer to settle.**
    ///
    /// Only the forking thread survives `fork`, so the `oslo-history` loop is gone in the child —
    /// but the `Receiver` it was looping over is still in the inherited memory, so `send` succeeds
    /// and the job is queued into something nothing will ever drain. `settle` then blocked for the
    /// life of the process, and `exec` calls it unconditionally: `( exec /bin/true )` at an
    /// interactive prompt never returned, and the parent blocked behind it in `waitpid`. Measured
    /// before the fix: `rc=137` with nothing after it running; after: `rc=0`.
    #[test]
    fn settling_the_writer_does_not_hang_a_forked_child() {
        use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
        use nix::unistd::{ForkResult, fork};

        // Start the writer thread here, so the child inherits a queue nothing is draining.
        oslo_base::track::writer::defer(|| {});
        oslo_base::track::writer::settle();

        // SAFETY: the child calls `settle` and `_exit` and nothing else — no allocation across the
        // fork, and it never returns into the test harness.
        match unsafe { fork() }.expect("fork") {
            ForkResult::Child => {
                oslo_base::track::writer::settle();
                unsafe { nix::libc::_exit(7) };
            }
            ForkResult::Parent { child } => {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    match waitpid(child, Some(WaitPidFlag::WNOHANG)) {
                        Ok(WaitStatus::Exited(_, code)) => {
                            assert_eq!(code, 7, "the child got past settle");
                            return;
                        }
                        Ok(_) if std::time::Instant::now() < deadline => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Ok(_) => {
                            let _ =
                                nix::sys::signal::kill(child, nix::sys::signal::Signal::SIGKILL);
                            let _ = waitpid(child, None);
                            panic!("still inside settle after five seconds");
                        }
                        Err(e) => panic!("waitpid: {e}"),
                    }
                }
            }
        }
    }
}
