use super::*;

fn text(row: &Record, name: &str) -> String {
    row.get(name).map(render_transport).unwrap_or_default()
}

/// The first line is the names, and a column of numbers arrives as numbers.
#[test]
fn a_header_row_names_the_columns() {
    let rows = from_delimited("name,amount\na,10\nb,9\n", ',').expect("valid");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].columns(), ["name", "amount"]);
    assert_eq!(rows[0].get("amount"), Some(&Val::Int(10)));
}

/// **A newline inside a quoted field does not end the record.** This is the difference between a
/// parser and `split('\n')`, and the case that quietly corrupts a spreadsheet export.
#[test]
fn a_newline_inside_a_quoted_field_is_not_a_record_boundary() {
    let rows = from_delimited("a,b\n\"one\ntwo\",3\n", ',').expect("valid");
    assert_eq!(rows.len(), 1, "one record, not two");
    assert_eq!(text(&rows[0], "a"), "one\ntwo");
    assert_eq!(rows[0].get("b"), Some(&Val::Int(3)));
}

/// A doubled quote is one quote, and a delimiter inside quotes is text.
#[test]
fn quotes_and_delimiters_survive_inside_a_field() {
    let rows = from_delimited("a,b\n\"say \"\"hi\"\"\",\"x,y\"\n", ',').expect("valid");
    assert_eq!(text(&rows[0], "a"), "say \"hi\"");
    assert_eq!(text(&rows[0], "b"), "x,y");
}

/// A document that does not end in a newline still has a last record.
#[test]
fn a_missing_final_newline_does_not_lose_a_row() {
    let rows = from_delimited("a\n1\n2", ',').expect("valid");
    assert_eq!(rows.len(), 2);
}

/// A field left open is a mistake worth reporting, not a truncated table.
#[test]
fn an_unclosed_quote_is_refused() {
    assert!(from_delimited("a\n\"never closed\n", ',').is_err());
}

/// Nothing in is nothing out, rather than a row of nothing.
#[test]
fn an_empty_document_is_no_rows() {
    assert!(from_delimited("", ',').expect("valid").is_empty());
}

/// **A field is quoted only when it has to be**, so a plain table stays greppable.
#[test]
fn writing_quotes_only_what_needs_it() {
    let rows = vec![
        Record::from_pairs([("name", Val::Str("plain".into())), ("n", Val::Int(1))]),
        Record::from_pairs([("name", Val::Str("has,comma".into())), ("n", Val::Int(2))]),
        Record::from_pairs([("name", Val::Str("says \"hi\"".into())), ("n", Val::Int(3))]),
    ];
    let csv = to_delimited(&rows, ',');
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "name,n", "a header row, unquoted");
    assert_eq!(lines[1], "plain,1");
    assert_eq!(lines[2], "\"has,comma\",2");
    assert_eq!(lines[3], "\"says \"\"hi\"\"\",3");
}

/// What is written comes back the same, which is the only claim worth making about a format.
#[test]
fn a_table_survives_the_round_trip() {
    let rows = vec![
        Record::from_pairs([
            ("a", Val::Str("one\ntwo".into())),
            ("b", Val::Str("x,y".into())),
            ("c", Val::Int(3)),
        ]),
        Record::from_pairs([
            ("a", Val::Str("plain".into())),
            ("b", Val::Str("q\"q".into())),
            ("c", Val::Int(4)),
        ]),
    ];
    for delimiter in [',', '\t'] {
        let back = from_delimited(&to_delimited(&rows, delimiter), delimiter).expect("valid");
        assert_eq!(back.len(), 2, "delimiter {delimiter:?}");
        assert_eq!(text(&back[0], "a"), "one\ntwo");
        assert_eq!(text(&back[0], "b"), "x,y");
        assert_eq!(back[1].get("c"), Some(&Val::Int(4)));
    }
}

/// The header is the union of every row's columns, because rows may disagree and a CSV reader
/// needs one shape.
#[test]
fn the_header_is_the_union_of_the_rows() {
    let rows = vec![
        Record::from_pairs([("a", Val::Int(1))]),
        Record::from_pairs([("b", Val::Int(2))]),
    ];
    let csv = to_delimited(&rows, ',');
    assert_eq!(csv.lines().next(), Some("a,b"));
    assert_eq!(csv.lines().nth(1), Some("1,"), "a gap is an empty field");
}

/// A tab-separated field containing a tab is quoted, not escaped — `to tsv` is for somebody else's
/// program, and `to text` is the one that escapes.
#[test]
fn tsv_quotes_a_tab_rather_than_escaping_it() {
    let rows = vec![Record::from_pairs([("a", Val::Str("x\ty".into()))])];
    assert_eq!(to_delimited(&rows, '\t').lines().nth(1), Some("\"x\ty\""));
}

/// Only two delimiters are known, and a typo in the format is refused by name elsewhere.
#[test]
fn the_delimiters_are_named() {
    assert_eq!(delimiter("csv"), Some(','));
    assert_eq!(delimiter("tsv"), Some('\t'));
    assert_eq!(delimiter("psv"), None);
}

/// Quoting says "this is text", and the reader has to believe it.
///
/// Checked against Python's `csv` module, which answers `'007'` and `'  pad  '` for these two
/// documents. oslo answered `7` and `pad`: the quotes were parsed and then discarded, so a
/// spreadsheet's zero-padded id came back as a number and its padding came back trimmed.
#[test]
fn a_quoted_field_is_text_and_keeps_its_spaces() {
    let rows = from_delimited("v\n\"007\"\n", ',').expect("valid");
    assert_eq!(rows[0].get("v"), Some(&Val::Str("007".into())));

    let rows = from_delimited("v\n\"  pad  \"\n", ',').expect("valid");
    assert_eq!(rows[0].get("v"), Some(&Val::Str("  pad  ".into())));
}

/// An unquoted field keeps the old rule, spacing included.
///
/// `a, 1, 2` is how a CSV gets written by hand, and the space before the `1` is layout rather than
/// data — so this half deliberately does not change.
#[test]
fn an_unquoted_field_still_becomes_a_number() {
    let rows = from_delimited("v\n 42 \n", ',').expect("valid");
    assert_eq!(rows[0].get("v"), Some(&Val::Int(42)));
}

/// The writer's half: a string that would read back as something else is quoted.
///
/// Without this the reader's fix buys nothing for a round trip — `007` written bare comes back as
/// `7` however carefully the quotes are honoured. A string that reads back as itself is still
/// written bare, so a plain table stays greppable.
#[test]
fn a_string_that_looks_like_a_number_is_quoted() {
    let rows = vec![Record::from_pairs([
        ("id", Val::Str("007".into())),
        ("name", Val::Str("plain".into())),
        ("n", Val::Int(7)),
    ])];
    assert_eq!(
        to_delimited(&rows, ',').lines().nth(1),
        Some("\"007\",plain,7")
    );
}

/// The property the two halves exist for, over the values that break it.
#[test]
fn the_awkward_values_survive_a_round_trip() {
    let awkward = [
        "007",
        "  pad  ",
        "a,b",
        "he said \"hi\"",
        "line1\nline2",
        "a\tb",
        "--flag",
        "a=b",
        "",
        "9223372036854775807",
    ];
    for delimiter in [',', '\t'] {
        for value in awkward {
            let rows = vec![Record::from_pairs([("v", Val::Str(value.into()))])];
            let back = from_delimited(&to_delimited(&rows, delimiter), delimiter).expect("valid");
            assert_eq!(
                back[0].get("v"),
                Some(&Val::Str(value.to_string())),
                "{value:?} did not survive {delimiter:?}"
            );
        }
    }
}
