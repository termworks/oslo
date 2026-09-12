//! `rm`, as a builtin, with the conveniences confined to where they cannot hurt a script.
//!
//! # Why a builtin `rm` is a careful thing to write
//!
//! A builtin shadows `/bin/rm` for **everything** the shell runs, and oslo is meant to be
//! `/bin/sh` on a distribution. Every `postinst`, every `configure`, every `Makefile` recipe on
//! the machine would reach this code. So the rule here is narrow and mechanical:
//!
//! **In a non-interactive shell, `rm` does exactly what `rm` has always done.** No trash, and a
//! directory operand without `-r` is an error. The extensions exist only at a prompt, where a
//! person typed the line and can see what happened. `-s`/`--strict` asks for the same strictness
//! at a prompt, for the times you mean it.
//!
//! That is the same gate `crate::expand::sugar` puts on `=command` and `@name`, for the same
//! reason: a convenience that changes what a script means is not a convenience.
//!
//! # And why an unknown option is not an error
//!
//! GNU's `rm` has options this does not implement — `--one-file-system`, `--preserve-root=all`,
//! `-I`. A script using one of them must not break because oslo took the name over, so an option
//! this does not recognise hands the whole invocation to the real `rm` on `$PATH`. The builtin can
//! therefore never be *less* capable than the system's, which is the only honest way to shadow a
//! command everything depends on.

mod trash;
mod walk;

use crate::env::options::ShellOption;
use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::path::{Path, PathBuf};

/// What the options add up to.
struct Options {
    /// `-f`: missing operands are not errors, and nothing is ever prompted for.
    force: bool,
    /// `-i`: prompt before each removal.
    interactive: bool,
    /// `-r`/`-R`: recurse into directories.
    recursive: bool,
    /// `-d`: remove an empty directory, as `rmdir` would.
    dir: bool,
    /// `-v`: say what was removed.
    verbose: bool,
    /// `-s`/`--strict`: POSIX behaviour, even at a prompt.
    strict: bool,
}

/// How this invocation should behave, once the options and the session are both known.
struct Mode {
    /// Whether a directory may be removed without `-r`.
    loose: bool,
    /// Where a removal should move things, or `None` to unlink them.
    trash: Option<trash::Trash>,
}

pub fn builtin_rm(env: &mut Environment, args: &[String]) -> Result<i32> {
    let origin = env.origin();
    let (options, operands) = match parse(args) {
        Parsed::Options(options, operands) => (options, operands),
        // An option this does not implement: the real `rm` gets the whole line, unchanged.
        Parsed::Delegate => return delegate(args),
        Parsed::Usage(message) => {
            eprintln!("{origin}rm: {message}");
            return Ok(2);
        }
    };

    if operands.is_empty() {
        if options.force {
            // `rm -f` with nothing to remove is POSIX's one silent success.
            return Ok(0);
        }
        eprintln!("{origin}rm: missing operand");
        eprintln!("Try 'rm --help' for more information.");
        return Ok(1);
    }

    let mode = mode_for(env, &options);
    let mut status = 0;
    for operand in operands {
        // The name's real bytes: `rm b*` over `bad\xffname` must reach that file.
        let real = oslo_base::lossless::to_os(operand);
        match remove_operand(Path::new(&real), operand, &options, &mode, &origin) {
            Removal::Gone => {}
            Removal::Failed => status = 1,
            // A Ctrl-C part-way through stops the whole line, not just the operand it landed in:
            // the next one is as likely to be the big tree as the one that was interrupted.
            Removal::Interrupted => return Ok(130),
        }
    }
    Ok(status)
}

/// What one operand came to.
enum Removal {
    Gone,
    Failed,
    Interrupted,
}

/// Whether a person is actually typing at a prompt — the one condition the loose mode rests on.
///
/// **`-i` alone is not a prompt.** `ShellOption::Interactive` is set by the `-i` *flag*, and
/// `sh -i -c '…'` is an ordinary scripted form: the `bash -ic` trick that loads interactive rc
/// files for nvm and direnv, `SHELL='bash -i'` in a Makefile, ssh and CI shims. With the flag
/// alone as the test, a bare `rm dir` in one of those became `rm -rf dir` and answered **0** —
/// POSIX and bash both refuse a directory without `-r` and exit 1, so a caller had no way to know
/// a tree had been deleted. `Rm::to_tmp` is off by default, so there was no trash to recover from
/// either. That is the worst outcome this file can produce, and it was reachable from a script.
///
/// Three flags together, because each rules out a case the others do not: `Interactive` says the
/// session is interactive, the absence of `CommandString` says the shell was not handed a program
/// with `-c`, and the absence of `StdinInput` says it is not reading one from a pipe with `-s`.
/// All three are recorded when the shell is invoked, so this asks what the shell was *asked to be*
/// rather than probing a descriptor that a redirection could have moved.
fn at_a_prompt(env: &Environment) -> bool {
    let options = env.options();
    options.is_set(ShellOption::Interactive)
        && !options.is_set(ShellOption::CommandString)
        && !options.is_set(ShellOption::StdinInput)
}

/// The behaviour this shell allows, which is the whole safety argument in five lines.
fn mode_for(env: &Environment, options: &Options) -> Mode {
    if options.strict || !at_a_prompt(env) {
        return Mode {
            loose: false,
            trash: None,
        };
    }
    let all = oslo_ui::settings::current();
    let settings = &all.builtin.rm;
    Mode {
        loose: true,
        trash: settings.to_tmp.then(|| trash::Trash::new(settings)),
    }
}

/// Remove one operand.
fn remove_operand(
    path: &Path,
    shown: &str,
    options: &Options,
    mode: &Mode,
    origin: &str,
) -> Removal {
    // `symlink_metadata`, never `metadata`: `rm link-to-dir` removes the link and must not
    // recurse into what it points at. That distinction is the difference between deleting one
    // entry and deleting someone's home directory.
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if options.force {
                return Removal::Gone;
            }
            eprintln!(
                "{origin}rm: cannot remove {}: No such file or directory",
                oslo_base::shown::quoted(shown)
            );
            return Removal::Failed;
        }
        Err(e) => {
            eprintln!(
                "{origin}rm: cannot remove {}: {}",
                oslo_base::shown::quoted(shown),
                oslo_base::error::reason(&e)
            );
            return Removal::Failed;
        }
    };

    if let Some(refusal) = refuse(path, shown, &meta, options, mode) {
        eprintln!("{origin}rm: {refusal}");
        return Removal::Failed;
    }

    if let Some(trash) = &mode.trash
        && let Some(outcome) = trashed(trash, path, shown, &meta, options, origin)
    {
        return outcome;
    }

    // **`-d` on its own is `rmdir`, not `rm -r`**, and asking for it explicitly says so louder
    // than the prompt's convenience does: `loose` is what makes a typed `rm dir` work without
    // `-r`, so a line that named `-d` has already said which of the two it meant.
    let recursive = options.recursive || (mode.loose && !options.dir);
    let outcome = walk::remove_tree(
        path,
        shown,
        &walk::Walk {
            origin: origin.to_string(),
            force: options.force,
            interactive: options.interactive,
            recursive,
            verbose: options.verbose,
        },
    );
    match outcome {
        walk::Outcome {
            interrupted: true, ..
        } => Removal::Interrupted,
        walk::Outcome { failed: true, .. } => Removal::Failed,
        _ => Removal::Gone,
    }
}

/// Move the operand aside instead of destroying it, when the trash is on and it is small enough.
///
/// `None` means the trash declined it — too large — and the ordinary removal should go ahead.
fn trashed(
    trash: &trash::Trash,
    path: &Path,
    shown: &str,
    meta: &std::fs::Metadata,
    options: &Options,
    origin: &str,
) -> Option<Removal> {
    // The prompt still comes first: a trashed removal is recoverable, not invisible, and `-i`
    // asked to be told before anything moved.
    if options.interactive
        && !options.force
        && !walk::confirm(
            origin,
            &format!("remove {} '{shown}'", walk::describe(meta)),
        )
    {
        return Some(Removal::Gone);
    }
    match trash.take(path, shown, meta.is_dir())? {
        Ok(moved) => {
            if options.verbose {
                println!("moved '{shown}' to '{}'", moved.display());
            }
            Some(Removal::Gone)
        }
        Err(e) => {
            // **Not a fallback to destroying it.** The whole point of the trash is that a
            // removal is recoverable; quietly unlinking a file the move could not save would
            // be the one failure the user is relying on this not to have.
            eprintln!("{origin}rm: cannot move '{shown}' to the trash: {e}");
            Some(Removal::Failed)
        }
    }
}

/// Why this operand must not be removed, if it must not be.
fn refuse(
    path: &Path,
    shown: &str,
    meta: &std::fs::Metadata,
    options: &Options,
    mode: &Mode,
) -> Option<String> {
    // POSIX names these two explicitly, and the reason is not pedantry: `rm -r .` would walk
    // the working directory, and `rm -r ..` its parent, from a line that looks local.
    if ends_in_dot(shown) {
        return Some(format!(
            "refusing to remove '.' or '..' directory: skipping '{shown}'"
        ));
    }
    if !meta.is_dir() {
        return None;
    }
    // Only the real root, resolved — so a symlink pointing at `/` is caught too, and a directory
    // merely *named* `/` in a longer path is not.
    if path.canonicalize().is_ok_and(|full| full == Path::new("/")) {
        // Both lines, because the second is the one that tells a reader the refusal is a policy
        // with a way past it rather than a thing oslo cannot do.
        return Some(
            "it is dangerous to operate recursively on '/'\n\
             rm: use --no-preserve-root to override this failsafe"
                .to_string(),
        );
    }
    if options.recursive || options.dir || mode.loose {
        return None;
    }
    Some(format!(
        "cannot remove {}: Is a directory",
        oslo_base::shown::quoted(shown)
    ))
}

/// Whether the last component of an operand is `.` or `..`.
///
/// **Done on the text, not on a `Path`.** `Path::file_name` answers `None` for anything ending in
/// `..`, and `Path::components` drops a trailing `.` entirely — so both of the things this exists
/// to catch are invisible to the path API. `a/..` reached `remove_dir_all` on the parent directory
/// until this was written against the string instead.
fn ends_in_dot(operand: &str) -> bool {
    let trimmed = operand.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    // The empty case is `/` and its friends, which `refuse` catches by canonicalising instead.
    matches!(last, "." | "..")
}

/// What the argument list turned out to be.
enum Parsed<'a> {
    Options(Options, Vec<&'a String>),
    /// An option this does not implement. The real `rm` decides.
    Delegate,
    Usage(String),
}

fn parse(args: &[String]) -> Parsed<'_> {
    let mut options = Options {
        force: false,
        interactive: false,
        recursive: false,
        dir: false,
        verbose: false,
        strict: false,
    };
    let mut operands: Vec<&String> = Vec::new();
    let mut only_operands = false;

    for arg in args.iter().skip(1) {
        if only_operands || !arg.starts_with('-') || arg == "-" {
            operands.push(arg);
            continue;
        }
        if arg == "--" {
            only_operands = true;
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            match long {
                "force" => options.force = true,
                "recursive" => options.recursive = true,
                "interactive" => options.interactive = true,
                "dir" => options.dir = true,
                "verbose" => options.verbose = true,
                "strict" => options.strict = true,
                "help" => return Parsed::Usage(HELP.to_string()),
                _ => return Parsed::Delegate,
            }
            continue;
        }
        for letter in arg.chars().skip(1) {
            match letter {
                // `-f` and `-i` are the same knob from opposite ends, and the *last* one wins —
                // which is why each clears the other rather than only setting itself.
                'f' => {
                    options.force = true;
                    options.interactive = false;
                }
                'i' => {
                    options.interactive = true;
                    options.force = false;
                }
                'r' | 'R' => options.recursive = true,
                'd' => options.dir = true,
                'v' => options.verbose = true,
                's' => options.strict = true,
                _ => return Parsed::Delegate,
            }
        }
    }
    Parsed::Options(options, operands)
}

const HELP: &str = "usage: rm [-dfirRvs] [--strict] file...";

/// Hand the invocation to the real `rm`.
fn delegate(args: &[String]) -> Result<i32> {
    let Some(program) = external_rm() else {
        eprintln!(
            "{}rm: unknown option, and no rm on PATH to hand it to",
            origin_now()
        );
        return Ok(2);
    };
    super::spawn::run_external(&program, args, "rm")
}

/// The `rm` that is not this one.
///
/// `$PATH` first, so a machine that puts its coreutils somewhere unusual still works, then the two
/// places `rm` has lived for forty years — needed because a shell that has just been made
/// `/bin/sh` may be running with a `$PATH` that has not been set up yet.
fn external_rm() -> Option<PathBuf> {
    if let Some(found) = super::spawn::resolve_program("rm")
        && !is_this_shell(&found)
    {
        return Some(found);
    }
    ["/usr/bin/rm", "/bin/rm"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// Whether a path found on `$PATH` is this very shell.
///
/// **Both sides are resolved before they are compared.** This used to test the found path against
/// the literal `/usr/bin/oslo`, which is one of the places oslo can be and not the only one: an
/// install under `~/.local/bin`, a build being tested, or — the shape this shell already ships —
/// a *symlink* named for another program pointing at it. `resolve_program` answers the link, so a
/// name-only test never matched and oslo would have handed `rm` to itself.
///
/// A path that cannot be resolved is treated as not-this-shell: the fallbacks below are literal
/// files, and refusing to run a real `rm` because its link could not be read would be the worse
/// mistake of the two.
fn is_this_shell(candidate: &Path) -> bool {
    let Ok(found) = candidate.canonicalize() else {
        return false;
    };
    std::env::current_exe()
        .and_then(|exe| exe.canonicalize())
        .is_ok_and(|exe| exe == found)
}

#[cfg(test)]
#[path = "remove/tests.rs"]
mod tests;
