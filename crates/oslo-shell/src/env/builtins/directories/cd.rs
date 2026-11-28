//! `cd` and `pwd`: the option matrix in front of the shared change-directory helper.
//!
//! [`builtin_cd`] is a ladder, and the order of its rungs is the whole safety argument. Options,
//! arity, `$HOME`, `$OLDPWD` and the directory ring are settled first and answer directly; then the
//! operand is tried as what it says it is, against the filesystem, through `CDPATH`. Only when that
//! has already failed is a remembered directory of that name considered, in [`super::jump`], and
//! only in a shell that has a store. So every POSIX case is decided before anything clever exists,
//! and the clever part can only ever turn an error into a success — never a success into a surprise.

use super::chdir::{PathMode, attempt_directory, logical_pwd, report_failure};
use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::env;

const CD_USAGE: &str = "cd: usage: cd [-L|[-P [-e]]] [-@] [dir]";
const PWD_USAGE: &str = "pwd: usage: pwd [-LP]";

/// Consume a leading run of `-L`/`-P` flags and an optional `--`, returning the mode and the
/// operands that follow.
///
/// Last flag wins (`cd -LP` is `cd -P`), a bare `-` is an operand rather than an option, and the
/// first operand ends option parsing — so `cd dir -P` has two operands, which is an error, not a
/// physical `cd`.
///
/// `cd` also accepts `-e`, which changes nothing here.
#[cfg(test)]
fn parse_mode(args: &[String]) -> std::result::Result<(PathMode, &[String]), String> {
    parse_mode_from(args, PathMode::Logical)
}

/// [`parse_mode`] with `mode` as the answer when no flag is given: physical under `set -P`.
fn parse_mode_from(
    args: &[String],
    mut mode: PathMode,
) -> std::result::Result<(PathMode, &[String]), String> {
    let mut index = 1;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--" {
            index += 1;
            break;
        }
        if arg.len() < 2 || !arg.starts_with('-') {
            break;
        }
        // `cd -3` is a destination, not a mistyped option: it means three back through the
        // directory history. Stopping here leaves it to be read as an operand, where it belongs.
        if arg[1..].chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        for flag in arg.chars().skip(1) {
            match flag {
                'L' => mode = PathMode::Logical,
                'P' => mode = PathMode::Physical,
                'e' if args[0] == "cd" => {}
                other => return Err(format!("-{other}")),
            }
        }
        index += 1;
    }
    Ok((mode, &args[index..]))
}

/// The mode `cd` and `pwd` use when no flag picks one.
fn default_mode(env: &Environment) -> PathMode {
    match env.physical() {
        true => PathMode::Physical,
        false => PathMode::Logical,
    }
}

pub fn builtin_cd(env: &mut Environment, args: &[String]) -> Result<i32> {
    let (mode, operands) = match parse_mode_from(args, default_mode(env)) {
        Ok(parsed) => parsed,
        Err(flag) => {
            crate::env::complain_with_usage(
                args,
                &flag,
                &format!("cd: {flag}: invalid option"),
                "not an option here",
                CD_USAGE,
            );
            return Ok(2);
        }
    };

    if operands.len() > 1 {
        // A usage error rather than a failed cd: the shell has not moved, and bash reports 2
        // for every builtin whose arguments do not parse.
        eprintln!("{}cd: too many arguments", origin_now());
        return Ok(2);
    }

    // `cd -` announces where it went, because the destination came from the environment rather
    // than from anything visible in the script.
    let mut announce = false;
    // Whether a failure may be answered by a remembered directory. Only an operand the user typed
    // as a name is a candidate: `$HOME`, `$OLDPWD` and a ring entry are places the shell itself
    // chose, and if one of those has gone, guessing at a replacement would be a lie about where it
    // was sending you.
    let mut named = false;
    let target = match operands.first().map(String::as_str) {
        None => match env.get_var("HOME").map(str::to_string) {
            Some(home) if !home.is_empty() => home,
            _ => {
                eprintln!("{}cd: HOME not set", origin_now());
                return Ok(1);
            }
        },
        Some("-") => {
            announce = true;
            match env.get_var("OLDPWD").map(str::to_string) {
                // An empty `OLDPWD` is a directory of no name: bash stays put and says so.
                Some(old) if old.is_empty() => {
                    println!();
                    return Ok(0);
                }
                Some(old) => old,
                None => {
                    eprintln!("{}cd: OLDPWD not set", origin_now());
                    return Ok(1);
                }
            }
        }
        // `cd -3` — three directories back through the ring, which `cd -` cannot express and
        // which is what you want the moment you are more than one wrong turn from home. Only a
        // *number* is treated this way; `cd -L` and `cd -P` are options and were handled above.
        Some(operand)
            if operand.len() > 1
                && operand.starts_with('-')
                && operand[1..].chars().all(|c| c.is_ascii_digit()) =>
        {
            announce = true;
            let n: usize = operand[1..].parse().unwrap_or(0);
            match super::ring::nth_back(n) {
                Some(path) => path,
                None => {
                    crate::env::complain(
                        args,
                        operand,
                        &format!("cd: {operand}: no such entry in the directory history"),
                        "further back than the history goes",
                        Some("`cd -` is the last directory; `cd -2` the one before it"),
                    );
                    return Ok(1);
                }
            }
        }
        Some(operand) => {
            named = true;
            operand.to_string()
        }
    };

    match attempt_directory(env, &target, mode) {
        Ok(destination) => {
            if announce {
                println!("{destination}");
            }
            Ok(0)
        }
        Err(e) => {
            // `cdable_vars`: a name that is not a directory may be a variable holding one.
            if named
                && crate::env::builtins::shopt::on("cdable_vars")
                && let Some(value) = env.get_var(&target).map(str::to_string)
                && let Ok(destination) = attempt_directory(env, &value, mode)
            {
                println!("{destination}");
                return Ok(0);
            }
            // A jump always says where it went, for the same reason `cd -` does: the destination
            // came from something the user cannot see on the line they typed.
            if named && let Some(destination) = super::jump::jump(env, &target, mode) {
                println!("{destination}");
                return Ok(0);
            }
            // Nothing else worked, so the original failure is the answer — the same message from
            // the same place it has always come from.
            report_failure(&env.origin(), "cd", &target, &e);
            Ok(1)
        }
    }
}

pub fn builtin_pwd(env: &mut Environment, args: &[String]) -> Result<i32> {
    let (mode, _operands) = match parse_mode_from(args, default_mode(env)) {
        // Operands are ignored: `pwd` has none, and bash does not complain about extras.
        Ok(parsed) => parsed,
        Err(flag) => {
            crate::env::complain_with_usage(
                args,
                &flag,
                &format!("pwd: {flag}: invalid option"),
                "not an option here",
                "pwd: usage: pwd [-LP]",
            );
            eprintln!("{PWD_USAGE}");
            return Ok(2);
        }
    };

    match mode {
        PathMode::Logical => println!("{}", logical_pwd(env)),
        PathMode::Physical => match env::current_dir() {
            Ok(path) => println!("{}", oslo_base::lossless::encode(path.as_os_str())),
            Err(e) => {
                eprintln!(
                    "{}pwd: error retrieving current directory: {e}",
                    origin_now()
                );
                return Ok(1);
            }
        },
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::super::ring;
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    /// The working directory and the ring are both process-wide, so a test that runs `cd` for real
    /// is a critical section over the whole test binary. Holding them in one guard also puts the
    /// directory back when an assertion fails, which a bare `set_current_dir` at the end would not:
    /// the scratch directory is deleted on drop, and a process left standing inside a deleted
    /// directory makes every later test fail for a reason that has nothing to do with it.
    struct Standing {
        _ring: std::sync::MutexGuard<'static, ()>,
        was: PathBuf,
    }

    impl Drop for Standing {
        fn drop(&mut self) {
            let _ = env::set_current_dir(&self.was);
        }
    }

    fn standing() -> Standing {
        let _ring = ring::exclusive();
        Standing {
            _ring,
            was: env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        }
    }

    /// A scratch tree with every symlink already resolved, so an expectation built from it can be
    /// compared against `$PWD`.
    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = dir.path().canonicalize().expect("canonicalize");
        (dir, base)
    }

    /// A shell standing in `base`, with `$PWD` saying so.
    fn shell(base: &Path) -> Environment {
        env::set_current_dir(base).expect("stand in the scratch directory");
        let mut shell = Environment::new();
        shell.set_var("PWD", &base.to_string_lossy(), true);
        shell
    }

    fn cd(shell: &mut Environment, args: &[&str]) -> i32 {
        builtin_cd(shell, &words(args)).expect("cd reports a status rather than erroring")
    }

    fn pwd(shell: &Environment) -> String {
        shell.get_var("PWD").unwrap_or_default().to_string()
    }

    fn at(base: &Path, name: &str) -> String {
        base.join(name).to_string_lossy().into_owned()
    }

    #[test]
    fn last_mode_flag_wins() {
        let args = words(&["cd", "-L", "-P", "dir"]);
        let (mode, operands) = parse_mode(&args).expect("flags parse");
        assert_eq!(mode, PathMode::Physical);
        assert_eq!(operands, ["dir"]);

        let args = words(&["cd", "-PL"]);
        let (mode, operands) = parse_mode(&args).expect("flags parse");
        assert_eq!(mode, PathMode::Logical);
        assert!(operands.is_empty());
    }

    #[test]
    fn double_dash_ends_options() {
        let args = words(&["cd", "--", "-P"]);
        let (mode, operands) = parse_mode(&args).expect("flags parse");
        assert_eq!(mode, PathMode::Logical);
        assert_eq!(operands, ["-P"]);
    }

    #[test]
    fn a_bare_dash_is_an_operand() {
        let args = words(&["cd", "-"]);
        let (_, operands) = parse_mode(&args).expect("flags parse");
        assert_eq!(operands, ["-"]);
    }

    #[test]
    fn an_operand_ends_option_parsing() {
        let args = words(&["cd", "dir", "-P"]);
        let (mode, operands) = parse_mode(&args).expect("flags parse");
        assert_eq!(mode, PathMode::Logical);
        assert_eq!(operands, ["dir", "-P"]);
    }

    #[test]
    fn unknown_flag_is_reported_by_letter() {
        assert_eq!(parse_mode(&words(&["cd", "-x"])).unwrap_err(), "-x");
        assert_eq!(parse_mode(&words(&["cd", "-Lx"])).unwrap_err(), "-x");
    }

    // --- the ladder, rung by rung ---

    /// Rung one, and the whole safety argument: an operand that names a real directory is a real
    /// directory, whatever else the word might have meant. `root` is the word `cd` would otherwise
    /// read as "the top of this working tree", and a machine that has a directory called `root`
    /// must not be teleported out of it.
    #[test]
    fn a_directory_that_exists_beats_every_word_cd_knows() {
        let _standing = standing();
        let (_dir, base) = scratch();
        fs::create_dir_all(base.join("root")).expect("a directory actually named root");

        let mut shell = shell(&base);
        assert_eq!(cd(&mut shell, &["cd", "root"]), 0);
        assert_eq!(pwd(&shell), at(&base, "root"));
    }

    /// Rung four, unchanged: a name nothing on disk answers to is the POSIX failure it has always
    /// been. A shell with no store — every script, every `oslo -c`, every subshell — cannot reach
    /// the jump at all, so this is the answer it gets there too, and the shell has not moved.
    #[test]
    fn a_name_that_resolves_to_nothing_fails_without_moving_the_shell() {
        let _standing = standing();
        let (_dir, base) = scratch();

        let mut shell = shell(&base);
        assert_eq!(cd(&mut shell, &["cd", "no-such-place-4e91"]), 1);
        assert_eq!(pwd(&shell), base.to_string_lossy());
        // The empty operand keeps its own wording and its own status, and is not a jump either.
        assert_eq!(cd(&mut shell, &["cd", ""]), 1);
        assert_eq!(pwd(&shell), base.to_string_lossy());
    }

    /// Bare `cd` is `$HOME`, and only `$HOME`. The owner's zsh `cd ~ && cd -` is deliberately not
    /// built: POSIX says `$HOME` and a script relies on it.
    #[test]
    fn bare_cd_goes_home() {
        let _standing = standing();
        let (_dir, base) = scratch();
        fs::create_dir_all(base.join("home")).expect("a home");

        let mut shell = shell(&base);
        shell.set_var("HOME", &at(&base, "home"), true);
        assert_eq!(cd(&mut shell, &["cd"]), 0);
        assert_eq!(pwd(&shell), at(&base, "home"));
    }

    /// `cd -N` counts back through this session's wandering, which `cd -` cannot express.
    #[test]
    fn cd_minus_n_counts_back_through_the_ring() {
        let _standing = standing();
        let (_dir, base) = scratch();
        for name in ["a", "b", "c"] {
            fs::create_dir_all(base.join(name)).expect("a subdirectory");
        }

        let mut shell = shell(&base);
        // The directory a shell starts in is the ring's first entry, as the interactive loop seeds
        // it; without it there is nothing for `cd -1` to count back to on the very first move.
        ring::record(&base.to_string_lossy());
        for name in ["a", "b", "c"] {
            assert_eq!(cd(&mut shell, &["cd", &at(&base, name)]), 0);
        }

        assert_eq!(cd(&mut shell, &["cd", "-2"]), 0);
        assert_eq!(pwd(&shell), at(&base, "a"), "two back from `c` is `a`");
        assert_eq!(
            cd(&mut shell, &["cd", "-99"]),
            1,
            "and counting past the start of the ring is an error, not a guess"
        );
        assert_eq!(pwd(&shell), at(&base, "a"), "which does not move the shell");
    }

    /// `cd -` reads `$OLDPWD` and `cd -1` counts back one through the ring. They are two different
    /// mechanisms and they must name the same directory, or the owner's model — "`cd -` already
    /// does that, and `cd -2` means two directories before" — is not true of the shell.
    #[test]
    fn cd_dash_and_cd_minus_one_name_the_same_directory() {
        let _standing = standing();
        let (_dir, base) = scratch();
        fs::create_dir_all(base.join("a")).expect("a subdirectory");

        let mut shell = shell(&base);
        ring::record(&base.to_string_lossy());

        assert_eq!(cd(&mut shell, &["cd", "a"]), 0);
        assert_eq!(cd(&mut shell, &["cd", "-1"]), 0);
        let by_number = pwd(&shell);

        assert_eq!(cd(&mut shell, &["cd", "a"]), 0);
        assert_eq!(cd(&mut shell, &["cd", "-"]), 0);
        let by_dash = pwd(&shell);

        assert_eq!(by_number, base.to_string_lossy());
        assert_eq!(by_dash, by_number);
    }

    /// `-P` and `-L` are options; `-2` is a destination. The ring arm must never swallow a flag and
    /// the flag arm must never swallow a number — this is the one place the two shapes collide.
    #[test]
    fn a_mode_flag_is_never_a_ring_entry() {
        let _standing = standing();
        let (_dir, base) = scratch();
        fs::create_dir_all(base.join("real")).expect("a directory");
        std::os::unix::fs::symlink(base.join("real"), base.join("link")).expect("a symlink");

        let mut shell = shell(&base);
        ring::record(&base.to_string_lossy());

        assert_eq!(cd(&mut shell, &["cd", "-P", "link"]), 0);
        assert_eq!(
            pwd(&shell),
            at(&base, "real"),
            "-P resolved the symlink; it was not read as a walk back through the ring"
        );

        // And the number still reaches the ring, from the same shell and the same parser.
        assert_eq!(cd(&mut shell, &["cd", "-1"]), 0);
        assert_eq!(pwd(&shell), base.to_string_lossy());
    }
}
