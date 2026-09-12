//! The macros a spec file names that only a shell can answer.
//!
//! ```yaml
//! completion:
//!   positional:
//!     - ["$(git branch --format '%(refname:short)')"]
//!     - ["$bash(compgen -A hostname)"]
//! ```
//!
//! # `$(…)` runs oslo, not `sh`
//!
//! carapace has to run `sh -c` here: it is a completion binary, and the shell it is completing for
//! is somewhere else. oslo is the shell, so `$(…)` runs *oslo* — no `sh` on `$PATH` required, and
//! oslo's own semantics for the command a spec author wrote.
//!
//! It used to evaluate in this process, through the same command-substitution path a `$(…)` on a
//! real line takes. That saved a fork and could not be given a **deadline**, which is the thing
//! that matters here: this is the Tab keystroke path, with the terminal in raw mode, so a macro
//! that blocks on a network or a dead mount stopped the editor for good. Every macro now runs as a
//! child under a deadline. Nothing was lost by forking — the in-process path was handed a *fresh*
//! `Environment`, so it never saw this session's variables, functions or aliases either.
//!
//! A shell that is named — `$bash(…)`, `$zsh(…)` — is run as itself, because a spec that asks for
//! bash is asking for bash's own completions. One that is not installed answers nothing rather than
//! an error: a spec is written for many machines and a missing shell is a fact about this one.
//!
//! # What the command is told
//!
//! The variables of the line — `C_VALUE`, `C_ARG0…`, `C_FLAG_…` — as assignments, and the words
//! already typed as `"$@"`. That is what carapace passes and it is what a `compgen` line needs.

use oslo_ui::spec::action::{Offer, Query};

/// Shells that may be named, and that oslo will not pretend to be.
const SHELLS: &[&str] = &[
    "bash", "zsh", "fish", "nu", "elvish", "xonsh", "osh", "pwsh", "cmd",
];

/// Answer one macro, or nothing at all.
pub fn offers(name: &str, arg: &str, query: &Query) -> Vec<Offer> {
    match name {
        // `$(cmd)` and `$sh(cmd)` are oslo's own.
        "" | "sh" => rows(&here(arg, query)),
        shell if SHELLS.contains(&shell) => rows(&elsewhere(shell, arg, query)),
        // `$spec(file)` hands the rest of the parse to another spec file, which is a re-entry into
        // the walk rather than a list of values. Not read yet, and quiet rather than wrong.
        _ => Vec::new(),
    }
}

/// How long a macro may take before the editor gives up on it.
///
/// **This runs on the Tab keystroke path**, in the shell's own process, with the terminal in raw
/// mode and nothing else able to draw. A macro with no deadline is a shell that stops responding
/// for as long as somebody's `git`, `ssh` or `compgen` takes — and if that command blocks on a
/// network or a dead mount, for good, with no way out but another terminal.
///
/// Two seconds is chosen to be longer than any completion worth waiting for and shorter than a
/// person's patience: `git branch` on a large repository is milliseconds, and anything past this is
/// not going to arrive in time to be a completion.
const LONGEST: std::time::Duration = std::time::Duration::from_secs(2);

/// Run a prepared command under [`LONGEST`], answering what it printed.
///
/// **Drained on a thread of its own**, because polling for exit without reading the pipe means a
/// command with more than a pipe buffer to say blocks on the write and is then killed for taking
/// too long — the same fault `lua::api::spawn` had.
fn bounded(process: std::process::Command) -> String {
    bounded_within(process, LONGEST).out
}

/// How a bounded command ended.
pub(super) struct Ran {
    pub out: String,
    /// What it wrote to stderr, which is only ever read to explain a failure.
    pub err: String,
    /// It exited, and with success.
    pub ok: bool,
    /// It was still running at the deadline and was killed.
    pub expired: bool,
}

/// [`bounded`], under a deadline of the caller's choosing, saying how it ended.
///
/// **An empty answer and a failed one are different things** to a caller that has to choose between
/// them: an empty directory has been listed and an unreachable machine has not. `bounded` itself
/// cannot tell them apart, because a macro that prints nothing is a macro that offers nothing
/// either way — see [`super::remote`], which is the caller that needs the distinction.
pub(super) fn bounded_within(
    mut process: std::process::Command,
    within: std::time::Duration,
) -> Ran {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    // **A process group of its own, so the deadline can reach what the macro started.** A macro is
    // a shell command, so the child is usually `sh` and the thing that blocks is *its* child —
    // which inherits the write end of the pipe. Killing only the direct child leaves that
    // grandchild holding the pipe open, so the read below waits for it anyway and the deadline buys
    // nothing: measured, a `$(sleep 60)` still took sixty seconds. One signal to the group is what
    // `timeout(1)` does, and for this reason.
    //
    // SAFETY: `setpgid` is async-signal-safe and is all this does between `fork` and `exec`.
    unsafe {
        process.pre_exec(|| {
            let _ =
                nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0));
            Ok(())
        });
    }
    let mut child = match process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // A macro's complaints are not completions. They would otherwise land on the terminal in
        // the middle of a drawn dropdown, so they are kept to explain a failure instead.
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return Ran {
                out: String::new(),
                err: error.to_string(),
                ok: false,
                expired: false,
            };
        }
    };
    fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> Option<std::thread::JoinHandle<String>> {
        pipe.map(|mut pipe| {
            std::thread::spawn(move || {
                let mut out = String::new();
                let _ = pipe.read_to_string(&mut out);
                out
            })
        })
    }
    let reading = drain(child.stdout.take());
    let complaining = drain(child.stderr.take());

    let deadline = std::time::Instant::now() + within;
    let mut finished = false;
    let mut expired = false;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                finished = status.success();
                break;
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            // Out of time. Killed rather than left running: it is writing into a pipe nobody will
            // read again, and a completion that arrives after the menu is gone is not one.
            Ok(None) => {
                // The whole group, so a grandchild holding the pipe goes too — see above.
                let group = nix::unistd::Pid::from_raw(child.id() as i32);
                let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                expired = true;
                break;
            }
            Err(_) => break,
        }
    }
    let joined = |reader: Option<std::thread::JoinHandle<String>>| {
        reader
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default()
    };
    Ran {
        out: joined(reading),
        err: joined(complaining),
        ok: finished,
        expired,
    }
}

/// Run `command` in this shell, in a subshell, and answer with what it printed.
fn here(command: &str, query: &Query) -> String {
    let mut script = preamble(query);
    if !query.dir.is_empty() {
        script.push_str(&format!(
            "cd {} 2>/dev/null || exit 0\n",
            quoted(&query.dir)
        ));
    }
    script.push_str(command);
    script.push('\n');

    // **Run as a child of this shell rather than inside it**, which is a change from what the module
    // note above describes and is here for one reason: a deadline. The substitution path evaluates
    // in this process and waits for whatever it forks, so a macro that blocks — a `git` on a dead
    // mount, an `ssh` with no route — stopped the editor with the terminal in raw mode and no way
    // out. Nothing is lost by forking: the substitution was given a *fresh* `Environment` anyway,
    // so it never saw this session's variables, functions or aliases either.
    //
    // The magic link, not the resolved name — see `oslo_base::exe`.
    let mut process = std::process::Command::new(oslo_base::exe::path());
    process.arg("-c").arg(&script).arg("--");
    process.args(query.words.iter().skip(1));
    if !query.dir.is_empty() {
        process.current_dir(&query.dir);
    }
    bounded(process)
}

/// Run `command` in the shell it named.
fn elsewhere(shell: &str, command: &str, query: &Query) -> String {
    let Ok(program) = which::which(shell) else {
        return String::new();
    };
    let mut process = std::process::Command::new(program);
    match shell {
        "cmd" => {
            process.arg("/c").arg(command);
        }
        "nu" | "pwsh" | "elvish" | "xonsh" => {
            process.arg("-c").arg(command);
        }
        // The POSIX-ish ones take the words after a `--`, so the command sees them as `"$@"`.
        _ => {
            process.arg("-c").arg(command).arg("--");
            process.args(query.words.iter().skip(1));
        }
    }
    for (name, value) in variables(query) {
        process.env(name, value);
    }
    if !query.dir.is_empty() {
        process.current_dir(&query.dir);
    }
    bounded(process)
}

/// The assignments and the `set --` a command is run under.
fn preamble(query: &Query) -> String {
    let mut script = String::new();
    for (name, value) in variables(query) {
        script.push_str(&format!("{name}={}\n", quoted(&value)));
    }
    // The words the command was typed with, minus the command's own name — the same `"$@"`
    // carapace hands a `sh -c … -- "$@"`.
    script.push_str("set --");
    for word in query.words.iter().skip(1) {
        script.push(' ');
        script.push_str(&quoted(word));
    }
    script.push('\n');
    script
}

/// `C_VALUE`, `C_ARG0…`, `C_FLAG_…`: what the line has said so far.
fn variables(query: &Query) -> Vec<(String, String)> {
    let mut out = vec![("C_VALUE".to_string(), query.value.clone())];
    for (index, arg) in query.args.iter().enumerate() {
        out.push((format!("C_ARG{index}"), arg.clone()));
    }
    for (name, value) in &query.flags {
        if is_a_name(name) {
            out.push((format!("C_FLAG_{name}"), value.clone()));
        }
    }
    out
}

/// What a command printed, one offer per line.
fn rows(output: &str) -> Vec<Offer> {
    output
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.splitn(3, '\t');
            Offer {
                value: fields.next().unwrap_or_default().to_string(),
                description: fields.next().filter(|d| !d.is_empty()).map(str::to_string),
                tag: None,
            }
        })
        .collect()
}

/// Whether a word may be written as a shell variable name.
fn is_a_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// One word, quoted so the shell reads it back as itself.
fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Quoting and shaping only, no subshell.** Command substitution forks, and forking from a
    /// test process with a dozen threads in it is how a suite hangs — the same rule `argc::call`
    /// follows, and for the same reason it was learnt.
    #[test]
    fn the_line_reaches_the_command_as_variables_and_arguments() {
        let mut flags = std::collections::HashMap::new();
        flags.insert("FILE".to_string(), "out.txt".to_string());
        let query = Query {
            args: vec!["build".into()],
            words: vec!["deploy".into(), "build".into()],
            value: "part".into(),
            flags,
            dir: String::new(),
        };
        let script = preamble(&query);
        assert!(script.contains("C_VALUE='part'"), "{script}");
        assert!(script.contains("C_ARG0='build'"), "{script}");
        assert!(script.contains("C_FLAG_FILE='out.txt'"), "{script}");
        assert!(script.ends_with("set -- 'build'\n"), "{script}");
    }

    #[test]
    fn a_word_is_written_so_the_shell_reads_it_back_as_itself() {
        assert_eq!(quoted("it's"), "'it'\\''s'");
        assert_eq!(quoted("$HOME"), "'$HOME'");
    }

    #[test]
    fn every_printed_line_is_an_offer_and_a_tab_splits_off_its_description() {
        let offers = rows("one\ntwo\twith description\n\nthree\tstyled\tblue\n");
        assert_eq!(offers.len(), 3);
        assert_eq!(offers[0].value, "one");
        assert_eq!(offers[1].description.as_deref(), Some("with description"));
        // The third field is carapace's style, which oslo paints from its own theme.
        assert_eq!(offers[2].value, "three");
        assert_eq!(offers[2].description.as_deref(), Some("styled"));
    }

    /// A macro naming a shell that is not installed answers nothing. A spec is written for many
    /// machines; a missing shell is a fact about this one, not an error in the spec.
    #[test]
    fn a_shell_that_is_not_here_is_quiet() {
        let query = Query::default();
        assert!(offers("definitely-not-a-shell", "echo hi", &query).is_empty());
        assert!(offers("spec", "other.yaml", &query).is_empty());
    }
}

/// **A macro runs on the Tab keystroke path and must not be able to stop the editor.**
///
/// It had no deadline at all: `$(…)` evaluated in this process and waited for whatever it forked,
/// and `$bash(…)` called `output()`, which waits for ever. With the terminal in raw mode, a macro
/// blocking on a network or a dead mount was a shell that stopped responding with no way out.
#[cfg(test)]
mod deadline_tests {
    use super::{LONGEST, bounded};
    use std::time::{Duration, Instant};

    fn sh(script: &str) -> std::process::Command {
        let mut command = std::process::Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    #[test]
    fn a_macro_that_answers_is_not_delayed() {
        let started = Instant::now();
        assert_eq!(bounded(sh("printf 'alpha\\nbeta\\n'")), "alpha\nbeta\n");
        assert!(
            started.elapsed() < LONGEST,
            "it returned when the command did, not at the deadline"
        );
    }

    #[test]
    fn a_macro_that_blocks_gives_up() {
        let started = Instant::now();
        let out = bounded(sh("sleep 60; echo never"));
        let took = started.elapsed();

        assert_eq!(out, "", "nothing arrived from the command that blocked");
        assert!(took >= LONGEST, "it waited its allowance: {took:?}");
        assert!(
            took < LONGEST + Duration::from_secs(5),
            "and gave up rather than waiting for the command: {took:?}"
        );
    }

    /// More than a pipe buffer, well inside the deadline: polling for exit without draining the
    /// pipe would block the command on its own write and then kill it for taking too long.
    #[test]
    fn a_large_answer_is_not_mistaken_for_a_hang() {
        let (out, took) = {
            let started = Instant::now();
            let out = bounded(sh("head -c 200000 /dev/zero | tr '\\0' a"));
            (out, started.elapsed())
        };
        assert_eq!(out.len(), 200_000, "all of it came back");
        assert!(took < LONGEST, "and well inside the deadline: {took:?}");
    }

    /// A command that does not exist answers nothing rather than hanging or complaining.
    #[test]
    fn a_command_that_cannot_run_answers_nothing() {
        let mut missing = std::process::Command::new("oslo-no-such-program-anywhere");
        assert_eq!(bounded(std::mem::replace(&mut missing, sh("true"))), "");
    }
}
