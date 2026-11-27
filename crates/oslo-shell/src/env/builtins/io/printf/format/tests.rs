//! `printf`'s format walker, each expectation taken from bash 5.3.

use super::super::args::Args;
use super::super::out::Out;
use super::{Printer, Stop};

/// Every pass `printf` would make, the kept bytes and the status.
fn run(format: &str, args: &[&str], utf8: bool) -> (Vec<u8>, i32) {
    let list: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
    let mut printer = Printer {
        args: Args::new(&list, utf8),
        out: Out::new(true),
        utf8,
        posix: false,
        counts: Vec::new(),
    };
    let status = loop {
        let before = printer.args.next;
        if let Err(Stop(code)) = printer.pass(format.as_bytes()) {
            break code;
        }
        if printer.args.next >= printer.args.len() || printer.args.next == before {
            break i32::from(printer.args.error);
        }
    };
    (printer.out.into_kept(), status)
}

fn printf(format: &str, args: &[&str]) -> String {
    String::from_utf8_lossy(&run(format, args, true).0).into_owned()
}

#[test]
fn integers_take_c_flags_and_precision() {
    assert_eq!(
        printf("%05d|%+05d|% 05d|%-5d|", &["-7", "7", "3", "4"]),
        "-0007|+0007| 0003|4    |"
    );
    assert_eq!(
        printf("%.3d|%.0d|%8.3d|%08.3d|%.3x|", &["7", "0", "5", "5", "10"]),
        "007||     005|     005|00a|"
    );
    assert_eq!(
        printf("%#o|%#x|%#08x|%#.0o|%#.0x|", &["8", "255", "255", "0", "0"]),
        "010|0xff|0x0000ff|0||"
    );
    assert_eq!(
        printf("%'d|%u|%x", &["1234567", "-1", "-1"]),
        "1234567|18446744073709551615|ffffffffffffffff"
    );
}

#[test]
fn a_bad_number_prints_its_prefix_and_fails() {
    assert_eq!(run("%d", &["12abc"], true), (b"12".to_vec(), 1));
    assert_eq!(
        run("%d", &["99999999999999999999"], true),
        (b"9223372036854775807".to_vec(), 1)
    );
    assert_eq!(
        run("%d", &["-9223372036854775808"], true),
        (b"-9223372036854775808".to_vec(), 0)
    );
    assert_eq!(
        run("%u", &["18446744073709551615"], true),
        (b"18446744073709551615".to_vec(), 0)
    );
    assert_eq!(run("%d", &[""], true), (b"0".to_vec(), 1));
    assert_eq!(
        run("%d|%d", &["0b101", "0x1f"], true),
        (b"5|31".to_vec(), 0)
    );
    assert_eq!(
        run("%d|%f", &["'a", "\"a"], true),
        (b"97|97.000000".to_vec(), 0)
    );
    // A missing argument is zero and no error.
    assert_eq!(run("%d|%s|", &[], true), (b"0||".to_vec(), 0));
}

#[test]
fn backslash_c_in_b_stops_everything() {
    assert_eq!(run("%b|%s\n", &["a\\cb", "x"], true), (b"a".to_vec(), 0));
    assert_eq!(run("%d%b", &["abc", "\\c"], true), (b"0".to_vec(), 0));
    assert_eq!(printf("a\\cb", &[]), "a\\cb");
    assert_eq!(printf("%5b|%.1b|", &["ab", "abc"]), "   ab|a|");
}

#[test]
fn c_takes_one_byte_and_a_missing_one_is_nul() {
    assert_eq!(run("%c|%c|%c|", &["é", ""], true).0, b"\xc3|\0|\0|");
    assert_eq!(printf("%.1s|%5s|", &["éa", "é"]), "\u{fffd}|   é|");
    assert_eq!(printf("%lc|%5ls|%.1ls|", &["é", "é", "éa"]), "é|    é|é|");
}

#[test]
fn q_quotes_as_bash_does() {
    let q = |text: &str, utf8: bool| String::from_utf8(run("%q", &[text], utf8).0).expect("ascii");
    assert_eq!(q("a b,{c}", true), "a\\ b\\,\\{c\\}");
    assert_eq!(
        q("~a a~ :~ #x x# ^", true),
        "\\~a\\ a~\\ :\\~\\ #x\\ x#\\ \\^"
    );
    assert_eq!(q("#x", true), "\\#x");
    assert_eq!(q("\x1b[1m", true), "$'\\E[1m'");
    assert_eq!(q("é", true), "é");
    assert_eq!(q("é", false), "$'\\303\\251'");
    assert_eq!(q("", true), "''");
    assert_eq!(
        printf("%#q|%Q|%.3Q|%.3q|", &["it's", "a b", "a b c", "a b"]),
        "'it'\\''s'|a\\ b|a\\ b|a\\ |"
    );
}

#[test]
fn a_time_conversion_formats_an_epoch() {
    // `%s` and `%%` read no zone, so they are the same everywhere.
    assert_eq!(
        printf("%(%s)T|%(%%)T|%-4(%s)T|", &["5", "5", "7"]),
        "5|%|7   |"
    );
    assert_eq!(run("%(abc", &[], true).0, b"%(abc");
}

#[test]
fn malformed_formats_stop_with_status_one() {
    assert_eq!(run("a%yb", &[], true), (b"a".to_vec(), 1));
    assert_eq!(run("x%5", &[], true), (b"x".to_vec(), 1));
    assert_eq!(run("a%9999999999s|b", &["x"], true), (b"a".to_vec(), 1));
    assert_eq!(run("a%9999999999b|b", &["x"], true), (b"ax|b".to_vec(), 1));
    assert_eq!(printf("%zb|%hhd|%Lf", &["x", "1", "1"]), "x|1|1.000000");
}

#[test]
fn n_counts_the_bytes_of_this_pass() {
    let list = [b"c1".to_vec(), b"abc".to_vec(), b"c2".to_vec()];
    let mut printer = Printer {
        args: Args::new(&list, true),
        out: Out::new(true),
        utf8: true,
        posix: false,
        counts: Vec::new(),
    };
    assert!(printer.pass(b"%n|%s%n|").is_ok());
    assert_eq!(
        printer.counts,
        vec![("c1".to_string(), 0), ("c2".to_string(), 4)]
    );
}
