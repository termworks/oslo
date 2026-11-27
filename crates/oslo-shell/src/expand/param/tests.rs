//! What `${name<op>}` does, checked against the cases that have actually gone wrong.
//!
//! Split out of `param.rs` for the 600-line limit, and it is the right seam: the file above is
//! the expansion rules and this is every case that pins them down.

use super::{expand_param, expand_to_string, is_param_name, positional_fields};
use crate::env::Environment;
use crate::expand::word::{Origin, field_text};
use oslo_base::ast::{ParamExpansion, ReplaceScope, Word, WordPart};

fn texts(env: &Environment, name: &str, origin: Origin) -> Vec<String> {
    positional_fields(env, name, origin)
        .iter()
        .map(|f| field_text(f))
        .collect()
}

/// Expand `${name<op>}` against an environment holding `vars`.
fn expand(vars: &[(&str, &str)], name: &str, op: ParamExpansion) -> Result<String, String> {
    let mut env = Environment::new();
    for (k, v) in vars {
        env.set_var(k, v, false);
    }
    expand_to_string(&mut env, name, &op).map_err(|e| e.to_string())
}

fn lit(s: &str) -> Word {
    Word::from_literal(s)
}

/// A word that expands `$other` — the payload shape R2.6 exists for.
fn var_word(other: &str) -> Word {
    Word {
        parts: vec![WordPart::Variable {
            name: other.to_string(),
            expansion_type: ParamExpansion::Normal,
        }],
    }
}

#[test]
fn at_is_one_field_per_positional() {
    let mut env = Environment::new();
    env.set_positional(vec!["a b".into(), "c".into()]);
    assert_eq!(texts(&env, "@", Origin::Quoted), vec!["a b", "c"]);
}

#[test]
fn at_with_no_positionals_is_no_field() {
    let mut env = Environment::new();
    env.set_positional(Vec::new());
    assert!(positional_fields(&env, "@", Origin::Quoted).is_empty());
}

#[test]
fn star_joins_with_the_first_ifs_character() {
    let mut env = Environment::new();
    env.set_positional(vec!["a".into(), "b".into()]);
    env.set_var("IFS", "-+", false);
    assert_eq!(texts(&env, "*", Origin::Quoted), vec!["a-b"]);
}

/// An unset IFS means the default, whose first character is a space.
#[test]
fn star_defaults_to_a_space() {
    let mut env = Environment::new();
    env.set_positional(vec!["a".into(), "b".into()]);
    env.unset_var("IFS");
    assert_eq!(texts(&env, "*", Origin::Quoted), vec!["a b"]);
}

/// With IFS empty there is no separator at all, so `$*` concatenates.
#[test]
fn star_with_empty_ifs_concatenates() {
    let mut env = Environment::new();
    env.set_positional(vec!["a".into(), "b".into()]);
    env.set_var("IFS", "", false);
    assert_eq!(texts(&env, "*", Origin::Quoted), vec!["ab"]);
}

/// The regression this module's second rule exists for: an unimplemented form used to be
/// looked up as a variable named after the whole brace body, and answer `""`.
#[test]
fn an_unparsed_brace_body_is_an_error_not_an_empty_string() {
    for body in ["v:2:3", "v/x/y", "v^^", "v@Q", "!v", "a b", ""] {
        let got = expand(&[], body, ParamExpansion::Normal);
        assert!(
            got.is_err(),
            "${{{body}}} expanded to {got:?}, not an error"
        );
    }
}

#[test]
fn a_real_name_is_still_a_name() {
    assert!(is_param_name("HOME"));
    assert!(is_param_name("_x1"));
    assert!(is_param_name("12"));
    assert!(is_param_name("@"));
    assert!(is_param_name("#"));
    assert!(!is_param_name(""));
    assert!(!is_param_name("1a"));
    assert!(!is_param_name("v-d"));
}

/// Expand `${@<op>}` / `${*<op>}` over `params`, as a list of field texts.
fn positional(params: &[&str], name: &str, op: ParamExpansion) -> Result<Vec<String>, String> {
    let mut env = Environment::new();
    env.set_positional(params.iter().map(|p| (*p).to_string()).collect());
    expand_param(&mut env, name, &op, true)
        .map(|fields| fields.iter().map(|f| field_text(f)).collect())
        .map_err(|e| e.to_string())
}

fn slice(offset: &str, length: Option<&str>) -> ParamExpansion {
    ParamExpansion::Substring {
        offset: lit(offset),
        length: length.map(lit),
    }
}

/// R11/B5: `${@:1}` slices the *argument list*.
///
/// This used to fall through to the scalar path, which joins the positionals with a space and
/// then cuts characters out of that string — so `set -- a b c; "${@:1}"` handed back the one
/// field `" b c"` and every `"${@:2}"` forwarding idiom silently corrupted its arguments.
#[test]
fn slicing_the_positionals_selects_arguments_not_characters() {
    let args = ["a", "b", "c"];
    let got = positional(&args, "@", slice("1", None));
    assert_eq!(got, Ok(vec!["a".into(), "b".into(), "c".into()]));
    assert_eq!(
        positional(&args, "@", slice("2", None)),
        Ok(vec!["b".into(), "c".into()])
    );
    assert_eq!(
        positional(&args, "@", slice("1", Some("2"))),
        Ok(vec!["a".into(), "b".into()])
    );
    // Negative counts back from the end of the same list; past the front selects nothing, and
    // `"$@"` with nothing selected is no field rather than one empty one.
    assert_eq!(
        positional(&args, "@", slice("-2", None)),
        Ok(vec!["b".into(), "c".into()])
    );
    assert_eq!(positional(&args, "@", slice("9", None)), Ok(vec![]));
    // An argument that contains the separator stays one field, which is the whole reason the
    // joined string is the wrong thing to slice.
    assert_eq!(
        positional(&["a b", "c"], "@", slice("1", None)),
        Ok(vec!["a b".into(), "c".into()])
    );
    // `${*:1}` is the joined form of the same slice: one field, always.
    assert_eq!(
        positional(&args, "*", slice("2", None)),
        Ok(vec!["b c".into()])
    );
    assert_eq!(
        positional(&args, "*", slice("9", None)),
        Ok(vec![String::new()])
    );
    // bash makes a negative length fatal for a list, unlike for a string.
    assert!(positional(&args, "@", slice("1", Some("-1"))).is_err());
}

/// The list bash slices is `$0 $1 $2 …`, one longer than `$@` — which is why the forwarding
/// idiom is written `"${@:1}"` and `"${@:0}"` picks up the shell's own name.
#[test]
fn a_positional_slice_is_offset_by_arg_zero() {
    let mut env = Environment::new();
    env.set_positional(vec!["a".into(), "b".into()]);
    let zero = env.get_param("0").expect("$0 is always set");
    let got = expand_param(&mut env, "@", &slice("0", None), true).unwrap();
    let texts: Vec<String> = got.iter().map(|f| field_text(f)).collect();
    assert_eq!(texts, vec![zero, "a".to_string(), "b".to_string()]);
}

/// The twin of the array rule: `${@#pat}` rewrites each argument, and `$0` takes no part.
#[test]
fn a_pattern_operator_maps_over_the_positionals() {
    let op = ParamExpansion::RemovePrefix {
        pattern: lit("a"),
        longest: false,
    };
    let got = positional(&["a.c", "b.c"], "@", op);
    assert_eq!(got, Ok(vec![".c".into(), "b.c".into()]));

    let op = ParamExpansion::CaseConvert {
        pattern: None,
        upper: true,
        all: true,
    };
    assert_eq!(positional(&["ab", "cd"], "*", op), Ok(vec!["AB CD".into()]));
}

#[test]
fn length_counts_characters_and_positionals() {
    let len = |value: &str| {
        expand(
            &[("LC_ALL", "C.UTF-8"), ("s", value)],
            "s",
            ParamExpansion::Length,
        )
    };
    assert_eq!(len("hello"), Ok("5".into()));
    // Characters, not bytes, in a UTF-8 locale; bytes in the C one.
    assert_eq!(len("héllo"), Ok("5".into()));
    let bytes = expand(&[("LC_ALL", "C"), ("s", "héllo")], "s", ParamExpansion::Length);
    assert_eq!(bytes, Ok("6".into()));
    assert_eq!(expand(&[], "nope", ParamExpansion::Length), Ok("0".into()));

    let mut env = Environment::new();
    env.set_positional(vec!["a".into(), "bb".into(), "ccc".into()]);
    // `${#@}` is the count, not the width of the joined string.
    let count = expand_to_string(&mut env, "@", &ParamExpansion::Length).unwrap();
    assert_eq!(count, "3");
    // And `${#}` is `$#`, which is the same number.
    let count = expand_to_string(&mut env, "#", &ParamExpansion::Normal).unwrap();
    assert_eq!(count, "3");
}

/// The colon-less forms test only for unset; the `:` forms also reject a null value.
#[test]
fn the_colon_decides_whether_null_counts_as_set() {
    // `x` is set but null throughout: `${x:-d}` substitutes, `${x-d}` does not.
    for (test_null, expected) in [(true, "d"), (false, "")] {
        let op = ParamExpansion::DefaultValue {
            default: lit("d"),
            assign_if_unset: false,
            test_null,
        };
        assert_eq!(expand(&[("x", "")], "x", op), Ok(expected.to_string()));
    }
    for test_null in [true, false] {
        let op = ParamExpansion::UseAlternative {
            alternative: lit("s"),
            test_null,
        };
        let expected = if test_null { "" } else { "s" };
        assert_eq!(expand(&[("x", "")], "x", op), Ok(expected.to_string()));
    }
}

/// R2.6: the payload is a word, so it expands.
#[test]
fn a_default_payload_is_expanded() {
    let op = ParamExpansion::DefaultValue {
        default: var_word("d"),
        assign_if_unset: false,
        test_null: true,
    };
    assert_eq!(expand(&[("d", "/tmp/x")], "v", op), Ok("/tmp/x".into()));
}

/// R2.6: and the assigning form persists the *expanded* text, not the source.
#[test]
fn the_assigning_form_stores_the_expanded_text() {
    let mut env = Environment::new();
    env.set_var("y", "value", false);
    let op = ParamExpansion::DefaultValue {
        default: var_word("y"),
        assign_if_unset: true,
        test_null: true,
    };
    assert_eq!(expand_to_string(&mut env, "x", &op).unwrap(), "value");
    assert_eq!(env.get_param("x"), Some("value".to_string()));
}

#[test]
fn error_if_unset_reports_the_expanded_message() {
    let op = ParamExpansion::ErrorIfUnset {
        message: var_word("m"),
        test_null: true,
    };
    let err = expand(&[("m", "is unset")], "v", op).unwrap_err();
    assert!(err.contains("is unset"), "{err}");
}

#[test]
fn prefix_and_suffix_operators_use_patterns() {
    let p = "/usr/local/lib/libfoo.so";
    let strip = |pattern: &str, longest, prefix| {
        let op = if prefix {
            ParamExpansion::RemovePrefix {
                pattern: lit(pattern),
                longest,
            }
        } else {
            ParamExpansion::RemoveSuffix {
                pattern: lit(pattern),
                longest,
            }
        };
        expand(&[("p", p)], "p", op).unwrap()
    };
    assert_eq!(strip("*/", true, true), "libfoo.so");
    assert_eq!(strip("/*", false, false), "/usr/local/lib");
    assert_eq!(strip(".*", false, false), "/usr/local/lib/libfoo");
}

/// The pattern is a word too, so `${p##$sep}` works.
#[test]
fn a_strip_pattern_is_expanded() {
    let op = ParamExpansion::RemovePrefix {
        pattern: var_word("sep"),
        longest: true,
    };
    let vars = [("p", "a/b/c"), ("sep", "*/")];
    assert_eq!(expand(&vars, "p", op), Ok("c".into()));
}

#[test]
fn substring_reads_arithmetic_operands() {
    let op = ParamExpansion::Substring {
        offset: lit("1+1"),
        length: Some(lit("3")),
    };
    assert_eq!(expand(&[("v", "abcdefgh")], "v", op), Ok("cde".into()));

    let op = ParamExpansion::Substring {
        offset: lit(" -3"),
        length: None,
    };
    assert_eq!(expand(&[("v", "abcdefgh")], "v", op), Ok("fgh".into()));

    // bash makes a window that ends before it starts a fatal error, not an empty string.
    let op = ParamExpansion::Substring {
        offset: lit("2"),
        length: Some(lit("-9")),
    };
    assert!(expand(&[("v", "abcdefgh")], "v", op).is_err());
}

#[test]
fn replacement_covers_every_scope() {
    let scopes = [
        (ReplaceScope::First, "-", "+", "a+b-c"),
        (ReplaceScope::All, "-", "+", "a+b+c"),
        (ReplaceScope::Prefix, "a", "A", "A-b-c"),
        (ReplaceScope::Suffix, "c", "C", "a-b-C"),
    ];
    for (scope, pat, rep, expected) in scopes {
        let op = ParamExpansion::Replace {
            pattern: lit(pat),
            replacement: lit(rep),
            scope,
        };
        assert_eq!(expand(&[("v", "a-b-c")], "v", op), Ok(expected.to_string()));
    }
}

#[test]
fn case_conversion_covers_both_directions() {
    let cases = [
        (true, true, "hello", "HELLO"),
        (true, false, "hello", "Hello"),
        (false, true, "WORLD", "world"),
        (false, false, "WORLD", "wORLD"),
    ];
    for (upper, all, value, expected) in cases {
        let op = ParamExpansion::CaseConvert {
            pattern: None,
            upper,
            all,
        };
        assert_eq!(expand(&[("v", value)], "v", op), Ok(expected.to_string()));
    }
}

/// `oslo_ref`, not `name`: [`Environment::new`] inherits the real environment, and a plain
/// word like `name` is genuinely exported by some development shells — which would make the
/// unset case below pass or fail depending on who ran the tests.
#[test]
fn indirection_follows_the_named_parameter() {
    let vars = [("oslo_target", "payload"), ("oslo_ref", "oslo_target")];
    let got = expand(
        &vars,
        "oslo_ref",
        ParamExpansion::Indirect(Box::new(ParamExpansion::Normal)),
    );
    assert_eq!(got, Ok("payload".into()));
    // Only the *inner* parameter may be unset; that is an empty string, not an error.
    let vars = [("oslo_ref", "oslo_nosuchvar")];
    let got = expand(
        &vars,
        "oslo_ref",
        ParamExpansion::Indirect(Box::new(ParamExpansion::Normal)),
    );
    assert_eq!(got, Ok(String::new()));
    // bash aborts the expansion when the referring parameter is unset, or holds anything
    // that is not a name — including the empty string.
    for vars in [vec![], vec![("oslo_ref", "")], vec![("oslo_ref", "a b")]] {
        let got = expand(
            &vars,
            "oslo_ref",
            ParamExpansion::Indirect(Box::new(ParamExpansion::Normal)),
        );
        assert!(got.is_err(), "{vars:?} expanded to {got:?}");
    }
}

/// `${!v<op>}` — the indirection and the operator compose, and the operator applies to the
/// parameter the *first* one names.
///
/// stdenv's `runHook` is written `${!hooksSlice+"${!hooksSlice}"}`, so a shell without this cannot
/// run a dev shell's hooks at all. Every case here was checked against bash.
#[test]
fn an_indirection_composes_with_an_operator() {
    let vars = [("s", "greeting"), ("greeting", "hi")];
    let indirect = |inner| ParamExpansion::Indirect(Box::new(inner));

    // The alternative tests the *target*, not the pointer.
    assert_eq!(
        expand(
            &vars,
            "s",
            indirect(ParamExpansion::UseAlternative {
                alternative: Word::from_literal("yes"),
                test_null: false,
            })
        ),
        Ok("yes".to_string())
    );
    // A default falls through to the target's value when it is set.
    assert_eq!(
        expand(
            &vars,
            "s",
            indirect(ParamExpansion::DefaultValue {
                default: Word::from_literal("no"),
                assign_if_unset: false,
                test_null: true,
            })
        ),
        Ok("hi".to_string())
    );
    // And a pointer to something unset takes the default.
    assert_eq!(
        expand(
            &[("s", "nothere")],
            "s",
            indirect(ParamExpansion::DefaultValue {
                default: Word::from_literal("no"),
                assign_if_unset: false,
                test_null: true,
            })
        ),
        Ok("no".to_string())
    );
}

/// `${!s}` where `s` holds `name[@]` — the indirection resolves to an *array reference*, which is
/// how stdenv's `runHook` reaches its hook list. Checked against bash.
#[test]
fn an_indirection_may_name_an_array() {
    let mut env = Environment::new();
    env.set_array("h", crate::env::scope::ShellArray::from_values(["p", "q"]));
    env.set_var("s", "h[@]", false);
    let indirect = |inner| ParamExpansion::Indirect(Box::new(inner));

    assert_eq!(
        expand_to_string(&mut env, "s", &indirect(ParamExpansion::Normal))
            .map_err(|e| e.to_string()),
        Ok("p q".to_string())
    );
    // The `+` form is what `runHook` is written with: set, so the alternative wins.
    assert_eq!(
        expand_to_string(
            &mut env,
            "s",
            &indirect(ParamExpansion::UseAlternative {
                alternative: Word::from_literal("yes"),
                test_null: false,
            })
        )
        .map_err(|e| e.to_string()),
        Ok("yes".to_string())
    );
    // And an array nothing ever created is unset, so it answers with nothing rather than erroring.
    env.set_var("s", "missing[@]", false);
    assert_eq!(
        expand_to_string(
            &mut env,
            "s",
            &indirect(ParamExpansion::UseAlternative {
                alternative: Word::from_literal("yes"),
                test_null: false,
            })
        )
        .map_err(|e| e.to_string()),
        Ok(String::new())
    );
}

/// An unset *positional* used as the pointer is empty, not an error — `${!1}` in a function called
/// with no arguments, which is the first line of `runHook`, `runPhase` and `substituteStream`.
#[test]
fn an_unset_positional_pointer_is_empty() {
    let mut env = Environment::new();
    assert_eq!(
        expand_to_string(
            &mut env,
            "1",
            &ParamExpansion::Indirect(Box::new(ParamExpansion::Normal))
        )
        .map_err(|e| e.to_string()),
        Ok(String::new())
    );
    // A named variable that does not exist is still a mistake worth reporting, as in bash.
    assert!(
        expand_to_string(
            &mut env,
            "nothing",
            &ParamExpansion::Indirect(Box::new(ParamExpansion::Normal))
        )
        .is_err()
    );
}
