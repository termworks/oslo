//! `read`: the option grammar, and what the pieces it selects are wired to.

use super::read_input::{InputSpec, is_terminal, probe_readable, read_logical_line, status_of};
use super::read_split::{all_fields, assign_fields};
use crate::env::origin_now;
use crate::env::scope::{Environment, ShellArray};
use oslo_base::error::Result;
use std::os::fd::RawFd;

/// Everything the option run decided.
struct ReadOptions {
    raw: bool,
    silent: bool,
    prompt: Option<String>,
    /// `-n`/`-N`: a character budget.
    limit: Option<usize>,
    /// `-N`: read exactly `limit` characters, ignoring delimiters *and* field splitting.
    exact: bool,
    timeout: Option<f64>,
    /// `-d`: the byte that ends the line. `-d ''` selects NUL, as in bash.
    delim: u8,
    fd: RawFd,
    array: Option<String>,
    names: Vec<String>,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            raw: false,
            silent: false,
            prompt: None,
            limit: None,
            exact: false,
            timeout: None,
            delim: b'\n',
            fd: 0,
            array: None,
            names: Vec::new(),
        }
    }
}

/// Why option parsing stopped early, and the status bash leaves behind for it.
///
/// bash separates the two: an option it does not have is a usage error (2), while an option
/// whose argument does not parse is an ordinary failure (1).
#[derive(Debug)]
struct OptionError {
    /// The word the caret goes under. Empty when the mistake is not one word — nothing is drawn
    /// then, and the one-liner is what a person gets, which is what it always was.
    word: String,
    message: String,
    status: i32,
}

/// The option letters this `read` has, printed under a usage error. Unprefixed, as bash leaves
/// its own — the line above it has already said where the error came from.
const USAGE: &str = "read: usage: read [-ers] [-a array] [-d delim] [-i text] [-n nchars] [-N nchars] \
     [-p prompt] [-t timeout] [-u fd] [name ...]";

fn usage(word: impl Into<String>, message: String) -> OptionError {
    OptionError {
        word: word.into(),
        message,
        status: 2,
    }
}

fn invalid(word: impl Into<String>, message: String) -> OptionError {
    OptionError {
        word: word.into(),
        message,
        status: 1,
    }
}

/// Which option letters consume a value, either as the rest of their cluster or as the next
/// argument. `read -n3`, `read -n 3` and `read -rn3` all mean the same thing.
fn takes_argument(flag: char) -> bool {
    matches!(flag, 'p' | 'n' | 'N' | 't' | 'd' | 'u' | 'a' | 'i')
}

/// Pull the value of `flag` out of the cluster remainder, or out of the next argument.
fn argument_for(
    flag: char,
    rest: &str,
    args: &[String],
    idx: &mut usize,
) -> std::result::Result<String, OptionError> {
    if !rest.is_empty() {
        return Ok(rest.to_string());
    }
    *idx += 1;
    args.get(*idx).cloned().ok_or_else(|| {
        usage(
            format!("-{flag}"),
            format!("-{flag}: option requires an argument"),
        )
    })
}

fn parse_number<T: std::str::FromStr>(
    flag: char,
    value: &str,
) -> std::result::Result<T, OptionError> {
    value
        .parse::<T>()
        .map_err(|_| invalid(value, format!("-{flag}: {value}: invalid number")))
}

/// `-t`'s argument: seconds, which must be a finite non-negative number.
///
/// A negative or non-finite deadline has no meaning — it cannot be waited for and it cannot be
/// probed — so bash rejects it rather than rounding it to zero, and so does oslo. Clamping it to
/// an immediate probe would turn a typo into a silently different read.
fn timeout_seconds(value: &str) -> std::result::Result<f64, OptionError> {
    match value.parse::<f64>() {
        Ok(secs) if secs.is_finite() && secs >= 0.0 => Ok(secs),
        _ => Err(invalid(
            value,
            format!("-t: {value}: invalid timeout specification"),
        )),
    }
}

/// Apply one option letter, returning the unconsumed remainder of its cluster.
fn apply_flag<'a>(
    opts: &mut ReadOptions,
    flag: char,
    rest: &'a str,
    args: &[String],
    idx: &mut usize,
) -> std::result::Result<&'a str, OptionError> {
    if !takes_argument(flag) {
        match flag {
            'r' => opts.raw = true,
            's' => opts.silent = true,
            // Readline editing options. oslo reads a descriptor, not a line editor, so they
            // change nothing — but rejecting them would break scripts that pass them harmlessly.
            'e' | 'E' => {}
            _ => {
                return Err(usage(
                    format!("-{flag}"),
                    format!("-{flag}: invalid option"),
                ));
            }
        }
        return Ok(rest);
    }

    let value = argument_for(flag, rest, args, idx)?;
    match flag {
        'p' => opts.prompt = Some(value),
        'a' => opts.array = Some(value),
        'n' => opts.limit = Some(parse_number('n', &value)?),
        'N' => {
            opts.limit = Some(parse_number('N', &value)?);
            opts.exact = true;
        }
        't' => opts.timeout = Some(timeout_seconds(&value)?),
        'u' => opts.fd = parse_number('u', &value)?,
        // `-d ''` is bash's way of spelling a NUL delimiter, which is what `find -print0` needs.
        'd' => opts.delim = value.bytes().next().unwrap_or(0),
        // `-i text` is readline's initial text, which a read from a descriptor ignores.
        'i' => {}
        _ => unreachable!("takes_argument admits only the letters handled here"),
    }
    Ok("")
}

fn parse_options(args: &[String]) -> std::result::Result<ReadOptions, OptionError> {
    let mut opts = ReadOptions::default();
    let mut idx = 1;
    while idx < args.len() {
        let arg = args[idx].as_str();
        if arg == "--" {
            idx += 1;
            break;
        }
        // A bare `-`, and anything not starting with one, is the first name.
        let Some(cluster) = arg.strip_prefix('-').filter(|c| !c.is_empty()) else {
            break;
        };

        for (offset, flag) in cluster.char_indices() {
            let rest = &cluster[offset + flag.len_utf8()..];
            let remainder = apply_flag(&mut opts, flag, rest, args, &mut idx)?;
            if remainder.len() != rest.len() {
                break;
            }
        }
        idx += 1;
    }
    opts.names = args[idx..].to_vec();
    Ok(opts)
}

/// Assign a line that was never split: `-N`, and the nameless `REPLY` case.
///
/// `-N` reads a fixed number of characters, so there is no delimiter to have split on and no
/// trailing whitespace that a delimiter would have removed — the text is assigned as it stands.
fn assign_verbatim(env: &mut Environment, names: &[String], text: &str) {
    match names.split_first() {
        Some((first, rest)) => {
            env.set_var(first, text, false);
            for name in rest {
                env.set_var(name, "", false);
            }
        }
        None => {
            env.set_var("REPLY", text, false);
        }
    }
}

/// `read [-rs] [-p prompt] [-n N] [-N N] [-t sec] [-d delim] [-u fd] [-a array] [name…]`
///
/// One logical line of input, split across `name…` on `$IFS` or left whole in `REPLY`.
///
/// The status is the only thing that ever stops `while read`: 1 when input ran out before the
/// delimiter arrived — even though the data that did arrive is still assigned — and 128 + SIGALRM
/// when `-t` expired.
pub fn builtin_read(env: &mut Environment, args: &[String]) -> Result<i32> {
    let opts = match parse_options(args) {
        Ok(opts) => opts,
        Err(err) => {
            // A usage error prints the usage under it, the way every other builtin here does and
            // the way bash does; an option whose *argument* was wrong is not a spelling problem
            // and a list of option letters would not help with it. The same split decides what the
            // caret is labelled and whether the report gets a help line at all.
            let spelling = err.status == 2;
            let drew = crate::env::complain(
                args,
                &err.word,
                &format!("read: {}", err.message),
                match spelling {
                    true => "not an option here",
                    false => "not a value this option takes",
                },
                spelling.then_some(USAGE),
            );
            if spelling && !drew {
                eprintln!("{USAGE}");
            }
            return Ok(err.status);
        }
    };

    // The prompt is a terminal courtesy: bash prints nothing when the input is a file or a pipe,
    // and a script that redirects `read` must not find the prompt in its output.
    if let Some(prompt) = &opts.prompt
        && is_terminal(opts.fd)
    {
        let _ = nix::unistd::write(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(2) },
            prompt.as_bytes(),
        );
    }

    // `-t 0` asks a question about the descriptor rather than reading it, so it answers before
    // any input is touched and assigns nothing either way.
    if opts.timeout == Some(0.0) {
        return Ok(i32::from(!probe_readable(opts.fd).unwrap_or(false)));
    }

    let spec = InputSpec {
        fd: opts.fd,
        raw: opts.raw,
        // `-N` reads through delimiters; only a count or EOF stops it.
        delim: if opts.exact { None } else { Some(opts.delim) },
        limit: opts.limit,
        timeout: opts.timeout,
        silent: opts.silent,
    };
    let line = match read_logical_line(&spec) {
        Ok(line) => line,
        Err(err) => {
            eprintln!("{}read: {}: {err}", origin_now(), opts.fd);
            return Ok(1);
        }
    };

    if let Some(array) = &opts.array {
        // `-a` replaces the array wholesale — a shorter line must not leave the previous read's
        // tail behind — and any `name…` operands are ignored, as in bash.
        //
        // `-N` reads a fixed count *through* delimiters, so there is nothing to split on: the
        // text arrives as the single element bash leaves in `${array[0]}`.
        let fields = if opts.exact {
            let text = line.text();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![text]
            }
        } else {
            all_fields(env, &line)
        };
        if !env.set_array(array, ShellArray::from_values(fields)) {
            return Ok(1);
        }
    } else if opts.exact || opts.names.is_empty() {
        assign_verbatim(env, &opts.names, &line.text());
    } else {
        assign_fields(env, &opts.names, &line);
    }

    Ok(status_of(line.stop))
}

#[cfg(test)]
mod tests {
    use super::parse_options;

    fn names(args: &[&str]) -> Vec<String> {
        let argv: Vec<String> = std::iter::once("read".to_string())
            .chain(args.iter().map(|a| (*a).to_string()))
            .collect();
        parse_options(&argv).expect("options parse").names
    }

    #[test]
    fn options_are_not_variable_names() {
        assert_eq!(names(&["-r", "v"]), ["v"]);
        assert_eq!(names(&["-p", "prompt> ", "v"]), ["v"]);
        assert_eq!(names(&["-n", "3", "v"]), ["v"]);
        assert_eq!(names(&["-t", "5", "v"]), ["v"]);
        assert_eq!(names(&["-rs", "v"]), ["v"]);
    }

    #[test]
    fn a_clustered_argument_may_follow_its_letter_directly() {
        let argv: Vec<String> = ["read", "-rn3", "v"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let opts = parse_options(&argv).expect("options parse");
        assert!(opts.raw);
        assert_eq!(opts.limit, Some(3));
        assert_eq!(opts.names, ["v"]);
    }

    #[test]
    fn double_dash_ends_the_options() {
        assert_eq!(names(&["--", "-r"]), ["-r"]);
        // …and a bare `-` is not an option at all.
        assert_eq!(names(&["-"]), ["-"]);
    }

    #[test]
    fn dash_big_n_selects_exact_mode() {
        let argv: Vec<String> = ["read", "-N", "4"].iter().map(|s| s.to_string()).collect();
        let opts = parse_options(&argv).expect("options parse");
        assert!(opts.exact);
        assert_eq!(opts.limit, Some(4));
    }

    #[test]
    fn an_empty_delimiter_argument_means_nul() {
        let argv: Vec<String> = ["read", "-d", "", "v"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_options(&argv).expect("options parse").delim, 0);
    }

    #[test]
    fn dash_a_names_an_array_and_not_a_variable() {
        let argv: Vec<String> = ["read", "-a", "arr", "extra"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let opts = parse_options(&argv).expect("options parse");
        assert_eq!(opts.array.as_deref(), Some("arr"));
        // bash ignores operand names once `-a` is given; they must not become the array's name.
        assert_eq!(opts.names, ["extra"]);
    }

    #[test]
    fn a_timeout_must_be_a_finite_non_negative_number() {
        let reject = |value: &str| {
            let argv: Vec<String> = ["read", "-t", value, "v"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            parse_options(&argv).err().map(|e| e.status)
        };
        assert_eq!(reject("-1"), Some(1));
        assert_eq!(reject("inf"), Some(1));
        assert_eq!(reject("nan"), Some(1));
        // …but a fraction is the whole point of the option, and zero is the probe form.
        let accept = |value: &str| {
            let argv: Vec<String> = ["read", "-t", value, "v"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            parse_options(&argv).expect("options parse").timeout
        };
        assert_eq!(accept("0.5"), Some(0.5));
        assert_eq!(accept("0"), Some(0.0));
    }

    #[test]
    fn an_unknown_option_is_a_usage_error_and_a_bad_number_is_not() {
        let bad_opt: Vec<String> = ["read", "-Z"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse_options(&bad_opt).err().map(|e| e.status), Some(2));
        let bad_num: Vec<String> = ["read", "-n", "abc"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_options(&bad_num).err().map(|e| e.status), Some(1));
        let missing: Vec<String> = ["read", "-d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse_options(&missing).err().map(|e| e.status), Some(2));
    }
}
