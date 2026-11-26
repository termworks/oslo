//! `which` and `whereis` — builtins, because the programs by those names cannot see a shell.
//!
//! # Why they are not left to `/usr/bin/which`
//!
//! `which` is a program that reads `$PATH`, and everything interesting about a name in oslo is
//! invisible to a program: an alias lives in this shell's memory, a builtin has no file at all, and
//! a stored function is a row in a database that dispatch reaches *after* `$PATH`. So the honest
//! answer to `which ll` is one only the shell can give, and `/usr/bin/which` answers "nothing" —
//! not because nothing runs, but because it was asked by something that cannot see.
//!
//! zsh has had `which` as a builtin for this reason for thirty years; bash has `type` and leaves
//! `which` wrong. This is the zsh answer, in zsh's wording:
//!
//! ```text
//! ls        → /usr/bin/ls
//! ll        → ll: aliased to ls -alF
//! cd        → cd: shell built-in command
//! mkcd      → mkcd: stored function
//! if        → if: shell reserved word
//! ```
//!
//! A path is printed bare, because a bare path is what every `$(which foo)` in the world expects.
//! Anything that is *not* a file is `name: what it is`, which no script can mistake for a path.
//!
//! **The real program is still there**, at `/usr/bin/which`, for a script that wants a `$PATH`
//! search and nothing else — as is `which --skip-alias`, which does the same thing from here.
//!
//! # The one dispatch table
//!
//! Neither of these knows the resolution order. They ask [`control::ways`], which is what `type`
//! prints and what `command -v` agrees with; a `which` that disagreed with dispatch would be worse
//! than the one that was wrong for a visible reason.

mod whereis;

pub use whereis::builtin_whereis;

use crate::env::builtins::control::{self, Kind};
use crate::env::scope::Environment;
use oslo_base::error::Result;

#[derive(Default, Debug)]
struct Options {
    /// `-a`: every match, not just the first — including the ones a nearer match shadows.
    all: bool,
    /// `-s`: no output, the status alone. What a probe wants.
    silent: bool,
    /// `--skip-alias`: a plain `$PATH` search, which is what the program by this name does.
    skip_shell: bool,
}

/// `which [-as] [--skip-alias] name …`
pub fn builtin_which(env: &mut Environment, args: &[String]) -> Result<i32> {
    if let Some(handed_over) = handover(env, "which", args) {
        return handed_over;
    }
    let (opts, names) = match parse_options(args.get(1..).unwrap_or_default()) {
        Ok(parsed) => parsed,
        Err(code) => return Ok(code),
    };
    if names.is_empty() {
        // The program prints its usage and fails; a shell with no names to look up has nothing to
        // say and nothing went wrong either, so it agrees with the program: status 1, no output.
        return Ok(1);
    }

    let mut status = 0;
    for name in names {
        let found = report(env, name, &opts);
        if !found {
            // **Silent on a miss, like every `which` since BSD.** `which foo >/dev/null || …` is
            // the whole idiom; a shell that printed "not found" would break it in the noisiest way.
            status = 1;
        }
    }
    Ok(status)
}

/// Run the program by this name instead, when the shell is not the one that typed the command.
///
/// **A script gets the program it was written for.** `which` is not POSIX; what a script has always
/// had is `command -v`, which answers about *this* shell and knows every alias, builtin and stored
/// macro there is. `which` is the interactive convenience on top, and somebody's configure script
/// doing `ECHO=$(which echo)` must keep getting `/usr/bin/echo` rather than a sentence about a
/// builtin — which is exactly what it would get from the shell it was written for.
///
/// The same rule as the frecency jump in `cd` and as `\command`: the shell-aware behaviour is for
/// the prompt, and a script sees what every other shell shows it.
///
/// `None` when this shell should answer: at a prompt, or on a system with no such program at all —
/// a small distribution may ship neither, and the builtin is better than nothing.
fn handover(env: &Environment, name: &str, args: &[String]) -> Option<Result<i32>> {
    if env.interactive() {
        return None;
    }
    let program = crate::env::builtins::spawn::resolve_program(name)?;
    Some(crate::env::builtins::spawn::run_external(
        &program, args, name,
    ))
}

/// Print what `name` resolves to; `false` when nothing does.
fn report(env: &Environment, name: &str, opts: &Options) -> bool {
    let ways = ways_for(env, name, opts);
    if ways.is_empty() {
        return false;
    }
    if !opts.silent {
        for kind in &ways {
            println!("{}", line(name, kind));
        }
    }
    true
}

/// The resolutions to report, which `--skip-alias` narrows to files.
///
/// **And to *every* file, oslo's own copies included.** Everywhere else those are left out, because
/// dispatch leaves them out — but a caller writing `which --skip-alias` is asking for what the
/// program by this name would say, and the copy is a real file on `$PATH` that a real `which` would
/// find and any other shell would run. Answering "nothing" to the flag that means "just search
/// `$PATH`" would be the least useful reading of it.
fn ways_for(env: &Environment, name: &str, opts: &Options) -> Vec<Kind> {
    if opts.skip_shell {
        let mut files = binaries(name);
        if !opts.all {
            files.truncate(1);
        }
        return files.into_iter().map(Kind::File).collect();
    }
    control::ways(env, name, opts.all)
}

/// Every file named `name` on `$PATH`, in search order and unfiltered.
///
/// Unfiltered of oslo's own macro copies, that is — a name this directory hides is still hidden.
/// `which --skip-alias` asks a narrower question than `type`, not a different one, and the two
/// disagreeing about whether a command exists is worse than either answer. See `oslo_base::command`.
pub(super) fn binaries(name: &str) -> Vec<std::path::PathBuf> {
    if oslo_base::command::hidden(name) {
        return Vec::new();
    }
    which::which_all(name)
        .map(|found| found.collect())
        .unwrap_or_default()
}

/// One line of output: a bare path for a file, `name: what it is` for everything else.
fn line(name: &str, kind: &Kind) -> String {
    match (kind.path(), kind.alias_body()) {
        (Some(path), _) => path.display().to_string(),
        (None, Some(body)) => format!("{name}: aliased to {body}"),
        (None, None) => format!("{name}: {}", kind.noun()),
    }
}

/// Split the leading option run off `args`, or produce the exit status for a usage error.
fn parse_options(args: &[String]) -> std::result::Result<(Options, &[String]), i32> {
    let mut opts = Options::default();
    let mut rest = args;
    while let Some(arg) = rest.first() {
        if arg == "--" {
            rest = &rest[1..];
            break;
        }
        // The long options the program by this name takes, so a script written for it keeps
        // working. `--read-alias` is what this builtin does anyway, and asking for it is not an
        // error even though nothing has to be done about it.
        match arg.as_str() {
            "--all" => opts.all = true,
            "--skip-alias" | "--skip-functions" => opts.skip_shell = true,
            "--read-alias" | "--read-functions" | "--tty-only" => {}
            _ if !arg.starts_with('-') || arg == "-" => break,
            _ if arg.starts_with("--") => {
                crate::env::complain_with_usage(
                    args,
                    arg,
                    &format!("which: {arg}: invalid option"),
                    "not an option here",
                    "which: usage: which [-as] [--skip-alias] name [name ...]",
                );
                return Err(2);
            }
            _ => {
                for c in arg.chars().skip(1) {
                    match c {
                        'a' => opts.all = true,
                        's' => opts.silent = true,
                        other => {
                            crate::env::complain_option(
                                args,
                                other,
                                &format!("which: -{other}: invalid option"),
                                "which: usage: which [-as] [--skip-alias] name [name ...]",
                            );
                            return Err(2);
                        }
                    }
                }
            }
        }
        rest = &rest[1..];
    }
    Ok((opts, rest))
}

#[cfg(test)]
#[path = "locate/tests.rs"]
mod tests;
