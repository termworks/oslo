//! Running a stored function or script — the one that never touches the disk.
//!
//! # Found after `$PATH`, never before it
//!
//! Exactly the rule `super::simple::autoload` states for `functions/*.sh`, and for its reason: a
//! shell that promises a script sees POSIX behaviour cannot have a *database row* quietly redefining
//! `test`. So this is reached only once the `$PATH` search has already failed, which also means it
//! costs a database open on a line that was going to fail anyway and nothing at all otherwise.
//!
//! It is the opposite of what a dotfiles `bin/` does, and deliberately.
//!
//! # A script with no file
//!
//! A script needs a path for the kernel to `exec`, and a database row is not one. Writing a
//! temporary file is the obvious answer and the wrong one: a write per run, rubbish to clean up, and
//! your script sitting in a world-readable directory where it can be read or raced.
//!
//! `memfd_create(2)` makes a file that exists only in memory, and `/proc/self/fd/N` is a path the
//! kernel honours — shebang and all. Measured on Linux before this was written:
//!
//! ```text
//! #!/bin/sh              from a memfd   → ran
//! #!/usr/bin/env python3 from a memfd   → ran
//! readable by another user?             → no. `/memfd:name (deleted)`, reachable only through
//!                                         this process's own /proc/PID/fd
//! ```
//!
//! # What `$0` is, and the one case where it can be right
//!
//! The kernel rewrites `argv` for a shebang, so a script started this way sees `$0` as
//! `/proc/self/fd/3` rather than its own name. For a **shell** interpreter that is repairable,
//! because `sh -c` takes the name as its next argument:
//!
//! ```text
//! sh -c '. /proc/self/fd/3 "$@"' deploy alpha beta   →  $0 is deploy, "$@" is alpha beta
//! ```
//!
//! For any other interpreter the name goes in `$OSLO_SCRIPT` and `$0` stays the fd path. That is
//! honest; writing a file named after the script to make one variable prettier is not.
//!
//! A name is not a path, so a script that hands `$0` to the `argc` program is given an `argc`
//! function that routes that one call to oslo — see `argc_prelude`.

use crate::env::Environment;
use oslo_base::macros::{self, Kind};
use std::os::fd::{AsRawFd, OwnedFd};

/// Run the stored function or script `words[0]` names, with the redirections applied.
///
/// `None` when nothing is stored under that name, which is what lets the caller carry on to
/// "command not found".
pub(super) fn try_call(
    env: &mut Environment,
    name: &str,
    words: &[String],
    redirections: &[oslo_base::ast::Redirection],
) -> Option<oslo_base::error::Result<i32>> {
    let entry = lookup(name)?;

    // The same guard a function call uses, and for its reason: a redirection that cannot be set up
    // fails the command rather than running it with the shell's own streams.
    let mut guard = crate::exec::redirect::RedirectGuard::new();
    if guard.apply(env, redirections).is_err() {
        return Some(Ok(1));
    }
    let args: Vec<String> = words.iter().skip(1).cloned().collect();
    Some(Ok(match entry.kind {
        Kind::Func => function(env, &entry.body, name, &args),
        _ => script(&entry.body, name, &args),
    }))
}

/// What would run under `name` once the `$PATH` search has failed, or `None` when nothing would.
///
/// The whole rule, in the one place that can be asked without running anything: `type` and
/// `command -v` exist to answer "what would run?", and a `type` that disagrees with dispatch is
/// worse than no `type` at all.
fn lookup(name: &str) -> Option<macros::Entry> {
    // Nothing is stored at all in the overwhelming case, and finding that out must not cost a
    // database open on every failed command. Whether the file exists is what a `stat(2)` answers.
    if !macros::database().is_some_and(|path| path.exists()) {
        return None;
    }
    let store = macros::open().ok()?;
    let entry = macros::get(&store, Kind::Func, name)
        .or_else(|| macros::get(&store, Kind::Script, name))
        // **Off is off here too**, and it is read at the moment of the call rather than applied at
        // startup — which is the whole reason a function and a script need no snapshot: they are
        // looked up when you use them, so the answer is never stale.
        .filter(|entry| entry.active)?;
    drop(store);
    if macros::live::session::is_off(&oslo_base::track::session::id(), name) {
        return None;
    }
    Some(entry)
}

/// The kind of stored macro `name` would run as — what `type` and `command -v` report.
pub fn kind_of(name: &str) -> Option<Kind> {
    lookup(name).map(|entry| entry.kind)
}

/// Run the stored macro `name` with `args`, whatever this shell was asked to do otherwise.
///
/// **The door for everything that is not oslo.** A stored script is reachable by name only from a
/// shell that can read the database, so a `bash` script, a `tmux` command or a `.desktop` file that
/// wants one has no way in — and scripts call each other constantly. `oslo macros run NAME …` is
/// that way in, and it is why the database can be the only copy rather than the second one.
///
/// `None` when nothing is stored under the name, which is what lets the caller report it.
pub fn run_named(env: &mut Environment, name: &str, args: &[String]) -> Option<i32> {
    let store = macros::open().ok()?;
    let entry = macros::get(&store, Kind::Func, name)
        .or_else(|| macros::get(&store, Kind::Script, name))
        .filter(|entry| entry.active)?;
    drop(store);
    Some(match entry.kind {
        Kind::Func => function(env, &entry.body, name, args),
        _ => script(&entry.body, name, args),
    })
}

/// A stored function: the shell's own, run in this shell.
///
/// **In-process, not exec'd.** A function is expected to be able to `cd`, set a variable and change
/// the shell it was called from; running it in a child would make it a script that happens to have
/// no file. The positional parameters are set to `args` and put back afterwards.
fn function(env: &mut Environment, body: &str, name: &str, args: &[String]) -> i32 {
    // `swap_positional` rather than read-then-write: it is what a function call already uses, and
    // both halves are moves rather than a copy of every parameter twice.
    let saved = env.swap_positional(args.to_vec());
    let saved_name = std::mem::replace(&mut env.shell_name, name.to_string());

    let status = match crate::syntax::parse_bash_script(body) {
        Ok(program) => match crate::exec::eval_command_list(env, &program) {
            Ok(status) => status,
            Err(error) => {
                if let Some(code) = error.control_flow_status() {
                    code
                } else {
                    eprintln!("oslo: {name}: {error}");
                    1
                }
            }
        },
        Err(error) => {
            eprintln!("oslo: {name}: {error}");
            2
        }
    };

    env.swap_positional(saved);
    env.shell_name = saved_name;
    status
}

/// A stored script: its own program, run from memory.
fn script(body: &str, name: &str, args: &[String]) -> i32 {
    let Some(fd) = memory_file(name, body.as_bytes()) else {
        eprintln!("oslo: {name}: cannot hold the script in memory");
        return 126;
    };
    let path = format!("/proc/self/fd/{}", fd.as_raw_fd());

    // A shell interpreter gets its `$0` back; see the module docs.
    let shell = macros::shebang_interpreter(body).filter(|interp| {
        matches!(
            interp.as_str(),
            "sh" | "bash" | "dash" | "ksh" | "zsh" | "oslo"
        )
    });

    let mut command = match &shell {
        Some(interp) => {
            let prelude = match interp.as_str() {
                "oslo" => String::new(),
                _ => std::env::current_exe()
                    .map(|exe| argc_prelude(body, &exe.to_string_lossy()))
                    .unwrap_or_default(),
            };
            let mut command = std::process::Command::new(interp);
            command
                .arg("-c")
                .arg(format!("{prelude}. {path} \"$@\""))
                .arg(name);
            command.args(args);
            command
        }
        None => {
            let mut command = std::process::Command::new(&path);
            command.args(args);
            command
        }
    };
    // The name, for anything that cannot get it from `$0`.
    command.env("OSLO_SCRIPT", name);

    // **The descriptor has to survive the exec**, or the child is handed a path to nothing.
    // `memory_file` leaves it without `FD_CLOEXEC`; this keeps the guard alive until the child has
    // been started rather than letting it drop at the end of the `match` above.
    let outcome = command.status();
    drop(fd);

    match outcome {
        Ok(status) => status.code().unwrap_or(128),
        Err(e) => {
            eprintln!("oslo: {name}: {}", oslo_base::error::reason(&e));
            126
        }
    }
}

/// An `argc` for the child shell that sends `--argc-eval <name>` to oslo when `<name>` is no file.
///
/// **A stored script's `$0` is its name**, so `eval "$(argc --argc-eval "$0" "$@")"` handed the
/// `argc` program a word it cannot open: `Error: Failed to load script at 'con'`. oslo's own
/// `--argc-eval` reads the store first. Every other call — and any `$0` that is a real file — still
/// goes to the `argc` on `$PATH`, untouched.
fn argc_prelude(body: &str, oslo: &str) -> String {
    if !cfg!(feature = "argc") || !body.contains("--argc-eval") {
        return String::new();
    }
    let oslo = format!("'{}'", oslo.replace('\'', "'\\''"));
    format!(
        "argc() {{ if [ \"$1\" = --argc-eval ] && [ ! -e \"$2\" ]; then shift; {oslo} \
         --argc-eval \"$@\"; else command argc \"$@\"; fi; }}; "
    )
}

/// An anonymous in-memory file holding `body`, ready to be executed.
///
/// **No `MFD_CLOEXEC`**, and that is the point: the child has to inherit the descriptor, because
/// `/proc/self/fd/N` is how it reads the script. Closing it on exec would hand the kernel a path to
/// nothing.
///
/// `libc` through `nix`, which is where the rest of the shell reaches raw syscalls from — `nix`
/// 0.29 has no wrapper for this one.
fn memory_file(name: &str, body: &[u8]) -> Option<OwnedFd> {
    use std::io::Write;
    use std::os::fd::FromRawFd;

    let label = std::ffi::CString::new(format!("oslo:{name}")).ok()?;
    // SAFETY: `label` is a valid NUL-terminated C string that outlives the call, and the flags are
    // zero. The descriptor returned is owned here and wrapped immediately.
    let raw = unsafe { nix::libc::memfd_create(label.as_ptr(), 0) };
    if raw < 0 {
        return None;
    }
    // SAFETY: `raw` is a fresh descriptor this process owns and nothing else holds.
    let mut file = unsafe { std::fs::File::from_raw_fd(raw) };
    file.write_all(body).ok()?;
    file.flush().ok()?;
    Some(OwnedFd::from(file))
}

#[cfg(test)]
#[path = "stored/tests.rs"]
mod tests;
