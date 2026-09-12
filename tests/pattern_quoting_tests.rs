//! Shell patterns where quoting decides per character.
//!
//! `[[ abc == "a*"c ]]` is false in bash — only the `c` is a pattern — and oslo answered true,
//! because the right operand reached the matcher as flat text. A backslash left in an unquoted
//! expansion is an escape to the matcher (bash 5.2), and a single `=` inside `[[ ]]` is a pattern
//! match exactly like `==`. Every expectation was run against GNU bash 5.3.9 on the same line.

mod common;

use common::run;

fn check(cases: &[(&str, &str)]) {
    for (line, want) in cases {
        let r = run(line);
        assert_eq!(r.out(), *want, "`{line}`: {}", r.stderr);
    }
}

#[test]
fn a_partly_quoted_right_side_keeps_each_parts_quoting() {
    check(&[
        (r#"[[ abc == "a*"c ]] && echo y || echo n"#, "n"),
        (r#"[[ abc == 'a'"*" ]] && echo y || echo n"#, "n"),
        (r#"[[ abc == a'*' ]] && echo y || echo n"#, "n"),
        (r#"[[ abc == a"b*" ]] && echo y || echo n"#, "n"),
        (r#"p='*'; [[ abc == "$p"c ]] && echo y || echo n"#, "n"),
        (r#"[[ abc == $'a*' ]] && echo y || echo n"#, "n"),
        (r#"[[ abc == a\* ]] && echo y || echo n"#, "n"),
        (r#"[[ 'a*' == a\* ]] && echo y || echo n"#, "y"),
        (r#"[[ abc == "a"* ]] && echo y || echo n"#, "y"),
        (r#"[[ abc != "a*"c ]] && echo y || echo n"#, "y"),
    ]);
}

#[test]
fn a_single_equals_is_a_pattern_match_in_double_brackets() {
    check(&[
        (r#"[[ abc = a* ]] && echo y || echo n"#, "y"),
        (r#"[[ abc = "a*" ]] && echo y || echo n"#, "n"),
        (r#"[ abc = 'a*' ] && echo y || echo n"#, "n"),
    ]);
}

#[test]
fn a_backslash_from_an_unquoted_expansion_escapes() {
    check(&[
        (
            r#"p='a\*'; case 'a*' in $p) echo y;; *) echo n;; esac"#,
            "y",
        ),
        (r#"p='a\*'; case abc in $p) echo y;; *) echo n;; esac"#, "n"),
        (r#"p='a\*'; [[ 'a*' == $p ]] && echo y || echo n"#, "y"),
        (r#"p='a\*'; [[ 'a*' == "$p" ]] && echo y || echo n"#, "n"),
    ]);
}

#[test]
fn collating_symbols_and_equivalence_classes() {
    check(&[
        (r#"case abc in [[.a.]]*) echo y;; *) echo n;; esac"#, "y"),
        (
            r#"case émile in [[=e=]]mile) echo y;; *) echo n;; esac"#,
            "n",
        ),
        (
            r#"case emile in [[=e=]]mile) echo y;; *) echo n;; esac"#,
            "y",
        ),
        (
            r#"case amile in [[=e=]]mile) echo y;; *) echo n;; esac"#,
            "n",
        ),
    ]);
}
