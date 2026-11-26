//! `whereis` — the files a name has, plus the answer only the shell knows.
//!
//! Where [`super`]'s `which` answers "what would run?", this answers "what is on disk under this
//! name?", and the two are different questions the moment a name is a builtin: `whereis cd` from
//! `/usr/bin/whereis` prints `cd:` and stops, which is true about the filesystem and useless about
//! the shell you typed it into.
//!
//! ```text
//! ls   → ls: /usr/bin/ls /usr/share/man/man1/ls.1.gz
//! cd   → cd: shell built-in command
//! mkcd → mkcd: stored function
//! ```
//!
//! A stored script is a file on `$PATH`, so it is reported by its path like any other program.
//!
//! # What is not searched
//!
//! Sources. `whereis -s` looks through a compiled-in list of source directories that means
//! something on a machine that keeps its ports tree in `/usr/src` and nothing on one that does not.
//! Inventing a list here would produce confident wrong answers, so `-s` is refused rather than
//! answered badly.

use crate::env::builtins::control::{self, Kind};
use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::path::PathBuf;

/// Where manual pages live when `$MANPATH` does not say.
const MAN_ROOTS: &[&str] = &[
    "/usr/share/man",
    "/usr/local/share/man",
    "/usr/local/man",
    "/usr/man",
];

#[derive(Default)]
struct Options {
    /// `-b`: binaries only.
    binaries: bool,
    /// `-m`: manuals only.
    manuals: bool,
}

impl Options {
    /// Neither flag means both, which is how every `whereis` reads them.
    fn wants_binaries(&self) -> bool {
        self.binaries || !self.manuals
    }

    fn wants_manuals(&self) -> bool {
        self.manuals || !self.binaries
    }
}

/// `whereis [-bm] name …`
pub fn builtin_whereis(env: &mut Environment, args: &[String]) -> Result<i32> {
    if let Some(handed_over) = super::handover(env, "whereis", args) {
        return handed_over;
    }
    let (opts, names) = match parse_options(args.get(1..).unwrap_or_default()) {
        Ok(parsed) => parsed,
        Err(code) => return Ok(code),
    };

    for name in names {
        let mut parts = Vec::new();
        if opts.wants_binaries() {
            parts.extend(everything_under(env, name));
        }
        if opts.wants_manuals() {
            parts.extend(manuals(name).iter().map(|p| p.display().to_string()));
        }
        // A name with nothing anywhere still prints its `name:` line — that is the program's
        // answer, and a column of names with empty lines beside them is the readable shape.
        println!(
            "{name}:{}",
            parts.iter().map(|p| format!(" {p}")).collect::<String>()
        );
    }

    // Always zero, as the program is: `whereis` reports, it does not judge. `which` is the one to
    // ask when the answer has to be a status.
    Ok(0)
}

/// Everything this name is: what the shell knows — builtin, alias, function, reserved word, stored
/// macro — and every file on `$PATH` that answers to it.
///
/// One entry per *thing*, which is why `oslo macros`' own copies are not among the files. A stored
/// script and the copy written for other shells are one macro, not a macro and a program, and
/// printing both would say the name means two things when it means one. Where it is, is the
/// database — the file is written from it and rewritten on every change.
fn everything_under(env: &Environment, name: &str) -> Vec<String> {
    control::ways(env, name, true)
        .iter()
        .map(|kind| match (kind, kind.alias_body()) {
            (Kind::File(path), _) => path.display().to_string(),
            (_, Some(body)) => format!("aliased to {body}"),
            _ => kind.noun().to_string(),
        })
        .collect()
}

/// Every manual page for `name`, from `$MANPATH` or the usual roots.
///
/// The directories are read rather than probed for likely names, because the section (`1`, `8`,
/// `3perl`) and the compression (`.gz`, `.zst`, none) both vary by distribution and a guessed
/// filename finds nothing on the machine that spells it differently.
fn manuals(name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in man_roots() {
        let Ok(sections) = std::fs::read_dir(&root) else {
            continue;
        };
        for section in sections.flatten() {
            if !section.file_name().to_string_lossy().starts_with("man") {
                continue;
            }
            let Ok(pages) = std::fs::read_dir(section.path()) else {
                continue;
            };
            for page in pages.flatten() {
                let file = page.file_name();
                let file = file.to_string_lossy();
                // `ls.1.gz` is a page for `ls`; `lsblk.8.gz` is not, and a prefix test would say
                // it was. The name ends at the first dot.
                if file.split('.').next() == Some(name) {
                    found.push(page.path());
                }
            }
        }
    }
    found.sort();
    found
}

fn man_roots() -> Vec<PathBuf> {
    match std::env::var_os("MANPATH") {
        Some(value) if !value.is_empty() => std::env::split_paths(&value)
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
        _ => MAN_ROOTS.iter().map(PathBuf::from).collect(),
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
        if !arg.starts_with('-') || arg == "-" {
            break;
        }
        for c in arg.chars().skip(1) {
            match c {
                'b' => opts.binaries = true,
                'm' => opts.manuals = true,
                other => {
                    crate::env::complain_option(
                        args,
                        other,
                        &format!("whereis: -{other}: invalid option"),
                        "whereis: usage: whereis [-bm] name [name ...]",
                    );
                    return Err(2);
                }
            }
        }
        rest = &rest[1..];
    }
    Ok((opts, rest))
}
