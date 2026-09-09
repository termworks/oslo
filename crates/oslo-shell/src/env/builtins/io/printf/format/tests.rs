//! `printf`'s format string, checked a conversion at a time.
//!
//! Split out for the 600-line limit; the expectations are bash's, run side by side.

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
