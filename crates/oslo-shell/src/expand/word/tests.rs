use super::{Origin, Run, expand_word, expand_word_fields};
use crate::env::Environment;
use oslo_base::ast::{Word, WordPart};

fn word(parts: Vec<WordPart>) -> Word {
    Word { parts }
}

fn texts(fields: &[Vec<Run>]) -> Vec<String> {
    fields.iter().map(|f| super::field_text(f)).collect()
}

#[test]
fn quoted_and_unquoted_runs_keep_their_origins() {
    let mut env = Environment::new();
    let w = word(vec![
        WordPart::DoubleQuoted(vec![WordPart::Literal("a".into())]),
        WordPart::Literal("*".into()),
    ]);
    let fields = expand_word_fields(&mut env, &w).unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0][0].origin, Origin::Quoted);
    assert_eq!(fields[0][1].origin, Origin::Literal);
}

#[test]
fn escaped_characters_are_quoted() {
    let mut env = Environment::new();
    let w = word(vec![
        WordPart::Literal("a".into()),
        WordPart::Escaped(" ".into()),
        WordPart::Literal("b".into()),
    ]);
    // The escaped space must neither split the field nor be treated as IFS whitespace.
    assert_eq!(expand_word(&mut env, &w).unwrap(), vec!["a b".to_string()]);
}

#[test]
fn quoted_at_is_one_field_per_positional() {
    let mut env = Environment::new();
    env.set_positional(vec!["a b".into(), "c".into(), String::new()]);
    let w = word(vec![WordPart::DoubleQuoted(vec![WordPart::Variable {
        name: "@".into(),
        expansion_type: oslo_base::ast::ParamExpansion::Normal,
    }])]);
    assert_eq!(expand_word(&mut env, &w).unwrap(), vec!["a b", "c", ""]);
}

#[test]
fn quoted_at_with_no_positionals_yields_no_field() {
    let mut env = Environment::new();
    env.set_positional(Vec::new());
    let w = word(vec![WordPart::DoubleQuoted(vec![WordPart::Variable {
        name: "@".into(),
        expansion_type: oslo_base::ast::ParamExpansion::Normal,
    }])]);
    assert!(expand_word(&mut env, &w).unwrap().is_empty());
}

/// `x"$@"y` splices: the first positional joins `x`, the last joins `y`.
#[test]
fn at_splices_into_its_neighbours() {
    let mut env = Environment::new();
    env.set_positional(vec!["1".into(), "2".into(), "3".into()]);
    let w = word(vec![
        WordPart::Literal("x".into()),
        WordPart::DoubleQuoted(vec![WordPart::Variable {
            name: "@".into(),
            expansion_type: oslo_base::ast::ParamExpansion::Normal,
        }]),
        WordPart::Literal("y".into()),
    ]);
    let fields = expand_word_fields(&mut env, &w).unwrap();
    assert_eq!(texts(&fields), vec!["x1", "2", "3y"]);
}

/// With nothing to splice, the neighbours still join into one field.
#[test]
fn empty_at_leaves_its_neighbours_joined() {
    let mut env = Environment::new();
    env.set_positional(Vec::new());
    let w = word(vec![
        WordPart::Literal("x".into()),
        WordPart::DoubleQuoted(vec![WordPart::Variable {
            name: "@".into(),
            expansion_type: oslo_base::ast::ParamExpansion::Normal,
        }]),
        WordPart::Literal("y".into()),
    ]);
    assert_eq!(expand_word(&mut env, &w).unwrap(), vec!["xy".to_string()]);
}

#[test]
fn empty_double_quotes_are_an_empty_argument() {
    let mut env = Environment::new();
    let w = word(vec![WordPart::DoubleQuoted(Vec::new())]);
    assert_eq!(expand_word(&mut env, &w).unwrap(), vec![String::new()]);
}

#[test]
fn unset_unquoted_parameter_yields_no_field() {
    let mut env = Environment::new();
    env.unset_var("OSLO_NO_SUCH_VAR");
    let w = word(vec![WordPart::Variable {
        name: "OSLO_NO_SUCH_VAR".into(),
        expansion_type: oslo_base::ast::ParamExpansion::Normal,
    }]);
    assert!(expand_word(&mut env, &w).unwrap().is_empty());
}

/// `set -f` must not merely fail to match — it must never consult the filesystem at all, so
/// the pattern survives verbatim even where it *would* have matched.
///
/// **The pattern is absolute, against files this test made.** It used to glob `Cargo.*` and
/// rely on the unit tests running in a directory where that matched exactly two things. That
/// held until the code moved into a crate of its own, whose root has a `Cargo.toml` and no
/// `Cargo.lock` — one match, and a failure that says nothing about globbing. A test that
/// depends on the working directory's contents is a test that moves house badly.
#[test]
fn noglob_leaves_the_pattern_alone() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("thing.one"), b"").expect("write");
    std::fs::write(dir.path().join("thing.two"), b"").expect("write");
    let pattern = format!("{}/thing.*", dir.path().display());

    let mut env = Environment::new();
    let w = word(vec![WordPart::Literal(pattern.clone())]);
    assert_eq!(expand_word(&mut env, &w).unwrap().len(), 2);
    env.set_option(crate::env::options::ShellOption::NoGlob, true);
    assert_eq!(expand_word(&mut env, &w).unwrap(), vec![pattern]);
}

#[test]
fn nounset_rejects_an_unset_parameter() {
    let mut env = Environment::new();
    env.set_option(crate::env::options::ShellOption::NoUnset, true);
    let w = word(vec![WordPart::Variable {
        name: "OSLO_NO_SUCH_VAR".into(),
        expansion_type: oslo_base::ast::ParamExpansion::Normal,
    }]);
    let err = expand_word(&mut env, &w).unwrap_err().to_string();
    assert!(err.contains("unbound variable"), "{err}");
}

/// The exemptions matter more than the rule: `${x-default}` is how a script reads a
/// possibly-unset variable *under* `set -u`, and `"$@"` with no arguments is not an error.
#[test]
fn nounset_exempts_the_defaulting_operators_and_the_argument_list() {
    let mut env = Environment::new();
    env.set_option(crate::env::options::ShellOption::NoUnset, true);
    env.set_positional(Vec::new());

    let defaulted = word(vec![WordPart::Variable {
        name: "OSLO_NO_SUCH_VAR".into(),
        expansion_type: oslo_base::ast::ParamExpansion::DefaultValue {
            default: Word {
                parts: vec![WordPart::Literal("d".into())],
            },
            assign_if_unset: false,
            test_null: false,
        },
    }]);
    assert_eq!(expand_word(&mut env, &defaulted).unwrap(), vec!["d"]);

    let args = word(vec![WordPart::DoubleQuoted(vec![WordPart::Variable {
        name: "@".into(),
        expansion_type: oslo_base::ast::ParamExpansion::Normal,
    }])]);
    assert!(expand_word(&mut env, &args).unwrap().is_empty());
}

/// nounset tests for *unset*, not for null: `x=; echo $x` is silent even under `set -u`.
#[test]
fn nounset_accepts_a_variable_that_is_set_but_empty() {
    let mut env = Environment::new();
    env.set_var("OSLO_EMPTY_VAR", "", false);
    env.set_option(crate::env::options::ShellOption::NoUnset, true);
    let w = word(vec![WordPart::Variable {
        name: "OSLO_EMPTY_VAR".into(),
        expansion_type: oslo_base::ast::ParamExpansion::Normal,
    }]);
    assert!(expand_word(&mut env, &w).unwrap().is_empty());
}

#[test]
fn literal_text_is_never_split_on_ifs() {
    let mut env = Environment::new();
    env.set_var("IFS", ":", false);
    let w = word(vec![WordPart::Literal("a:b:c".into())]);
    assert_eq!(
        expand_word(&mut env, &w).unwrap(),
        vec!["a:b:c".to_string()]
    );
}

#[test]
fn expansion_output_is_split_on_ifs() {
    let mut env = Environment::new();
    env.set_var("IFS", ":", false);
    env.set_var("V", "a:b:c", false);
    let w = word(vec![WordPart::Variable {
        name: "V".into(),
        expansion_type: oslo_base::ast::ParamExpansion::Normal,
    }]);
    assert_eq!(expand_word(&mut env, &w).unwrap(), vec!["a", "b", "c"]);
}
