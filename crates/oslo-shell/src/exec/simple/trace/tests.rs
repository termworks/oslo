use super::quote::{conditional_text, quote_word};
use super::{assignment_text, list_assignment_text};
use crate::syntax::lower::cond::{NEGATION, QUOTED_CLOSE, QUOTED_OPEN};

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[test]
fn words_are_quoted_as_bash_quotes_them() {
    assert_eq!(quote_word("echo"), "echo");
    assert_eq!(quote_word(""), "''");
    assert_eq!(quote_word("a b"), "'a b'");
    assert_eq!(quote_word("it's"), r"'it'\''s'");
    assert_eq!(quote_word("'"), r"\'");
    assert_eq!(quote_word("["), "'['");
    assert_eq!(quote_word("x~y"), "x~y");
    assert_eq!(quote_word("~x"), "'~x'");
    assert_eq!(quote_word("a#"), "a#");
    assert_eq!(quote_word("#a"), "'#a'");
    assert_eq!(quote_word("tab\tx"), r"$'tab\tx'");
    assert_eq!(quote_word("\u{1}\u{7f}"), r"$'\001\177'");
}

#[test]
fn assignments_print_an_empty_value_as_nothing() {
    assert_eq!(assignment_text("z", false, ""), "z=");
    assert_eq!(assignment_text("y", false, "a b"), "y='a b'");
    assert_eq!(assignment_text("x", true, " a"), "x+=' a'");
    assert_eq!(
        list_assignment_text("a", false, "1 \"2 3\""),
        "a=(1 \"2 3\")"
    );
}

#[test]
fn a_conditional_prints_its_operator_and_escapes_quoted_pattern_characters() {
    let quoted = format!("{QUOTED_OPEN}a*{QUOTED_CLOSE}");
    assert_eq!(
        conditional_text(&args(&["a", "==", &quoted, "]]"])),
        r"[[ a == a\* ]]"
    );
    assert_eq!(
        conditional_text(&args(&["x", "!=", "y", "]]"])),
        "[[ x != y ]]"
    );
    assert_eq!(
        conditional_text(&args(&[NEGATION, "-n", "", "]]"])),
        "[[ ! -n '' ]]"
    );
    let regex = format!("^a{QUOTED_OPEN}.{QUOTED_CLOSE}(b)$");
    assert_eq!(
        conditional_text(&args(&["ab", "=~", &regex, "]]"])),
        r"[[ ab =~ ^a\.(b)$ ]]"
    );
}
