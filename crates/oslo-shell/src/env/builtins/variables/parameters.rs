//! `set` and `shift`: the shell options, the positional parameters, and the state listing.

use super::quoting::quote_minimal;
use crate::env::options::{SetError, SetListing, ShellOption, parse_set_args};
use crate::env::origin_now;
use crate::env::scope::{Environment, is_valid_identifier};
use oslo_base::error::{Result, ShellError};

const USAGE: &str = "set: usage: set [-abCefhkmnotuvx] [-o option-name] [--] [-] [arg ...]";

/// `set` — change shell options, replace the positional parameters, or list the shell's state.
///
/// The three jobs are one builtin because POSIX made them one, and telling them apart is where
/// this used to go wrong: every word was a positional parameter, so `set -e` armed nothing and
/// `set -euo pipefail` left `$1` as `-euo`. Option words now end at the first operand, at `--`,
/// or at a bare `-`; see [`parse_set_args`] for the grammar.
///
/// The no-argument listing is sorted and quoted. It used to iterate a `HashMap` and print values
/// raw, which meant two runs of the same script produced different output *and* neither could be
/// read back: `x=a b` is an assignment followed by a command.
pub fn builtin_set(env: &mut Environment, args: &[String]) -> Result<i32> {
    if args.len() == 1 {
        print_variables(env);
        print_functions(env);
        return Ok(0);
    }

    // **Read before `parse_set_args`, and it never reaches it.** `-U` is not a shell option and
    // its operands are not positional parameters, so putting it through the POSIX grammar would
    // mean teaching that grammar about a word that is neither. It is one branch here instead, and
    // everything below is exactly the `set` POSIX describes. See `env::universal`.
    #[cfg(feature = "universal")]
    if args.get(1).is_some_and(|word| word == "-U") {
        return Ok(universal(env, &args[2..]));
    }

    // Parsed in full before anything is applied: a bad letter late in the line must not leave
    // half the options changed. bash agrees — `set -e -z` leaves `errexit` off.
    let parsed = match parse_set_args(args) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("{}set: {}", origin_now(), err);
            // The usage line answers "which letters are there?", so it follows a bad *letter*
            // and not a bad `-o` name, where it would list nothing relevant.
            if matches!(err, SetError::InvalidOption(_)) {
                eprintln!("{}", USAGE);
            }
            // Not `Ok(2)`: `set` is a *special* builtin, and POSIX 2.8.1 ends a non-interactive
            // shell that gives one a utility error. `bash --posix -c 'set -o nosuchopt; echo
            // alive'` prints no `alive` and exits 2. Outside POSIX mode `posix::
            // resolve_builtin_result` folds this straight back to `Ok(2)`, so nothing else
            // changes — the error type is how the builtin says "utility error" rather than
            // "ran and returned 2", which `shift 5` also does and which must *not* be fatal.
            return Err(ShellError::utility_error("set: invalid option", 2));
        }
    };

    let mut status = 0;
    for (option, on) in parsed.changes {
        // An option oslo does not implement is refused when you ask to turn it *on*, and accepted
        // when you ask to turn it off — it is already off, so there is nothing to disagree with.
        // Accepting both and doing neither is the failure mode `shopt`'s fixed states exist to
        // avoid: a script that sets an option and gets status 0 is entitled to believe it took.
        if on
            && let Some(spec) = crate::env::options::OptionSpec::of(option)
            && let Some(why) = spec.unsupported
        {
            eprintln!(
                "oslo: set: {}: not supported; {why}",
                spec.name.unwrap_or("?")
            );
            status = 1;
            continue;
        }
        env.set_option(option, on);
        // `-m` is the one option that has to *do* something rather than be remembered: job control
        // means owning the terminal and leading a process group, and neither happens by reading a
        // flag later. Without this a script's `set -m` got the half of job control that needs no
        // terminal — separate process groups — while `fg` and `bg` answered `no job control`.
        if option == ShellOption::Monitor {
            apply_monitor(on);
        }
        // Likewise `hashall`: the table lives in a thread-local the option has to reach.
        if option == ShellOption::HashAll {
            crate::env::builtins::note_hashall(on);
        }
    }
    for listing in parsed.listings {
        let text = match listing {
            SetListing::Long => env.options().long_listing(),
            SetListing::Reinputtable => env.options().reinputtable_listing(),
        };
        print!("{}", text);
    }
    // `None` means no operands were given, which leaves `$1 …` exactly as they were: `set -e`
    // must not wipe the arguments the script was called with.
    if let Some(positional) = parsed.positional {
        env.set_positional(positional);
    }

    Ok(status)
}

/// Turn job control on or off to match `set -m` / `set +m`.
///
/// Silent about failure on purpose. `set -m` with no controlling terminal — a script in a
/// pipeline, a cron job — is legal and simply has nothing to claim; bash accepts it too and the
/// option stays recorded either way, so `set -o` still reports what the script asked for.
fn apply_monitor(on: bool) {
    if on {
        crate::exec::job::enable_job_control();
    } else {
        crate::exec::job::leave_job_control();
    }
}

/// `set -U [-e] [NAME [VALUE...]]` — the variables every session shares.
///
/// ```sh
/// set -U theme dark     # here, and in every other oslo window
/// set -U                # what is in the store
/// set -U -e theme       # gone, everywhere
/// ```
///
/// **Set here means set here too**, immediately: the store is written and this session's own copy
/// with it, so the window you typed in is not the last one to find out. Every other session picks
/// it up at its next prompt. See [`crate::env::universal`].
///
/// Several values join with a space rather than becoming a list. A shell variable is a string —
/// that is the whole of the model oslo's expansion is built on — and inventing a second kind here
/// would mean inventing what `$theme` does when it holds one.
#[cfg(feature = "universal")]
fn universal(env: &mut Environment, args: &[String]) -> i32 {
    use crate::env::universal;

    let (erasing, rest) = match args.split_first() {
        Some((first, rest)) if first == "-e" || first == "--erase" => (true, rest),
        _ => (false, args),
    };

    let Some((name, values)) = rest.split_first() else {
        if erasing {
            eprintln!("{}set -U -e: needs the name of a variable", origin_now());
            return 2;
        }
        for (name, value) in universal::all() {
            println!("{name}={}", quote_minimal(&value));
        }
        return 0;
    };
    if !is_valid_identifier(name) {
        // `set -U` goes back on the front: `args` here starts after them, and a line rebuilt
        // from it alone would head the report with the variable name.
        let mut words = vec!["set".to_string(), "-U".to_string()];
        words.extend(args.iter().cloned());
        crate::env::complain(
            &words,
            name,
            &format!("set -U: {name}: not a valid name"),
            "not a name a shell can read",
            Some("a name is letters, digits and underscores, and does not start with a digit"),
        );
        return 2;
    }

    if erasing {
        return match universal::erase(name) {
            // Erased everywhere, and here. Status 1 for a name the store never had, which is what
            // lets a script tell "removed" from "was not there".
            Ok(had) => {
                env.unset_var(name);
                i32::from(!had)
            }
            Err(problem) => {
                eprintln!("{}set -U: {problem}", origin_now());
                1
            }
        };
    }

    let value = values.join(" ");
    match universal::set(name, &value) {
        Ok(()) => {
            env.set_var(name, &value, false);
            0
        }
        Err(problem) => {
            eprintln!("{}set -U: {problem}", origin_now());
            1
        }
    }
}

/// Print every shell variable as `name=value`, sorted, quoted only where quoting is needed.
///
/// Names `environ` accepted but no shell can parse — bash's `BASH_FUNC_x%%` entries arrive this
/// way — are skipped, because the listing's contract is that a shell can read it back.
fn print_variables(env: &Environment) {
    let vars = env.get_all_vars();
    let mut names: Vec<&String> = vars
        .keys()
        .filter(|name| is_valid_identifier(name))
        .collect();
    names.sort();
    for name in names {
        println!("{}={}", name, quote_minimal(&vars[name]));
    }
}

/// Print every function definition, sorted by name.
fn print_functions(env: &Environment) {
    let mut names: Vec<&String> = env.get_functions().keys().collect();
    names.sort();
    for name in names {
        // The same printer `type` and `declare -f` use.
        if let Some(definition) = env.function_definition(name) {
            println!("{definition}");
        }
    }
}

/// `shift [n]` — drop the first `n` positional parameters.
pub fn builtin_shift(env: &mut Environment, args: &[String]) -> Result<i32> {
    let n = if args.len() > 1 {
        match args[1].parse::<usize>() {
            Ok(num) => num,
            // 2, not 1: a bad operand is a *usage* error, and bash numbers those apart from the
            // ordinary failure `shift` past the end reports.
            Err(_) => {
                crate::env::complain(
                    args,
                    &args[1],
                    &format!("shift: {}: numeric argument required", args[1]),
                    "not a number",
                    Some("shift takes a count, and defaults to 1"),
                );
                return Ok(2);
            }
        }
    } else {
        1
    };

    let pos = env.get_positional().to_vec();
    // **Status 1, and quiet unless POSIX mode asked.** Shifting past the end is how a loop over
    // `"$@"` finds out it is done, so it is an ordinary answer rather than a mistake: bash says
    // nothing about it by default, and says so under `--posix` or `shopt -s shift_verbose`. oslo
    // printed unconditionally, so `while [ $# -gt 0 ]; do …; shift 2; done` on an odd number of
    // arguments wrote a line of stderr on its way out — and `shopt` reported `shift_verbose`
    // permanently *off* while the shell behaved as though it were permanently on.
    if n > pos.len() {
        if env.posix() {
            crate::env::complain(
                args,
                &n.to_string(),
                &format!("shift: {n}: shift count out of range"),
                "more than there are",
                Some(&format!(
                    "there {} {} positional parameter{}",
                    if pos.len() == 1 { "is" } else { "are" },
                    pos.len(),
                    if pos.len() == 1 { "" } else { "s" }
                )),
            );
        }
        return Ok(1);
    }

    env.set_positional(pos[n..].to_vec());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::builtin_set;
    use crate::env::Environment;
    use crate::env::builtins::variables::tests::words;

    /// `set`'s status, whether it came back as one or as the utility error a special builtin
    /// raises so that POSIX mode can end the shell over it. Both carry the same number, and
    /// which one it is belongs to `crate::exec::simple::posix` rather than to these tests.
    fn set(env: &mut Environment, parts: &[&str]) -> i32 {
        let mut argv = vec!["set".to_string()];
        argv.extend(words(parts));
        match builtin_set(env, &argv) {
            Ok(status) => status,
            Err(e) => e
                .survivable_utility_status()
                .expect("set only ever fails with a utility error"),
        }
    }

    /// The finding: every one of these words used to become a positional parameter.
    #[test]
    fn options_are_options_and_not_positionals() {
        let mut env = Environment::new();
        env.set_positional(words(&["keep", "me"]));
        assert_eq!(set(&mut env, &["-euo", "pipefail"]), 0);
        assert!(env.errexit() && env.nounset() && env.pipefail());
        assert_eq!(env.get_positional(), &words(&["keep", "me"])[..]);
        // `h` is on by default, the way bash has it — see `env::options::ShellOptions::default`.
        assert_eq!(env.get_param("-").as_deref(), Some("ehu"));
    }

    #[test]
    fn operands_after_the_options_are_the_new_positionals() {
        let mut env = Environment::new();
        assert_eq!(set(&mut env, &["-x", "a", "b"]), 0);
        assert!(env.xtrace());
        assert_eq!(env.get_positional(), &words(&["a", "b"])[..]);

        // `--` protects an operand that looks like an option.
        assert_eq!(set(&mut env, &["--", "-e"]), 0);
        assert_eq!(env.get_positional(), &words(&["-e"])[..]);
        assert!(!env.errexit());
    }

    #[test]
    fn plus_clears_an_option() {
        let mut env = Environment::new();
        set(&mut env, &["-e"]);
        assert!(env.errexit());
        set(&mut env, &["+e"]);
        assert!(!env.errexit());
        set(&mut env, &["-o", "noclobber"]);
        assert!(env.noclobber());
        set(&mut env, &["+o", "noclobber"]);
        assert!(!env.noclobber());
    }

    #[test]
    fn an_unknown_option_is_a_usage_error_worth_two() {
        let mut env = Environment::new();
        // A *utility* error, not a status: `set` is a special builtin, so `bash --posix -c
        // 'set -z; echo alive'` prints no `alive`. The distinction is the return type.
        let err = builtin_set(&mut env, &words(&["set", "-z"])).expect_err("a utility error");
        assert_eq!(err.survivable_utility_status(), Some(2));

        assert_eq!(set(&mut env, &["-e", "-z"]), 2);
        // Nothing was applied: the whole line is refused, not the part after the mistake.
        assert!(!env.errexit());
        assert_eq!(set(&mut env, &["-o", "nosuchthing"]), 2);
        assert_eq!(set(&mut env, &["-i"]), 2);
    }

    #[test]
    fn set_with_no_arguments_still_lists_variables() {
        let mut env = Environment::new();
        env.set_positional(words(&["a"]));
        assert_eq!(set(&mut env, &[]), 0);
        assert_eq!(env.get_positional(), &words(&["a"])[..]);
    }
}
