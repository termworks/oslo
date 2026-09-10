//! `printf` — the one output builtin that can produce exactly the bytes asked for.
//!
//! Built in rather than left to coreutils because a distro's `/bin/sh` runs before coreutils is on
//! the filesystem: an initramfs, an early boot script and a `Makefile` recipe in a stage-0 chroot
//! all call `printf` with nothing but the shell available. bash, dash and busybox all build it in
//! for the same reason. It is also the only portable way to emit a string without a trailing
//! newline, since `echo -n` is not POSIX.
//!
//! Two escape passes, which is the part that surprises people:
//!
//! * the **format** always has its backslash escapes decoded, so `printf 'a\nb'` prints two lines;
//! * an **argument** does not, unless it is consumed by `%b`, which is what `%b` is for.
//!
//! Both share `echo`'s decoder, so `\0ddd`, `\xHH` and the control escapes mean the same thing in
//! every builtin that writes bytes.

mod format;

use crate::env::origin_now;
use crate::env::scope::Environment;
use format::render;
pub use format::shell_quote;
use oslo_base::error::Result;

/// The usage line, under the diagnostic that caused it and unprefixed, as bash leaves its own.
const USAGE: &str = "printf: usage: printf [-v var] format [arguments]";

/// `printf [-v NAME] FORMAT [ARGUMENT]...`
pub fn builtin_printf(env: &mut Environment, args: &[String]) -> Result<i32> {
    let (into, operands) = match parse(&args[1..]) {
        Parsed::Options(into, operands) => (into, operands),
        Parsed::Usage(message) => {
            eprintln!("{}printf: {message}", origin_now());
            eprintln!("{USAGE}");
            return Ok(2);
        }
    };

    if let Some(name) = into
        && !crate::env::scope::is_valid_identifier(name)
    {
        crate::env::complain(
            args,
            name,
            &format!(
                "printf: `{}': not a valid identifier",
                oslo_base::shown::shown(name)
            ),
            "not a name",
            Some(
                "a name starts with a letter or underscore and continues with letters, digits or underscores",
            ),
        );
        return Ok(2);
    }

    let Some(format) = operands.first() else {
        eprintln!("{USAGE}");
        return Ok(2);
    };
    let arguments = &operands[1..];

    let mut out: Vec<u8> = Vec::new();
    let mut status = 0;
    let mut next = 0;

    // POSIX: the format is reused until the arguments run out. `printf '%s\n' a b c` prints three
    // lines. One pass always happens, even with no arguments at all.
    loop {
        let consumed_before = next;
        match render(format, arguments, &mut next, &mut out) {
            Ok(()) => {}
            Err(code) => status = code,
        }
        // Stop unless there are arguments left *and* this pass used at least one: a format with no
        // conversions would otherwise repeat for ever.
        if next >= arguments.len() || next == consumed_before {
            break;
        }
    }

    // `-v` puts the result in a variable rather than on stdout, which is the whole reason scripts
    // reach for it: building a string with `%s`/`%d` padding without a subshell to capture it.
    if let Some(name) = into {
        env.set_var(name, &String::from_utf8_lossy(&out), false);
        return Ok(status);
    }

    // A write failure outranks a formatting one: if the output never arrived, the status has to
    // say so whatever the format string did.
    match super::write_stdout("printf", &out) {
        0 => Ok(status),
        failed => Ok(failed),
    }
}

/// What the leading operands turned out to be.
enum Parsed<'a> {
    /// The `-v` name, if one was given, and the operands after the options.
    Options(Option<&'a str>, &'a [String]),
    Usage(String),
}

/// Split the options off the front.
///
/// **An option this does not know is an error, not a format string.** `printf -Z` used to print
/// `-Z` and report success, and `printf -v out '%s' hi` — an idiom common enough that scripts
/// written for bash use it freely — printed `-v` and left `out` untouched. Both are the same
/// mistake: a word beginning with `-` was never examined before being used as the format.
fn parse(operands: &[String]) -> Parsed<'_> {
    let mut into: Option<&str> = None;
    let mut rest = operands;

    while let Some(word) = rest.first() {
        // A lone `-` is a format that prints a dash, and `--` ends the options — the defensive
        // spelling `printf -- '%s\n' x` that scripts use to guard against exactly this parser.
        if word == "--" {
            rest = &rest[1..];
            break;
        }
        if !word.starts_with('-') || word == "-" {
            break;
        }
        let Some(attached) = word.strip_prefix("-v") else {
            return Parsed::Usage(format!("{word}: invalid option"));
        };
        if attached.is_empty() {
            let Some(name) = rest.get(1) else {
                return Parsed::Usage("-v: option requires an argument".to_string());
            };
            into = Some(name);
            rest = &rest[2..];
        } else {
            // `-vout`, which bash accepts as readily as `-v out`.
            into = Some(attached);
            rest = &rest[1..];
        }
    }

    Parsed::Options(into, rest)
}

#[cfg(test)]
mod tests {
    use super::{Parsed, builtin_printf, parse};
    use crate::env::scope::Environment;

    fn words(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn split(parts: &[&str]) -> Parsed<'static> {
        // Leaked so the borrow outlives the call; a test's allocation, not the shell's.
        let operands: &'static [String] = Box::leak(words(parts).into_boxed_slice());
        parse(operands)
    }

    /// **The bug.** A word starting with `-` was used as the format without ever being looked
    /// at, so `printf -Z` printed `-Z` and reported success.
    #[test]
    fn an_unknown_option_is_refused_rather_than_printed() {
        let mut env = Environment::new();
        for bad in ["-Z", "-x", "--nosuch"] {
            assert_eq!(
                builtin_printf(&mut env, &words(&["printf", bad])).unwrap(),
                2,
                "{bad}"
            );
        }
    }

    /// `-v NAME` assigns instead of printing — the idiom that silently printed `-v` before.
    #[test]
    fn dash_v_assigns_the_result_to_a_variable() {
        let mut env = Environment::new();
        let args = words(&["printf", "-v", "out", "%s-%s", "a", "b"]);
        assert_eq!(builtin_printf(&mut env, &args).unwrap(), 0);
        assert_eq!(env.get_var("out"), Some("a-b"));
    }

    /// bash takes the name attached to the flag too, and scripts written for it use both.
    #[test]
    fn dash_v_takes_an_attached_name() {
        let mut env = Environment::new();
        let args = words(&["printf", "-vout", "%s", "hi"]);
        assert_eq!(builtin_printf(&mut env, &args).unwrap(), 0);
        assert_eq!(env.get_var("out"), Some("hi"));
    }

    /// A reused format collects into the variable whole, rather than the last pass only.
    #[test]
    fn a_reused_format_assigns_every_pass() {
        let mut env = Environment::new();
        let args = words(&["printf", "-v", "out", "%s\n", "a", "b"]);
        assert_eq!(builtin_printf(&mut env, &args).unwrap(), 0);
        assert_eq!(env.get_var("out"), Some("a\nb\n"));
    }

    #[test]
    fn dash_v_wants_a_name_and_a_usable_one() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_printf(&mut env, &words(&["printf", "-v"])).unwrap(),
            2
        );
        let args = words(&["printf", "-v", "1bad", "%s", "hi"]);
        assert_eq!(builtin_printf(&mut env, &args).unwrap(), 2);
        assert!(env.get_var("1bad").is_none());
    }

    /// The two spellings that must keep meaning a format: `--` ends the options, and a lone
    /// `-` is a format that prints a dash.
    #[test]
    fn a_double_dash_and_a_lone_dash_are_not_options() {
        assert!(
            matches!(split(&["--", "%s", "x"]), Parsed::Options(None, rest) if rest.len() == 2)
        );
        assert!(matches!(split(&["-"]), Parsed::Options(None, rest) if rest.len() == 1));
    }
}
