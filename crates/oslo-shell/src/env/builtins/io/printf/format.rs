//! One pass over the format: literal bytes, escapes, and one `%` conversion at a time.
//!
//! Conversions bash hands to C's `printf` (`%d %s %c %f`...) follow C's rules for flags, width
//! and precision; the ones bash pads itself (`%b %q %Q %T %ls %lc`) follow bash's.

use super::args::{Args, Wide};
use super::float::Width;
use super::out::Out;
use super::render::Field;
use super::{quote, time};
use crate::env::builtins::io::escape::{expand_b, printf_escape};
use crate::env::origin_now;
use std::ops::Range;

/// The state one `printf` call carries from pass to pass.
pub struct Printer<'a> {
    pub args: Args<'a>,
    pub out: Out,
    pub utf8: bool,
    /// `--posix`: a floating conversion without `L` goes through `double`.
    pub posix: bool,
    /// `%n` assignments in the order they were made.
    pub counts: Vec<(String, usize)>,
}

/// A pass that ended `printf` early, with the status to return.
pub struct Stop(pub i32);

/// A width or precision as the format gives it.
#[derive(Clone, Copy)]
enum Given {
    Absent,
    /// From `*`, already an `int`.
    Star(i32),
    /// Written out, at this range of the format; `None` if it does not fit an `int`.
    Digits(Option<i32>, usize, usize),
    /// A `.` with no digits after it.
    Empty,
    /// A precision written with a `-`.
    Negative,
}

/// One conversion as written: flags, sizes, length modifiers and the letter.
struct Spec {
    field: Field,
    width: Given,
    precision: Given,
    long: bool,
    long_double: bool,
    letter: u8,
}

fn digits(format: &[u8], at: &mut usize) -> Given {
    let start = *at;
    let mut value: Option<i32> = Some(0);
    while let Some(&b) = format.get(*at).filter(|b| b.is_ascii_digit()) {
        value = value.and_then(|v| v.checked_mul(10)?.checked_add(i32::from(b - b'0')));
        *at += 1;
    }
    match *at == start {
        true => Given::Absent,
        false => Given::Digits(value, start, *at),
    }
}

impl Printer<'_> {
    /// Render `format` once, consuming arguments as its conversions ask.
    pub fn pass(&mut self, format: &[u8]) -> Result<(), Stop> {
        let mut at = 0;
        let mut decoded = Vec::new();
        while at < format.len() {
            match format[at] {
                b'\\' => {
                    let (used, _) =
                        printf_escape(&format[at + 1..], false, self.utf8, &mut decoded);
                    self.out.bytes(&decoded);
                    decoded.clear();
                    at += 1 + used;
                }
                b'%' if format.get(at + 1) == Some(&b'%') => {
                    self.out.byte(b'%');
                    at += 2;
                }
                b'%' => at = self.conversion(format, at)?,
                _ => {
                    let run = format[at..]
                        .iter()
                        .take_while(|b| !matches!(b, b'\\' | b'%'))
                        .count();
                    self.out.bytes(&format[at..at + run]);
                    at += run;
                }
            }
        }
        Ok(())
    }

    /// Read the conversion at `format[start] == '%'`, consuming the arguments its `*`s take.
    fn spec(&mut self, format: &[u8], start: usize) -> Result<(Spec, usize), Stop> {
        let mut at = start + 1;
        let mut field = Field::default();
        while let Some(&b) = format.get(at) {
            match b {
                b'#' => field.alt = true,
                b'-' => field.left = true,
                b'+' => field.plus = true,
                b' ' => field.space = true,
                b'0' => field.zero = true,
                b'\'' => {}
                _ => break,
            }
            at += 1;
        }
        let width = match format.get(at) == Some(&b'*') {
            true => {
                at += 1;
                Given::Star(self.args.int(0))
            }
            false => digits(format, &mut at),
        };
        let mut precision = Given::Absent;
        if format.get(at) == Some(&b'.') {
            at += 1;
            precision = if format.get(at) == Some(&b'*') {
                at += 1;
                Given::Star(self.args.int(-1))
            } else if format.get(at) == Some(&b'-') {
                at += 1;
                digits(format, &mut at);
                Given::Negative
            } else {
                match digits(format, &mut at) {
                    Given::Absent => Given::Empty,
                    given => given,
                }
            };
        }
        let (mut long, mut long_double) = (false, false);
        while let Some(&b) = format.get(at).filter(|b| b"hjlLtz".contains(b)) {
            long |= b == b'l';
            long_double |= b == b'L';
            at += 1;
        }
        let Some(&letter) = format.get(at) else {
            let malformed = String::from_utf8_lossy(&format[start..]).into_owned();
            complain_in(
                format,
                start..format.len(),
                &format!("printf: `{malformed}': missing format character"),
                "a conversion with no letter",
                Some(LETTERS),
            );
            return Err(Stop(1));
        };
        let spec = Spec {
            field,
            width,
            precision,
            long,
            long_double,
            letter,
        };
        Ok((spec, at + 1))
    }

    /// The field C's `printf` would use; a size past `int` is the error it would report.
    fn c_field(&self, spec: &Spec) -> Result<Field, Stop> {
        let mut field = spec.field;
        let too_large = || {
            eprintln!(
                "{}printf: Value too large for defined data type",
                origin_now()
            );
            Stop(1)
        };
        match spec.width {
            Given::Star(w) => {
                field.left |= w < 0;
                field.width = w.unsigned_abs() as usize;
            }
            Given::Digits(Some(w), ..) => field.width = w as usize,
            Given::Digits(None, ..) => return Err(too_large()),
            Given::Absent | Given::Empty | Given::Negative => {}
        }
        field.precision = match spec.precision {
            Given::Star(p) => usize::try_from(p).ok(),
            Given::Digits(Some(p), ..) => Some(p as usize),
            Given::Digits(None, ..) => return Err(too_large()),
            Given::Empty => Some(0),
            Given::Absent | Given::Negative => None,
        };
        Ok(field)
    }

    /// The field bash pads with itself; a size past `int` is reported and then ignored.
    fn bash_field(&mut self, spec: &Spec, format: &[u8]) -> Field {
        let mut field = spec.field;
        let mut out_of_range = |from: usize, to: usize| {
            let shown = String::from_utf8_lossy(&format[from..to]);
            let body = format!("printf: {shown}: Numerical result out of range");
            complain_in(format, from..to, &body, "wider than an int", None);
            self.args.error = true;
        };
        match spec.width {
            Given::Star(w) => {
                field.left |= w < 0;
                field.width = w.unsigned_abs() as usize;
            }
            Given::Digits(Some(w), ..) => field.width = w as usize,
            Given::Digits(None, from, to) => out_of_range(from, to),
            Given::Absent | Given::Empty | Given::Negative => {}
        }
        field.precision = match spec.precision {
            Given::Star(p) => usize::try_from(p).ok(),
            Given::Digits(Some(p), ..) => Some(p as usize),
            Given::Digits(None, from, to) => {
                out_of_range(from, to);
                Some(0)
            }
            Given::Empty | Given::Negative => Some(0),
            Given::Absent => None,
        };
        field
    }

    /// Render the conversion at `format[start]`, returning where the format continues.
    fn conversion(&mut self, format: &[u8], start: usize) -> Result<usize, Stop> {
        let (spec, next) = self.spec(format, start)?;
        let wide = self.utf8 && (spec.long || spec.letter.is_ascii_uppercase());
        match spec.letter {
            b'c' | b'C' if wide => {
                let field = self.bash_field(&spec, format);
                let (body, units): (Vec<u8>, usize) = match self.args.wide_char() {
                    Wide::Nul => (vec![0], 1),
                    Wide::Char(c) => (c.to_string().into_bytes(), 1),
                    Wide::Invalid => (Vec::new(), 1),
                };
                let shown = field.precision.map_or(units, |p| p.min(units));
                let body = if shown == 0 { Vec::new() } else { body };
                field.text_counted(&mut self.out, &body, shown);
            }
            b'c' | b'C' => {
                let field = self.c_field(&spec)?;
                let byte = self.args.byte();
                if spec.letter == b'C' && !byte.is_ascii() {
                    return Err(unconvertible());
                }
                field.text_counted(&mut self.out, &[byte], 1);
            }
            b's' | b'S' if wide => {
                let field = self.bash_field(&spec, format);
                let chars = self.args.wide_string();
                let shown = field.precision.map_or(chars.len(), |p| p.min(chars.len()));
                let body: Option<String> = chars[..shown].iter().copied().collect();
                field.text_counted(&mut self.out, body.unwrap_or_default().as_bytes(), shown);
            }
            b'S' => return Err(unconvertible()),
            b's' => {
                let field = self.c_field(&spec)?;
                let text = self.args.string();
                field.text(&mut self.out, text);
            }
            b'b' => {
                let field = self.bash_field(&spec, format);
                let (body, stop) = expand_b(self.args.string(), self.utf8);
                field.text(&mut self.out, &body);
                if stop {
                    return Err(Stop(0));
                }
            }
            b'q' | b'Q' => {
                // `%Q` cuts the argument to the precision before quoting and prints the quoted
                // text whole, except that a `.` with no digits prints nothing.
                let field = self.bash_field(&spec, format);
                let mut text = self.args.string();
                if spec.letter == b'Q' {
                    let cut = match spec.precision {
                        Given::Star(p) => usize::try_from(p).ok(),
                        Given::Digits(p, ..) => p.map(|p| p as usize),
                        _ => None,
                    };
                    text = &text[..cut.unwrap_or(text.len()).min(text.len())];
                }
                let quoted = quote::quote(text, self.utf8, field.alt);
                match (spec.letter, spec.precision) {
                    (b'Q', Given::Empty | Given::Negative) => {
                        field.text_counted(&mut self.out, b"", 0)
                    }
                    (b'Q', _) => field.text_counted(&mut self.out, &quoted, quoted.len()),
                    _ => field.text(&mut self.out, &quoted),
                }
            }
            b'(' => return self.time(format, start, next, &spec),
            b'n' => {
                let name = String::from_utf8_lossy(self.args.string()).into_owned();
                if !name.is_empty() {
                    if !crate::env::scope::is_valid_identifier(&name) {
                        let body = format!("printf: `{name}': not a valid identifier");
                        self.args.complain(&body, "not a name");
                        return Err(Stop(1));
                    }
                    self.counts.push((name, self.out.pass));
                }
            }
            b'd' | b'i' => {
                let field = self.c_field(&spec)?;
                let value = self.args.intmax();
                field.signed(&mut self.out, value);
            }
            b'o' | b'u' | b'x' | b'X' => {
                let field = self.c_field(&spec)?;
                let value = self.args.uintmax();
                field.unsigned(&mut self.out, value, spec.letter);
            }
            b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'a' | b'A' => {
                let field = self.c_field(&spec)?;
                let width = match self.posix && !spec.long_double {
                    true => Width::Double,
                    false => Width::Extended,
                };
                let value = self.args.float(width);
                field.float(&mut self.out, value, spec.letter, width);
            }
            other => {
                let shown = String::from_utf8_lossy(&[other]).into_owned();
                let body = format!("printf: `{shown}': invalid format character");
                complain_in(
                    format,
                    start..next,
                    &body,
                    "not a conversion",
                    Some(LETTERS),
                );
                return Err(Stop(1));
            }
        }
        Ok(next)
    }

    /// `%(fmt)T`. A format with no closing `)T` is a warning, and the `%` is printed as text.
    fn time(
        &mut self,
        format: &[u8],
        start: usize,
        next: usize,
        spec: &Spec,
    ) -> Result<usize, Stop> {
        let Some((pattern, used)) = time::split(&format[next..]) else {
            eprintln!(
                "{}printf: warning: invalid time format specification",
                origin_now()
            );
            self.out.byte(b'%');
            return Ok(start + 1);
        };
        let field = self.bash_field(spec, format);
        let arg = (self.args.next < self.args.len()).then(|| self.args.intmax());
        let text = time::render(pattern, arg);
        field.text(&mut self.out, &text);
        Ok(next + used)
    }
}

const LETTERS: &str = "a `%` starts a conversion and needs a letter: %s %d %f %x %c %b %q, or %% for a literal percent";

/// Report a problem at `range` of the format, with the caret under it where the bytes allow.
fn complain_in(format: &[u8], range: Range<usize>, body: &str, label: &str, help: Option<&str>) {
    let text = String::from_utf8_lossy(format).into_owned();
    let from = String::from_utf8_lossy(&format[..range.start]).len();
    let to = from + String::from_utf8_lossy(&format[range]).len();
    let inside = match to <= text.len() && text.is_char_boundary(from) && text.is_char_boundary(to)
    {
        true => from..to,
        false => 0..text.len(),
    };
    let words = [String::from("printf"), text.clone()];
    crate::env::complain_within(&words, &text, inside, body, label, help);
}

/// What C's `printf` reports for a wide conversion it cannot encode in this locale.
fn unconvertible() -> Stop {
    eprintln!(
        "{}printf: Invalid or incomplete multibyte or wide character",
        origin_now()
    );
    Stop(1)
}

#[cfg(test)]
#[path = "format/tests.rs"]
mod tests;
