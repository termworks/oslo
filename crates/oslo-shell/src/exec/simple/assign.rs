//! Performing an assignment.
//!
//! Four shapes reach here — `name=v`, `name[i]=v`, `name=(…)` and any of them with `+=` — and the
//! rules they share are the ones that are easy to get subtly different, which is why they live in
//! one place:
//!
//! * the right-hand side is **not** field-split and **not** globbed (POSIX 2.9.1), so `x=*.rs`
//!   stores the pattern and `x=$(printf 'a\nb')` keeps its newline;
//! * an *array literal's* elements are the opposite — each element is a word that expands to as
//!   many fields as it wants, so `a=($list)` and `a=(*.c)` are how a script builds an array;
//! * a subscript is arithmetic, evaluated when the assignment runs.

use crate::env::Environment;
use crate::env::announce::{Change, Scope, Source};
use crate::env::scope::ShellArray;
use crate::expand::arithmetic::eval_arithmetic;
use crate::expand::{expand_word, expand_word_to_string};
use oslo_base::ast::{ArrayElement, Assignment, AssignmentTarget, AssignmentValue};
use oslo_base::error::{Result, ShellError};

/// What one assignment did.
///
/// `assigned` exists because the environment can *refuse*: a read-only name, or a name or value
/// `environ` cannot represent. `Environment::set_var` prints the reason and returns `false`, and
/// that `false` used to be discarded at the call site — so `readonly r=1; r=2; echo $?` printed
/// the diagnostic and then claimed success (PLAN C3). Returning it makes the caller decide, which
/// it must, because in POSIX mode the answer is that the shell stops.
pub(super) struct Outcome {
    /// What `set -x` traces; an array literal traces as the elements it expanded to, which is the
    /// information a reader of the trace actually wants.
    pub trace: String,
    /// Whether the variable now holds the value.
    pub assigned: bool,
}

impl Outcome {
    fn new(trace: impl Into<String>, assigned: bool) -> Self {
        Outcome {
            trace: trace.into(),
            assigned,
        }
    }
}

/// Apply one assignment to the shell's own variables, and say so.
///
/// **The announcement is here rather than in `set_var`**, which the shell also uses to maintain
/// `PWD`, `_` and everything else it keeps up to date on your behalf. This is the assignment the
/// user wrote, which is the one a config asked to hear about.
pub(super) fn apply_assignment(env: &mut Environment, assign: &Assignment) -> Result<Outcome> {
    let outcome = assigned(env, assign)?;
    if outcome.assigned {
        let name = assign.target.name();
        crate::env::announce::announce(
            name,
            Change::Set {
                exported: env.is_exported(name),
            },
            Scope::Shell,
            Source::Local,
        );
    }
    Ok(outcome)
}

/// An empty operand is a zero, so `()+(4)` — which is not an expression — never gets built.
fn or_zero(text: &str) -> &str {
    match text.trim().is_empty() {
        true => "0",
        false => text,
    }
}

fn assigned(env: &mut Environment, assign: &Assignment) -> Result<Outcome> {
    match (&assign.target, &assign.value) {
        (AssignmentTarget::Name(name), AssignmentValue::Scalar(word)) => {
            let value = expand_word_to_string(env, word)?;
            if assign.append {
                // `x+=b` on an *array* appends an element, as bash does; on a scalar it
                // concatenates. Which one it is depends on what the name already holds.
                if env.get_array(name).is_some() {
                    let ok = env.append_array_element(name, &value);
                    return Ok(Outcome::new(value, ok));
                }
                // **On an integer name `+=` adds.** `declare -i q=6; q+=4` is 10, not the string
                // `64` — and concatenating first would not merely look wrong, it would then be
                // *evaluated* as the number sixty-four by the assignment below.
                if env.is_integer(name) {
                    let existing = or_zero(env.get_var(name).unwrap_or_default());
                    let sum = format!("({existing})+({})", or_zero(&value));
                    let ok = env.set_var(name, &sum, false);
                    let stored = env.get_var(name).unwrap_or_default().to_string();
                    return Ok(Outcome::new(stored, ok));
                }
                let value = format!("{}{}", env.get_var(name).unwrap_or_default(), value);
                let ok = env.set_var(name, &value, false);
                return Ok(Outcome::new(value, ok));
            }
            let ok = env.set_var(name, &value, false);
            Ok(Outcome::new(value, ok))
        }

        (AssignmentTarget::Element { name, index }, AssignmentValue::Scalar(word)) => {
            let index = eval_subscript(env, index)?;
            let mut value = expand_word_to_string(env, word)?;
            if assign.append {
                let existing = env.get_array(name).and_then(|a| a.get(index)).unwrap_or("");
                value = format!("{existing}{value}");
            }
            let ok = env.set_array_element(name, index, &value);
            Ok(Outcome::new(value, ok))
        }

        (AssignmentTarget::Name(name), AssignmentValue::Array(elements)) => {
            let array = build_array(env, name, elements, assign.append)?;
            // Parenthesised for the `set -x` trace: `+ a=(1 2)` reads as a list, where the bare
            // `1 2` would look like a scalar holding a space.
            let joined = format!("({})", array.joined(" "));
            // Inside a function `a=(…)` is still global unless `local`/`declare` said otherwise,
            // which is what `set_array` does; `set_local_array` is the declaration builtins' path.
            let ok = env.set_array(name, array);
            Ok(Outcome::new(joined, ok))
        }

        // `a[0]=(1 2)` — bash rejects this too ("cannot assign list to array member"). Refusing is
        // the point: silently storing the source text is what this round exists to remove.
        (AssignmentTarget::Element { name, .. }, AssignmentValue::Array(_)) => Err(
            ShellError::ExpansionError(format!("{name}: cannot assign a list to an array element")),
        ),
    }
}

/// Build the array an `a=(…)` or `a+=(…)` literal denotes.
///
/// Unindexed elements go to the next free index, so `a=(x [5]=y z)` puts `z` at 6 — bash's rule,
/// and the reason the running index is taken from the array rather than from the element count.
pub(crate) fn build_array(
    env: &mut Environment,
    name: &str,
    elements: &[ArrayElement],
    append: bool,
) -> Result<ShellArray> {
    let mut array = if append {
        env.get_array(name).cloned().unwrap_or_default()
    } else {
        ShellArray::default()
    };

    for element in elements {
        match &element.index {
            Some(index) => {
                let index = eval_subscript(env, index)?;
                array.set(index, expand_word_to_string(env, &element.value)?);
            }
            // Not `expand_word_to_string`: an element of an array literal is an ordinary word in
            // list context, so `a=($list)` splits and `a=(*.c)` globs, both into separate
            // elements. That is the difference between an array literal and a scalar assignment.
            None => {
                for field in expand_word(env, &element.value)? {
                    array.push(field);
                }
            }
        }
    }

    Ok(array)
}

/// Evaluate a subscript as arithmetic: `a[i+1]=x` writes where `i+1` points when it runs.
fn eval_subscript(env: &mut Environment, word: &oslo_base::ast::Word) -> Result<i64> {
    let text = expand_word_to_string(env, word)?;
    let text = text.trim();
    if text.is_empty() {
        return Err(ShellError::ExpansionError(
            "bad array subscript".to_string(),
        ));
    }
    eval_arithmetic(env, text)
}

#[cfg(test)]
mod tests {
    use crate::env::Environment;

    /// Run a snippet and report what `name` ended up holding, elements joined by a space.
    fn array_of(src: &str, name: &str) -> String {
        let mut env = Environment::new();
        let script = crate::syntax::parse_bash_script(src).expect("parse");
        crate::exec::eval_command_list(&mut env, &script).expect("exec");
        env.get_array(name)
            .map(|a| a.joined(" "))
            .unwrap_or_else(|| format!("<not an array: {:?}>", env.get_var(name)))
    }

    #[test]
    fn an_array_literal_stores_its_elements() {
        assert_eq!(array_of("oslo_x1=(1 2 3)", "oslo_x1"), "1 2 3");
        assert_eq!(array_of("oslo_x2=()", "oslo_x2"), "");
    }

    /// The elements are words in list context: an unquoted expansion splits into several.
    #[test]
    fn an_unquoted_element_splits_into_several() {
        let src = "oslo_l='a b c'\noslo_x3=($oslo_l)";
        assert_eq!(array_of(src, "oslo_x3"), "a b c");
        // …and a quoted one does not.
        let src = "oslo_l='a b c'\noslo_x4=(\"$oslo_l\" d)";
        assert_eq!(array_of(src, "oslo_x4"), "a b c d");
    }

    #[test]
    fn an_explicit_index_moves_the_running_position() {
        let mut env = Environment::new();
        let script = crate::syntax::parse_bash_script("oslo_x5=(a [5]=b c)").expect("parse");
        crate::exec::eval_command_list(&mut env, &script).expect("exec");
        let array = env.get_array("oslo_x5").expect("an array");
        assert_eq!(array.indices().collect::<Vec<_>>(), vec![0, 5, 6]);
    }

    #[test]
    fn append_extends_an_array_and_concatenates_a_scalar() {
        assert_eq!(array_of("oslo_x6=(a b)\noslo_x6+=(c)", "oslo_x6"), "a b c");
        let mut env = Environment::new();
        let script = crate::syntax::parse_bash_script("oslo_x7=a\noslo_x7+=b").expect("parse");
        crate::exec::eval_command_list(&mut env, &script).expect("exec");
        assert_eq!(env.get_var("oslo_x7"), Some("ab"));
    }

    /// An element assignment must write the element, not a variable whose name contains brackets.
    #[test]
    fn an_element_assignment_writes_an_element() {
        assert_eq!(array_of("oslo_x8[2]=y", "oslo_x8"), "y");
        let mut env = Environment::new();
        let script = crate::syntax::parse_bash_script("oslo_x9[2]=y").expect("parse");
        crate::exec::eval_command_list(&mut env, &script).expect("exec");
        assert_eq!(env.get_var("oslo_x9[2]"), None);
        assert_eq!(env.get_array("oslo_x9").unwrap().get(2), Some("y"));
    }

    /// A refused assignment reports failure and leaves the old value in place, in every shape.
    ///
    /// The status the shell reports for it is `exec::simple`'s decision, not this module's; what
    /// is asserted here is that the `false` reaches the caller at all, which is the bug: it used
    /// to be dropped, so the shell said the assignment had worked.
    #[test]
    fn a_read_only_name_refuses_every_shape_of_assignment() {
        use oslo_base::ast::{
            ArrayElement, Assignment, AssignmentTarget, AssignmentValue, Word as AstWord,
        };

        let mut env = Environment::new();
        env.set_var("oslo_ro", "1", false);
        env.set_readonly("oslo_ro");

        let scalar = Assignment::scalar("oslo_ro", AstWord::from_literal("2"));
        let mut appended = Assignment::scalar("oslo_ro", AstWord::from_literal("x"));
        appended.append = true;
        let literal = Assignment {
            target: AssignmentTarget::Name("oslo_ro".to_string()),
            value: AssignmentValue::Array(vec![ArrayElement {
                index: None,
                value: AstWord::from_literal("a"),
            }]),
            append: false,
        };
        let element = Assignment {
            target: AssignmentTarget::Element {
                name: "oslo_ro".to_string(),
                index: AstWord::from_literal("3"),
            },
            value: AssignmentValue::Scalar(AstWord::from_literal("z")),
            append: false,
        };

        for assignment in [&scalar, &appended, &literal, &element] {
            let outcome = super::apply_assignment(&mut env, assignment).expect("evaluates");
            assert!(!outcome.assigned, "{assignment:?} should have been refused");
        }
        assert_eq!(env.get_var("oslo_ro"), Some("1"));
    }

    /// A name or value `environ` cannot hold is refused the same way, and it is not a read-only
    /// name — so no caller may read "refused" as "read-only".
    #[test]
    fn an_unrepresentable_value_is_also_a_refusal() {
        let mut env = Environment::new();
        assert!(!env.set_var("oslo_nul", "a\0b", false));
        assert!(!env.is_readonly("oslo_nul"));
    }

    /// The subscript is arithmetic, evaluated when the assignment runs.
    #[test]
    fn a_subscript_is_arithmetic() {
        let src = "oslo_i=1\noslo_xa[oslo_i+1]=z";
        let mut env = Environment::new();
        let script = crate::syntax::parse_bash_script(src).expect("parse");
        crate::exec::eval_command_list(&mut env, &script).expect("exec");
        assert_eq!(env.get_array("oslo_xa").unwrap().get(2), Some("z"));
    }
}
