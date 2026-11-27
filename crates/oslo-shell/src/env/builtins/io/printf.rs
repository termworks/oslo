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
//! Everything is bytes from the arguments to the output, so a byte that is not UTF-8 goes out as
//! the byte it is.

mod args;
mod float;
mod format;
mod out;
mod quote;
mod render;
mod time;

use crate::env::origin_now;
use crate::env::scope::Environment;
use format::{Printer, Stop};
use oslo_base::error::Result;
use oslo_base::lossless;
pub use quote::shell_quote;

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
        && crate::env::builtins::arrays::parse_target(name).is_none()
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
    if format.is_empty() {
        if let Some(name) = into {
            env.set_var(name, "", false);
        }
        return Ok(0);
    }
    let format = lossless::decode(format);
    let arguments: Vec<Vec<u8>> = operands[1..].iter().map(|a| lossless::decode(a)).collect();
    let utf8 = super::utf8_locale(env);
    let mut printer = Printer {
        args: args::Args::new(&arguments, utf8).with_words(args, &operands[1..]),
        out: out::Out::new(into.is_some()),
        utf8,
        posix: env.posix(),
        counts: Vec::new(),
    };

    // The format is reused while arguments remain and each pass consumes at least one; one pass
    // always happens. A pass that stops early returns its own status.
    let status = loop {
        printer.out.pass = 0;
        let before = printer.args.next;
        if let Err(Stop(code)) = printer.pass(&format) {
            break code;
        }
        let next = printer.args.next;
        if next >= printer.args.len() || next == before {
            break i32::from(printer.args.error);
        }
    };

    for (name, count) in std::mem::take(&mut printer.counts) {
        env.set_var(&name, &count.to_string(), false);
    }

    // `-v` keeps the bytes up to the first NUL, as a C string would.
    if let Some(name) = into {
        let mut bytes = printer.out.into_kept();
        if let Some(nul) = bytes.iter().position(|&b| b == 0) {
            bytes.truncate(nul);
        }
        let value = lossless::encode(std::os::unix::ffi::OsStrExt::from_bytes(&bytes));
        return match crate::env::builtins::arrays::assign_target(env, name, &value) {
            Ok(true) => Ok(status),
            Ok(false) => Ok(1),
            Err(e) => {
                eprintln!("{}printf: {e}", origin_now());
                Ok(1)
            }
        };
    }

    // A failed write outranks the formatting status.
    printer.out.flush();
    Ok(printer.out.failed.unwrap_or(status))
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
