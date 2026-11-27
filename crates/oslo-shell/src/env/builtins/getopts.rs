//! `getopts` — POSIX option parsing, one option per call.
//!
//! The awkward part of `getopts` is not the parsing, it is the *state*: a call has to resume
//! where the previous one stopped, and the only part of that state POSIX exposes is `OPTIND`, the
//! index of the next word. Clustered options (`-abc`) need a second, hidden cursor — how far into
//! the current word we got — which no variable holds.
//!
//! That cursor lives here, in [`CURSOR`], and is validated against the world on every call: it is
//! only trusted when both the `OPTIND` the shell holds and the word it points at are the ones the
//! previous call left behind. A script that assigns `OPTIND=1` to restart parsing (the documented
//! way to parse a second argument list, and what every `getopts`-using function does with its own
//! `"$@"`) therefore gets a fresh cursor, and one that never touches `OPTIND` keeps its place
//! inside a cluster.
//!
//! The one case that check cannot see is `OPTIND` being assigned *the value it already had* while
//! the previous call stopped mid-cluster, over an identical argument list — `getopts ab o` on
//! `-ab`, then `OPTIND=1`, then `getopts` again. bash hooks assignment to `OPTIND` itself and
//! restarts; oslo only ever sees the value, which did not change, so it resumes at `b`. Detecting
//! it would need the variable layer to report writes, which is a change to `Environment`
//! ([`crate::env::scope`]) rather than to this builtin.

use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::cell::RefCell;

/// Where the last `getopts` call stopped.
struct Cursor {
    /// The `OPTIND` value that call wrote. A different one now means the script moved it.
    optind: usize,
    /// How far into the current word it got, counted in characters from the leading `-`;
    /// 0 means "this word has not been started yet".
    offset: usize,
    /// The word the offset indexes into. A different one now means the argument list changed
    /// underneath us, which is the ordinary case of a function parsing its own `"$@"`.
    word: String,
}

thread_local! {
    static CURSOR: RefCell<Cursor> = const {
        RefCell::new(Cursor { optind: 1, offset: 0, word: String::new() })
    };
}

/// Reset the hidden cursor. Only needed by tests, which share a thread with each other.
#[cfg(test)]
fn reset_cursor() {
    CURSOR.with(|c| {
        *c.borrow_mut() = Cursor {
            optind: 1,
            offset: 0,
            word: String::new(),
        };
    });
}

/// The outcome of examining one option character.
enum Step {
    /// An option was produced; `name` should be set to this, and `getopts` returns 0.
    Found { opt: String, optarg: Option<String> },
    /// The option list is over; `name` is set to `?` and `getopts` returns 1.
    Done,
}

/// `getopts optstring name [args…]`.
pub fn builtin_getopts(env: &mut Environment, args: &[String]) -> Result<i32> {
    let (Some(optstring), Some(name)) = (args.get(1), args.get(2)) else {
        eprintln!(
            "{}getopts: usage: getopts optstring name [arg ...]",
            origin_now()
        );
        return Ok(2);
    };
    // With no operands `getopts` parses the positional parameters, which is what every
    // `while getopts "ab:" opt; do` in a script is relying on.
    let operands: Vec<String> = if args.len() > 3 {
        args[3..].to_vec()
    } else {
        env.get_positional().to_vec()
    };

    // A leading `:` is "silent error reporting": the caller wants `?` and `:` reported through
    // `name` and `OPTARG` rather than as messages on stderr.
    let silent = optstring.starts_with(':');
    let spec = if silent {
        &optstring[1..]
    } else {
        &optstring[..]
    };
    // `OPTERR=0` silences the diagnostics *without* switching to silent mode's reporting, so the
    // two are tracked separately: only silent mode changes what ends up in `name` and `OPTARG`.
    let print_errors = !silent && env.get_var("OPTERR") != Some("0");

    let mut optind = env
        .get_var("OPTIND")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(1);
    let current_word = operands.get(optind.saturating_sub(1)).cloned();
    let mut offset = CURSOR.with(|c| {
        let cursor = c.borrow();
        // Trust the saved offset only if nothing moved: the same `OPTIND`, pointing at the same
        // word. Either changing means the script restarted the scan, and resuming mid-word would
        // silently swallow the first option of the new list.
        let same_place =
            cursor.optind == optind && current_word.as_deref() == Some(cursor.word.as_str());
        if same_place { cursor.offset } else { 0 }
    });

    let step = scan(&operands, spec, &mut optind, &mut offset);

    CURSOR.with(|c| {
        *c.borrow_mut() = Cursor {
            optind,
            offset,
            word: operands
                .get(optind.saturating_sub(1))
                .cloned()
                .unwrap_or_default(),
        };
    });
    env.set_var("OPTIND", &optind.to_string(), false);

    match step {
        Step::Found { opt, optarg } => {
            // `?` with no argument is `?` named in the optstring, not an unknown option.
            let named = opt == "?" && optarg.is_none();
            match optarg {
                Some(value) => {
                    env.set_var("OPTARG", &value, false);
                }
                // bash leaves `OPTARG` unset for an option that takes no argument; a stale value
                // from the previous option would otherwise look like this option's.
                None => env.unset_var("OPTARG"),
            }
            // A name that cannot be assigned is the builtin's failure, after the scan moved on.
            Ok(report(&opt, name, silent, print_errors && !named, env))
        }
        Step::Done => {
            env.unset_var("OPTARG");
            bind(env, name, "?");
            Ok(1)
        }
    }
}

/// Assign the option to `name` once the scan has moved on, as bash does: a name that is not one
/// is refused with 1, a read-only one with 2.
fn bind(env: &mut Environment, name: &str, value: &str) -> i32 {
    if !crate::env::scope::is_valid_identifier(name) {
        crate::env::complain(
            &crate::env::line("getopts", std::slice::from_ref(&name.to_string())),
            name,
            &format!(
                "getopts: `{}': not a valid identifier",
                oslo_base::shown::shown(name)
            ),
            "not a name",
            None,
        );
        return 1;
    }
    match env.set_var(name, value, false) {
        true => 0,
        false => 2,
    }
}

/// Set `name`, and complain on stderr unless the caller asked for silence.
///
/// The two error outcomes travel as the option characters `?` (unknown option) and `:` (an option
/// that needs an argument and did not get one). In silent mode they reach the script through
/// `name`/`OPTARG` and nothing is printed; otherwise `name` is always `?` and the message is what
/// the script's user sees. Answers the builtin's status, which [`bind`] decides.
fn report(opt: &str, name: &str, silent: bool, print_errors: bool, env: &mut Environment) -> i32 {
    let message = match opt {
        "?" => Some("illegal option"),
        ":" if !silent => Some("option requires an argument"),
        _ => None,
    };

    if let Some(text) = message {
        if print_errors {
            let bad = env.get_var("OPTARG").unwrap_or_default().to_string();
            eprintln!("{}getopts: {} -- {}", origin_now(), text, bad);
        }
        // Outside silent mode the offending character is *not* left in OPTARG: a script reading
        // it would mistake it for a real option argument. Silent mode is the opposite — reporting
        // through OPTARG is how it tells the caller which option was at fault.
        if !silent {
            env.unset_var("OPTARG");
        }
    }

    // Silent mode is the only way a script can see `:`; otherwise both errors surface as `?`.
    let value = match opt {
        ":" if !silent => "?",
        other => other,
    };
    bind(env, name, value)
}

/// Advance `optind`/`offset` over the argument list and classify the next option character.
fn scan(operands: &[String], spec: &str, optind: &mut usize, offset: &mut usize) -> Step {
    loop {
        let Some(word) = operands.get(optind.saturating_sub(1)) else {
            return Step::Done;
        };
        let chars: Vec<char> = word.chars().collect();

        if *offset == 0 {
            // A word that is not an option ends the option list, and so does `-` on its own.
            if chars.first() != Some(&'-') || chars.len() < 2 {
                return Step::Done;
            }
            if word == "--" {
                // The explicit end-of-options marker is consumed; `OPTIND` must point past it so
                // `shift $((OPTIND - 1))` drops it too.
                *optind += 1;
                return Step::Done;
            }
            *offset = 1;
        }

        if *offset >= chars.len() {
            // Exhausted a cluster (`-ab`); move to the next word and start over.
            *optind += 1;
            *offset = 0;
            continue;
        }

        let opt = chars[*offset];
        *offset += 1;
        let at_word_end = *offset >= chars.len();
        if at_word_end {
            *optind += 1;
            *offset = 0;
        }

        // `:` is never an option character itself — it is the marker that the *previous* one takes
        // an argument — so asking for it is always an unknown option.
        let wants_argument = match takes_argument(spec, opt) {
            Some(wants) if opt != ':' => wants,
            _ => {
                return Step::Found {
                    opt: "?".to_string(),
                    optarg: Some(opt.to_string()),
                };
            }
        };

        if !wants_argument {
            return Step::Found {
                opt: opt.to_string(),
                optarg: None,
            };
        }

        // The argument is the rest of this word if there is one (`-bvalue`), otherwise the whole
        // of the next word (`-b value`).
        if !at_word_end {
            let value: String = chars[*offset..].iter().collect();
            *optind += 1;
            *offset = 0;
            return Step::Found {
                opt: opt.to_string(),
                optarg: Some(value),
            };
        }
        match operands.get(optind.saturating_sub(1)) {
            Some(value) => {
                let value = value.clone();
                *optind += 1;
                return Step::Found {
                    opt: opt.to_string(),
                    optarg: Some(value),
                };
            }
            None => {
                return Step::Found {
                    opt: ":".to_string(),
                    optarg: Some(opt.to_string()),
                };
            }
        }
    }
}

/// `Some(true)` if `opt` is in `spec` and takes an argument, `Some(false)` if it takes none,
/// `None` if `spec` does not mention it at all.
fn takes_argument(spec: &str, opt: char) -> Option<bool> {
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // `::` after an option is a GNU extension for an optional argument; POSIX only has one
        // level, so any run of colons is collapsed into "takes an argument".
        let mut colons = 0;
        while chars.get(i + 1 + colons) == Some(&':') {
            colons += 1;
        }
        if c != ':' && c == opt {
            return Some(colons > 0);
        }
        i += 1 + colons;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{builtin_getopts, reset_cursor, takes_argument};
    use crate::env::Environment;

    /// Drive one `getopts` call the way a script does, and report `(status, opt, OPTARG, OPTIND)`.
    fn step(env: &mut Environment, spec: &str) -> (i32, String, Option<String>, String) {
        let args = vec!["getopts".to_string(), spec.to_string(), "opt".to_string()];
        let status = builtin_getopts(env, &args).expect("getopts");
        (
            status,
            env.get_var("opt").unwrap_or_default().to_string(),
            env.get_var("OPTARG").map(str::to_string),
            env.get_var("OPTIND").unwrap_or_default().to_string(),
        )
    }

    fn env_with(argv: &[&str]) -> Environment {
        reset_cursor();
        let mut env = Environment::new();
        env.set_positional(argv.iter().map(|s| s.to_string()).collect());
        env.set_var("OPTIND", "1", false);
        env.unset_var("OPTARG");
        env.set_var("OPTERR", "0", false);
        env
    }

    #[test]
    fn option_spec_is_read_correctly() {
        assert_eq!(takes_argument("ab:c", 'a'), Some(false));
        assert_eq!(takes_argument("ab:c", 'b'), Some(true));
        assert_eq!(takes_argument("ab:c", 'c'), Some(false));
        assert_eq!(takes_argument("ab:c", 'z'), None);
        assert_eq!(takes_argument("ab:c", ':'), None);
    }

    /// The corpus loop: a flag, an option with a separate argument, `--`, then the operands.
    #[test]
    fn a_full_option_loop_matches_the_shape_scripts_expect() {
        let mut env = env_with(&["-a", "-b", "value", "--", "rest"]);

        assert_eq!(step(&mut env, "ab:").0, 0);
        assert_eq!(env.get_var("opt"), Some("a"));

        let (status, opt, optarg, _) = step(&mut env, "ab:");
        assert_eq!(
            (status, opt.as_str(), optarg.as_deref()),
            (0, "b", Some("value"))
        );

        let (status, opt, _, optind) = step(&mut env, "ab:");
        assert_eq!((status, opt.as_str()), (1, "?"));
        // `shift $((OPTIND - 1))` must drop `-a -b value --` and leave `rest`.
        assert_eq!(optind, "5");
    }

    /// `-ab` is two options in one word, and `OPTIND` must not advance until the word is spent.
    #[test]
    fn clustered_flags_share_a_word() {
        let mut env = env_with(&["-ab", "x"]);

        let (_, opt, _, optind) = step(&mut env, "ab");
        assert_eq!((opt.as_str(), optind.as_str()), ("a", "1"));
        let (_, opt, _, optind) = step(&mut env, "ab");
        assert_eq!((opt.as_str(), optind.as_str()), ("b", "2"));
        assert_eq!(step(&mut env, "ab").0, 1);
    }

    /// An argument glued to its option (`-bvalue`) is the same as a separate one.
    #[test]
    fn a_glued_argument_is_accepted() {
        let mut env = env_with(&["-bvalue", "rest"]);
        let (status, opt, optarg, optind) = step(&mut env, "b:");
        assert_eq!(
            (status, opt.as_str(), optarg.as_deref()),
            (0, "b", Some("value"))
        );
        assert_eq!(optind, "2");
    }

    /// An unknown option is reported as `?` with the offending character in `OPTARG` (silent
    /// mode), and parsing carries on rather than stopping.
    #[test]
    fn an_unknown_option_is_reported_and_parsing_continues() {
        let mut env = env_with(&["-z", "-a"]);
        let (status, opt, optarg, _) = step(&mut env, ":a");
        assert_eq!(
            (status, opt.as_str(), optarg.as_deref()),
            (0, "?", Some("z"))
        );
        let (status, opt, _, _) = step(&mut env, ":a");
        assert_eq!((status, opt.as_str()), (0, "a"));
    }

    /// A missing argument is `:` in silent mode, with the option character in `OPTARG`.
    #[test]
    fn a_missing_argument_is_distinguishable_in_silent_mode() {
        let mut env = env_with(&["-b"]);
        let (status, opt, optarg, _) = step(&mut env, ":b:");
        assert_eq!(
            (status, opt.as_str(), optarg.as_deref()),
            (0, ":", Some("b"))
        );
    }

    /// The first non-option word ends the scan, leaving `OPTIND` pointing at it.
    #[test]
    fn a_non_option_word_ends_the_scan() {
        let mut env = env_with(&["-a", "file", "-b"]);
        assert_eq!(step(&mut env, "ab").0, 0);
        let (status, opt, _, optind) = step(&mut env, "ab");
        assert_eq!((status, opt.as_str(), optind.as_str()), (1, "?", "2"));
    }

    /// `OPTIND=1` before a second argument list is what every `getopts`-using function does with
    /// its own `"$@"`. The hidden intra-word cursor has to be dropped with it, or the first option
    /// of the new list is silently skipped.
    #[test]
    fn a_second_argument_list_restarts_the_scan() {
        let mut env = env_with(&["-ab"]);
        assert_eq!(step(&mut env, "ab").1, "a");

        env.set_positional(vec!["-cd".to_string()]);
        env.set_var("OPTIND", "1", false);
        assert_eq!(step(&mut env, "cd").1, "c");
    }

    /// The same, over the *same* list: rewinding `OPTIND` after a word has been fully consumed
    /// starts the scan again rather than resuming where it left off.
    #[test]
    fn rewinding_optind_over_the_same_list_starts_over() {
        let mut env = env_with(&["-a", "-b"]);
        assert_eq!(step(&mut env, "ab").1, "a");
        env.set_var("OPTIND", "1", false);
        assert_eq!(step(&mut env, "ab").1, "a");
    }
}
