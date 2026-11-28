//! Reading and printing floats, each expectation taken from bash 5.3 on glibc.

use super::super::out::Out;
use super::super::render::Field;
use super::{Real, Width, parse};

/// `printf` of one float conversion, with `precision` and the `#` flag.
fn show(text: &str, conversion: u8, precision: Option<usize>, width: Width) -> String {
    let value = parse(text.as_bytes(), width).value;
    let field = Field {
        precision,
        ..Field::default()
    };
    let mut out = Out::new(true);
    field.float(&mut out, value, conversion, width);
    String::from_utf8(out.into_kept()).expect("ascii")
}

fn long(text: &str, conversion: u8, precision: Option<usize>) -> String {
    show(text, conversion, precision, Width::Extended)
}

#[test]
fn a_long_double_prints_its_exact_value() {
    assert_eq!(long("0.1", b'f', Some(20)), "0.10000000000000000000");
    assert_eq!(
        long("0.1", b'f', Some(30)),
        "0.100000000000000000001355252716"
    );
    assert_eq!(
        show("0.1", b'f', Some(20), Width::Double),
        "0.10000000000000000555"
    );
    assert_eq!(long("1.5e308", b'f', None)[..20], *"14999999999999999999");
    assert_eq!(long("2.5", b'f', Some(0)), "2");
    assert_eq!(long("3.5", b'f', Some(0)), "4");
    assert_eq!(long("1e3", b'f', None), "1000.000000");
    assert_eq!(long("0x10", b'f', None), "16.000000");
}

#[test]
fn exponent_and_general_follow_c() {
    assert_eq!(long("1234.5", b'e', None), "1.234500e+03");
    assert_eq!(long("0.000123", b'E', None), "1.230000E-04");
    assert_eq!(long("1e-5", b'G', None), "1E-05");
    assert_eq!(long("1234567", b'g', None), "1.23457e+06");
    assert_eq!(long("0.0001", b'g', None), "0.0001");
    assert_eq!(long("100000", b'g', None), "100000");
    assert_eq!(long("9.96", b'g', Some(2)), "10");
    assert_eq!(long("0", b'e', None), "0.000000e+00");
}

#[test]
fn hex_floats_put_four_bits_before_the_point() {
    assert_eq!(long("1", b'a', None), "0x8p-3");
    assert_eq!(long("0.5", b'A', None), "0X8P-4");
    assert_eq!(long("0.1", b'a', None), "0xc.ccccccccccccccdp-7");
    assert_eq!(long("1.999", b'a', Some(2)), "0xf.fep-3");
    assert_eq!(long("1.5", b'a', Some(0)), "0xcp-3");
    assert_eq!(long("0", b'a', None), "0x0p+0");
    assert_eq!(long("0x1p-16445", b'a', None), "0x0.000000000000001p-16385");
    assert_eq!(show("1", b'a', None, Width::Double), "0x1p+0");
}

#[test]
fn reading_stops_where_strtold_stops() {
    let end = |text: &str| parse(text.as_bytes(), Width::Extended).end;
    assert_eq!(end(" 1.5x"), 4);
    assert_eq!(end("."), 0);
    assert_eq!(end("1e"), 1);
    assert_eq!(end("0x"), 1);
    assert_eq!(end("0x1p+"), 3);
    assert_eq!(end("infinit"), 3);
    assert_eq!(end("nan(abc)x"), 8);
    assert_eq!(end("nan("), 3);
    assert!(matches!(
        parse(b"-nan", Width::Extended).value,
        Real::Nan { negative: true }
    ));
}

#[test]
fn out_of_range_values_saturate_and_say_so() {
    let read = parse(b"1e5000", Width::Extended);
    assert!(read.range && matches!(read.value, Real::Infinite { negative: false }));
    assert!(parse(b"1e-5000", Width::Extended).range);
    assert!(parse(b"1e-4940", Width::Extended).range);
    assert!(!parse(b"0x1p-16445", Width::Extended).range);
    assert!(!parse(b"3.36210314311209350626e-4932", Width::Extended).range);
    assert!(parse(b"3.362103143112093506e-4932", Width::Extended).range);
}
