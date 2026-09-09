//! The format string itself: one `%` conversion at a time.
//!
//! Split from [`super`] because the two halves answer different questions and only one of them is
//! about the shell. The builtin decides what was asked for — which options, which variable, how
//! many times to reuse the format; this decides what `%#0*.3Lf` means, which is a question about
//! C rather than about `printf`, and is most of the code either way.

use crate::env::builtins::io::echo::expand_escapes;
use crate::env::origin_now;

/// One pass over the format. `next` indexes the next unconsumed argument.
pub fn render(
    format: &str,
    args: &[String],
    next: &mut usize,
    out: &mut Vec<u8>,
) -> std::result::Result<(), i32> {
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    let mut status = Ok(());

    while i < chars.len() {
        if chars[i] != '%' {
            // Literal text, escapes and all. Collected in one run so the decoder sees whole
            // sequences rather than a backslash at a time.
            let start = i;
            while i < chars.len() && chars[i] != '%' {
                i += 1;
            }
            let literal: String = chars[start..i].iter().collect();
            let (bytes, _) = expand_escapes(&literal);
            out.extend_from_slice(&bytes);
            continue;
        }

        // `%%` is a literal percent and consumes no argument.
        if i + 1 < chars.len() && chars[i + 1] == '%' {
            out.push(b'%');
            i += 2;
            continue;
        }

        let Some(spec) = Spec::parse(&chars, &mut i) else {
            // **The format ran out before a conversion letter**, which is `printf '%'`,
            // `printf '%5'` or `printf '%z'` — `z` being a length modifier, so it too is waiting
            // for the letter that never came.
            //
            // It used to print the `%` and report success, on the note that "bash prints it and
            // carries on". bash does neither: it says `printf: '%z': missing format character` and
            // exits 1, and so does dash. A format string with a typo in it silently producing
            // near-enough output and a status of 0 is the failure this whole builtin's error
            // handling exists to avoid.
            let malformed: String = chars[i..].iter().collect();
            // The caret goes into the format string itself, which is the one place printf has a
            // real source: the malformed piece is its tail, so where it starts is what is left of
            // the string without it.
            let at = format.len().saturating_sub(malformed.len());
            crate::env::complain_within(
                &[String::from("printf"), format.to_string()],
                format,
                at..format.len(),
                &format!("printf: `{malformed}': missing format character"),
                "a conversion with no letter",
                Some(
                    "a `%` starts a conversion and needs a letter: %s %d %f %x %c %b %q, or %% for a literal percent",
                ),
            );
            return Err(1);
        };

        // Each `*` eats an argument before the conversion's own, as C and every shell do.
        let mut sizes = Vec::new();
        for _ in 0..spec.star_count() {
            let value = args.get(*next).map(String::as_str).unwrap_or("0");
            if *next < args.len() {
                *next += 1;
            }
            // A width that is not a number is reported and treated as 0, exactly as an unreadable
            // *argument* is: the format itself is still valid, so the rest of the line is still
            // what the author asked for. bash and dash both say something here and carry on.
            sizes.push(parse_int(value).unwrap_or_else(|()| {
                eprintln!("{}printf: {value}: invalid number", origin_now());
                status = Err(1);
                0
            }));
        }
        let spec = spec.with_sizes(&sizes);

        let arg = args.get(*next).map(String::as_str).unwrap_or("");
        if *next < args.len() {
            *next += 1;
        }
        match spec.render(arg, out) {
            Ok(()) => {}
            // A bad *argument* is reported and the format carries on, printing 0 — bash does the
            // same, because the rest of the line is still what the author asked for.
            Err(Bad::Argument(code)) => status = Err(code),
            // A conversion letter that does not exist is a bug in the format itself, so nothing
            // further in it can be trusted; bash stops there and prints no more of the line.
            Err(Bad::Format(code)) => return Err(code),
        }
    }
    status
}

/// Why a conversion failed, which decides whether the rest of the format still runs.
enum Bad {
    /// The argument could not be read as the conversion wanted. Reported; the format continues.
    Argument(i32),
    /// The conversion letter does not exist. The format is wrong, so rendering stops.
    Format(i32),
}

/// A width or a precision: written out, or `*` meaning "the next argument says".
#[derive(Clone, Copy, PartialEq, Eq)]
enum Size {
    Fixed(usize),
    /// `%*d` and `%.*f`. The argument is consumed **before** the one being converted, which is the
    /// order C and every shell use — `printf '%*d' 5 42` is width 5, value 42.
    FromArgument,
}

/// One `%` conversion: its flags, width, precision and the letter that decides the type.
struct Spec {
    flags: String,
    width: Option<Size>,
    precision: Option<Size>,
    conversion: char,
}

impl Spec {
    /// How many arguments this conversion eats before its own: one per `*`.
    fn star_count(&self) -> usize {
        usize::from(self.width == Some(Size::FromArgument))
            + usize::from(self.precision == Some(Size::FromArgument))
    }

    /// Replace each `*` with the number that was read for it, in written order.
    ///
    /// A **negative** width means left-justify at that width, exactly as a `-` flag would — C says
    /// so, and `printf '%*s' -6 hi` is how a script asks for it without knowing the sign up front.
    fn with_sizes(mut self, sizes: &[i64]) -> Self {
        let mut next = sizes.iter().copied();
        if self.width == Some(Size::FromArgument) {
            let n = next.next().unwrap_or(0);
            if n < 0 {
                self.flags.push('-');
            }
            self.width = Some(Size::Fixed(n.unsigned_abs() as usize));
        }
        if self.precision == Some(Size::FromArgument) {
            // A negative precision means "no precision given" in C, not a precision of zero.
            self.precision = match next.next().unwrap_or(0) {
                n if n < 0 => None,
                n => Some(Size::Fixed(n as usize)),
            };
        }
        self
    }

    /// The width, once every `*` has been resolved.
    fn width(&self) -> Option<usize> {
        match self.width {
            Some(Size::Fixed(n)) => Some(n),
            _ => None,
        }
    }

    /// The precision, once every `*` has been resolved.
    fn precision(&self) -> Option<usize> {
        match self.precision {
            Some(Size::Fixed(n)) => Some(n),
            _ => None,
        }
    }

    /// Parse a conversion starting at `chars[*i] == '%'`, leaving `*i` just past it.
    fn parse(chars: &[char], i: &mut usize) -> Option<Self> {
        let mut j = *i + 1;
        let mut flags = String::new();
        while j < chars.len() && matches!(chars[j], '-' | '+' | ' ' | '#' | '0') {
            flags.push(chars[j]);
            j += 1;
        }
        // `*` takes the number from an argument. Required by every shell that scripts are written
        // against — `printf '%c %*u. %s\n'` is how `select-editor` lines its menu up — and without
        // it the `*` reached the conversion letter and the whole format was refused.
        let take_size = |j: &mut usize| -> Option<Size> {
            if chars.get(*j) == Some(&'*') {
                *j += 1;
                return Some(Size::FromArgument);
            }
            take_number(chars, j).map(Size::Fixed)
        };
        let width = take_size(&mut j);
        let precision = if j < chars.len() && chars[j] == '.' {
            j += 1;
            Some(take_size(&mut j).unwrap_or(Size::Fixed(0)))
        } else {
            None
        };
        // C's length modifiers are accepted and ignored: a shell has one integer type, so `%ld`
        // and `%d` cannot differ, and scripts written against C's printf pass them. Skipping them
        // is not cosmetic — bash reads `%zb` as `%b`, so treating `z` as the conversion letter
        // made a valid format an error. `q` is deliberately absent: in a shell it is a
        // *conversion*, not a modifier.
        while j < chars.len() && matches!(chars[j], 'h' | 'l' | 'L' | 'j' | 'z' | 't') {
            j += 1;
        }
        let conversion = *chars.get(j)?;
        *i = j + 1;
        Some(Self {
            flags,
            width,
            precision,
            conversion,
        })
    }

    fn render(&self, arg: &str, out: &mut Vec<u8>) -> std::result::Result<(), Bad> {
        let mut status = Ok(());
        let body: Vec<u8> = match self.conversion {
            's' => {
                let mut s = arg.to_string();
                if let Some(p) = self.precision() {
                    s.truncate(whole_characters(&s, p.min(s.len())));
                }
                s.into_bytes()
            }
            // bash's `%b`: the argument's escapes are decoded, which is the only way to get
            // `\n` out of *data* rather than out of the format.
            'b' => expand_escapes(arg).0,
            // bash's `%q`: quote the argument so the shell would read it back as this exact
            // string. What a script uses to build a command line out of untrusted data.
            'q' => shell_quote(arg).into_bytes(),
            'c' => arg
                .chars()
                .next()
                .map(String::from)
                .unwrap_or_default()
                .into_bytes(),
            'd' | 'i' => match parse_int(arg) {
                Ok(n) => self.signed(n.to_string()).into_bytes(),
                Err(()) => {
                    eprintln!("{}printf: {}: invalid number", origin_now(), arg);
                    status = Err(Bad::Argument(1));
                    b"0".to_vec()
                }
            },
            'u' => match parse_int(arg) {
                Ok(n) => (n as u64).to_string().into_bytes(),
                Err(()) => {
                    eprintln!("{}printf: {}: invalid number", origin_now(), arg);
                    status = Err(Bad::Argument(1));
                    b"0".to_vec()
                }
            },
            'o' | 'x' | 'X' => match parse_int(arg) {
                Ok(n) => {
                    let n = n as u64;
                    // `#` is C's alternate form: a leading `0` on octal, `0x`/`0X` on hex. It is
                    // how a script prints a number something else will read back as one, and it
                    // was parsed and then dropped — `printf '%#x' 255` gave `ff`.
                    let alt = self.flags.contains('#');
                    match self.conversion {
                        'o' => match alt && n != 0 {
                            true => format!("0{n:o}"),
                            false => format!("{n:o}"),
                        },
                        'x' => match alt && n != 0 {
                            true => format!("0x{n:x}"),
                            false => format!("{n:x}"),
                        },
                        _ => match alt && n != 0 {
                            true => format!("0X{n:X}"),
                            false => format!("{n:X}"),
                        },
                    }
                    .into_bytes()
                }
                Err(()) => {
                    eprintln!("{}printf: {}: invalid number", origin_now(), arg);
                    status = Err(Bad::Argument(1));
                    b"0".to_vec()
                }
            },
            'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
                let value: f64 = arg.trim().parse().unwrap_or_else(|_| {
                    if !arg.is_empty() {
                        eprintln!("{}printf: {}: invalid number", origin_now(), arg);
                        status = Err(Bad::Argument(1));
                    }
                    0.0
                });
                // **Rust's formatter takes a `u16` precision and panics above it.**
                // `printf '%.70000f' 1` aborted the shell with `Formatting argument out of range`;
                // bash prints the number. Clamped rather than refused: a precision past this is
                // already more digits than a `f64` can distinguish, so the answer is the same one.
                let p = self.precision().unwrap_or(6).min(u16::MAX as usize);
                let rendered = match self.conversion {
                    'e' => c_exponent(&format!("{:.*e}", p, value), 'e'),
                    'E' => c_exponent(&format!("{:.*E}", p, value), 'E'),
                    'g' | 'G' => self.general(value, p),
                    _ => format!("{:.*}", p, value),
                };
                self.signed(rendered).into_bytes()
            }
            other => {
                eprintln!(
                    "{}printf: `%{}': invalid format character",
                    origin_now(),
                    other
                );
                return Err(Bad::Format(1));
            }
        };

        out.extend_from_slice(&self.pad(body));
        status
    }

    /// Put back the sign the flags asked for.
    ///
    /// `+` prints one on a non-negative number and ` ` prints a space in its place, which is how a
    /// column of numbers is kept in line. Both were parsed and then ignored, so `printf '%+d' 5`
    /// gave `5`. `+` wins when both are given, as in C.
    fn signed(&self, rendered: String) -> String {
        if rendered.starts_with('-') {
            return rendered;
        }
        match (self.flags.contains('+'), self.flags.contains(' ')) {
            (true, _) => format!("+{rendered}"),
            (false, true) => format!(" {rendered}"),
            (false, false) => rendered,
        }
    }

    /// `%g`: whichever of `%e` and `%f` is the shorter honest answer, C's rule.
    ///
    /// It used to fall through to `%f`, so `printf '%g' 1e20` printed twenty-one digits and a
    /// fraction where C prints `1e+20` — the one conversion whose whole purpose is not to do that.
    ///
    /// The rule, from C99 7.19.6.1: with precision `p` (0 read as 1) and `x` the exponent `%e`
    /// would use, `%f` with precision `p - 1 - x` when `p > x >= -4`, and `%e` with precision
    /// `p - 1` otherwise. Trailing zeros come off unless `#` asked for them.
    fn general(&self, value: f64, precision: usize) -> String {
        let p = precision.max(1);
        let exponent = match value == 0.0 {
            true => 0,
            false => value.abs().log10().floor() as i32,
        };
        let wide = (p as i32) > exponent && exponent >= -4;
        let mut rendered = match wide {
            true => format!("{:.*}", (p as i32 - 1 - exponent).max(0) as usize, value),
            // Formatted in the case it will be printed in: `c_exponent` finds the marker in the
            // text, so asking it for `E` while writing `e` leaves the exponent unnormalised.
            false => match self.conversion == 'G' {
                true => c_exponent(&format!("{:.*E}", p - 1, value), 'E'),
                false => c_exponent(&format!("{:.*e}", p - 1, value), 'e'),
            },
        };
        if self.flags.contains('#') || !rendered.contains('.') {
            return rendered;
        }
        // Only the fraction's zeros, and only up to the exponent — `1.500000e+20` loses three
        // zeros and keeps its `e+20`.
        let (mantissa, tail) = match rendered.find(['e', 'E']) {
            Some(at) => (rendered[..at].to_string(), rendered[at..].to_string()),
            None => (rendered.clone(), String::new()),
        };
        let trimmed = mantissa.trim_end_matches('0').trim_end_matches('.');
        rendered = format!("{trimmed}{tail}");
        rendered
    }

    /// Apply width, with `-` for left and `0` for zero-fill.
    ///
    /// Zero-fill is ignored for `%s`, as in C: padding a string with zeros produces `000abc`,
    /// which no caller means.
    fn pad(&self, body: Vec<u8>) -> Vec<u8> {
        let Some(width) = self.width() else {
            return body;
        };
        if body.len() >= width {
            return body;
        }
        // **A width is a number somebody typed, and the padding for it is allocated.**
        // `printf '%9999999999s' x` asked for ten gigabytes of spaces; bash answers `Value too
        // large for defined data type` and carries on, and anything past a line of output is a
        // typo rather than a request. Reported once and clamped, so the format still runs.
        if width > WIDEST {
            eprintln!("{}printf: {width}: width is too large", origin_now());
            return body;
        }
        let fill = if self.flags.contains('0')
            && !self.flags.contains('-')
            && !matches!(self.conversion, 's' | 'b' | 'c')
        {
            b'0'
        } else {
            b' '
        };
        let padding = vec![fill; width - body.len()];
        if self.flags.contains('-') {
            let mut out = body;
            out.extend_from_slice(&padding);
            out
        } else {
            let mut out = padding;
            out.extend_from_slice(&body);
            out
        }
    }
}

/// Quote `text` so that reading it back as shell input yields `text` exactly.
///
/// Backslash-escaping rather than wrapping in single quotes. Both are correct shell and read back
/// the same, but bash prints `a\ b` where quoting would print `'a b'`, and the differential corpus
/// compares bytes — matching the form is what lets `%q` be covered by it at all.
///
/// A control character has no backslash spelling outside `$'...'`, which is where those go; a
/// string that needs nothing is printed as itself, and an empty one as `''` so it does not vanish.
pub fn shell_quote(text: &str) -> String {
    if text.is_empty() {
        return "''".to_string();
    }
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-./=:+@,%^".contains(c))
    {
        return text.to_string();
    }
    if text.chars().any(|c| c.is_control()) {
        let mut out = String::from("$'");
        for c in text.chars() {
            match c {
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\r' => out.push_str("\\r"),
                '\'' => out.push_str("\\'"),
                '\\' => out.push_str("\\\\"),
                c if c.is_control() => out.push_str(&format!("\\{:03o}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('\'');
        return out;
    }
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if !c.is_ascii_alphanumeric() && !"_-./=:+@,%^".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Rewrite Rust's exponent to C's, which is what every other `printf` prints.
///
/// Rust renders `1234.5` as `1.234500e3`; C — and therefore bash, dash and coreutils — renders it
/// `1.234500e+03`, with an explicit sign and at least two digits. A script comparing `printf %e`
/// output against a recorded value would differ on every number without this.
fn c_exponent(rendered: &str, marker: char) -> String {
    let Some((mantissa, exponent)) = rendered.split_once(marker) else {
        return rendered.to_string();
    };
    let (sign, digits) = match exponent.strip_prefix('-') {
        Some(rest) => ('-', rest),
        None => ('+', exponent.strip_prefix('+').unwrap_or(exponent)),
    };
    format!("{mantissa}{marker}{sign}{:0>2}", digits)
}

fn take_number(chars: &[char], j: &mut usize) -> Option<usize> {
    let start = *j;
    while *j < chars.len() && chars[*j].is_ascii_digit() {
        *j += 1;
    }
    if *j == start {
        return None;
    }
    chars[start..*j].iter().collect::<String>().parse().ok()
}

/// Parse an integer argument the way `printf` does.
///
/// An empty argument is 0 rather than an error: a format reused past the end of its arguments
/// gets empty strings, and `printf '%d\n' 1 2` must not complain about a third that is not there.
/// A leading `'` or `"` means "the numeric value of the next character", which is POSIX and is how
/// scripts get a character's codepoint without `od`.
fn parse_int(arg: &str) -> std::result::Result<i64, ()> {
    let text = arg.trim();
    if text.is_empty() {
        return Ok(0);
    }
    if let Some(rest) = text.strip_prefix(['\'', '"']) {
        return Ok(rest.chars().next().map(|c| c as i64).unwrap_or(0));
    }
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let value = if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        i64::from_str_radix(hex, 16)
    } else if digits.len() > 1 && digits.starts_with('0') {
        i64::from_str_radix(&digits[1..], 8)
    } else {
        digits.parse()
    };
    value.map(|v| sign * v).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{parse_int, render};

    /// One pass of the formatter, as a string.
    fn printf(format: &str, args: &[&str]) -> String {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let mut out = Vec::new();
        let mut next = 0;
        render(format, &args, &mut next, &mut out).expect("format");
        String::from_utf8(out).expect("utf8")
    }

    /// **The flags were parsed and then dropped.** `+`, a space and `#` all reached the `Spec` and
    /// none of them reached the output, so `printf '%+d' 5` gave `5` and `printf '%#x' 255` gave
    /// `ff`. Every expectation here is bash's, run side by side.
    #[test]
    fn the_sign_and_alternate_form_flags_are_honoured() {
        assert_eq!(printf("%+d|%+d", &["5", "-5"]), "+5|-5");
        assert_eq!(printf("[% d][% d]", &["5", "-5"]), "[ 5][-5]");
        // `+` wins when both are given, as in C.
        assert_eq!(printf("%+ d", &["5"]), "+5");
        assert_eq!(printf("%#o|%#x|%#X", &["8", "255", "255"]), "010|0xff|0XFF");
        // Zero takes no prefix: `0x0` is not what C prints.
        assert_eq!(printf("%#o|%#x", &["0", "0"]), "0|0");
        // And a float carries the sign too.
        assert_eq!(printf("%+.1f", &["1.5"]), "+1.5");
    }

    /// **`%g` is the conversion whose whole purpose is not to print twenty-one digits**, and it
    /// used to fall through to `%f` and do exactly that.
    ///
    /// C's rule: `%f` when the exponent fits inside the precision, `%e` otherwise, with the
    /// trailing zeros taken off unless `#` asked for them.
    #[test]
    fn g_chooses_between_fixed_and_exponent_as_c_does() {
        assert_eq!(printf("%g", &["1.5"]), "1.5");
        assert_eq!(printf("%g", &["100000"]), "100000");
        assert_eq!(printf("%g", &["0.0001"]), "0.0001");
        // Past six significant figures, and past 1e-4, it turns into an exponent.
        assert_eq!(printf("%g", &["1e20"]), "1e+20");
        assert_eq!(printf("%g", &["0.00001"]), "1e-05");
        assert_eq!(printf("%G", &["1e20"]), "1E+20");
        // `#` keeps the zeros the rule would otherwise strip.
        assert_eq!(printf("%#g", &["1.5"]), "1.50000");
    }

    #[test]
    fn integers_are_read_in_every_base_a_shell_accepts() {
        assert_eq!(parse_int("42"), Ok(42));
        assert_eq!(parse_int("-7"), Ok(-7));
        assert_eq!(parse_int("+7"), Ok(7));
        assert_eq!(parse_int("0x1f"), Ok(31));
        assert_eq!(parse_int("010"), Ok(8));
        // An empty argument is 0: a reused format runs past its arguments and must not complain.
        assert_eq!(parse_int(""), Ok(0));
        // POSIX: a leading quote means the next character's value.
        assert_eq!(parse_int("'A"), Ok(65));
        assert_eq!(parse_int("abc"), Err(()));
    }

    /// **A precision counts bytes, and a character can be several.** `printf '%.2s' 'héllo'` cut
    /// between the two bytes of `é` and `String::truncate` asserts on that — the shell aborted with
    /// a panic, which is a terminal left in raw mode. bash writes the half character and produces
    /// invalid UTF-8; a Rust `String` cannot, so the character is dropped instead.
    #[test]
    fn a_precision_never_cuts_a_character_in_half() {
        assert_eq!(printf("%.1s", &["héllo"]), "h");
        assert_eq!(
            printf("%.2s", &["héllo"]),
            "h",
            "é is two bytes and does not fit"
        );
        // Every other precision matches bash byte for byte.
        assert_eq!(printf("%.3s", &["héllo"]), "hé");
        assert_eq!(printf("%.4s", &["héllo"]), "hél");
        // A precision past the end is the whole string, not a panic.
        assert_eq!(printf("%.99s", &["héllo"]), "héllo");
        // And a four-byte character behaves the same way.
        assert_eq!(printf("%.2s", &["😀x"]), "");
        assert_eq!(printf("%.4s", &["😀x"]), "😀");
    }

    /// **Rust's formatter takes a `u16` precision and panics above it.** `printf '%.70000f' 1`
    /// aborted the shell with `Formatting argument out of range`; bash prints the number.
    #[test]
    fn an_enormous_float_precision_does_not_abort() {
        let wide = printf("%.70000f", &["1"]);
        assert!(
            wide.starts_with("1."),
            "it still formats: {}",
            &wide[..10.min(wide.len())]
        );
        assert!(
            wide.len() > 1000,
            "and it is long, not truncated to nothing"
        );
    }

    /// **A width is allocated, so an unbounded one is an unbounded allocation.**
    /// `printf '%9999999999s' x` asked for ten gigabytes of spaces. bash reports `Value too large
    /// for defined data type` and carries on; this reports and carries on too.
    #[test]
    fn an_enormous_width_is_refused_rather_than_allocated() {
        assert_eq!(
            printf("%9999999999s|", &["x"]),
            "x|",
            "no padding, and no allocation"
        );
        // A width anybody would really use still pads.
        assert_eq!(printf("%5s|", &["x"]), "    x|");
    }
}

/// The largest cut at or below `bytes` that does not land inside a character.
///
/// **`%.2s` on `héllo` aborted the shell.** POSIX counts a precision in bytes, and `é` is two of
/// them, so the cut fell between them — `String::truncate` asserts on that and a panic in a shell
/// is a terminal left in raw mode. bash writes the half character and produces invalid UTF-8; a
/// Rust `String` cannot hold that, so the character is dropped instead. The only disagreement with
/// bash is one broken byte that nothing can read on purpose.
fn whole_characters(text: &str, bytes: usize) -> usize {
    let mut at = bytes.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The widest field `printf` will pad to.
///
/// A width is a number somebody typed and the padding is allocated for it, so an unbounded one is
/// an unbounded allocation: `printf '%9999999999s' x` asked for ten gigabytes of spaces. A hundred
const WIDEST: usize = 100_000;
