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
//! * a subscript is arithmetic for an indexed array and a string for an associative one,
//!   evaluated when the assignment runs.

use super::trace::{assignment_for, list_assignment_text, trace_assignment};
use crate::env::Environment;
use crate::env::announce::{Change, Scope, Source};
use crate::env::builtins::arrays;
use crate::env::builtins::format_word;
use crate::expand::expand_word_to_string;
use oslo_base::ast::{Assignment, AssignmentTarget, AssignmentValue};
use oslo_base::error::{Result, ShellError};

/// What one assignment did.
///
/// `assigned` exists because the environment can *refuse*: a read-only name, or a name or value
/// `environ` cannot represent. `Environment::set_var` prints the reason and returns `false`, and
/// that `false` used to be discarded at the call site — so `readonly r=1; r=2; echo $?` printed
/// the diagnostic and then claimed success (PLAN C3). Returning it makes the caller decide, which
/// it must, because in POSIX mode the answer is that the shell stops.
pub(super) struct Outcome {
    /// Whether the variable now holds the value.
    pub assigned: bool,
}

impl Outcome {
    fn new(assigned: bool) -> Self {
        Outcome { assigned }
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

/// A subscript that names no element, or one whose arithmetic fails, abandons the command the way
/// a refused assignment does: reported here, then unwound as [`ShellError::Abandoned`].
fn abandoning<T>(env: &Environment, result: Result<T>) -> Result<T> {
    result.map_err(|e| match e {
        ShellError::ExpansionError(message) => {
            eprintln!("{}{message}", env.origin());
            ShellError::Abandoned
        }
        other => other,
    })
}

fn assigned(env: &mut Environment, assign: &Assignment) -> Result<Outcome> {
    match (&assign.target, &assign.value) {
        (AssignmentTarget::Name(name), AssignmentValue::Scalar(word)) => {
            let value = expand_word_to_string(env, word)?;
            // Traced once expanded and before it is made, as bash traces it.
            let traced = assignment_for(env, name, assign.append, &value);
            trace_assignment(env, &traced);
            // `x+=b` extends `$x`, which on an array is element 0; on an integer name it adds.
            let ok = match assign.append {
                true => arrays::append_scalar(env, name, &value),
                false => env.set_var(name, &value, false),
            };
            Ok(Outcome::new(ok))
        }

        (AssignmentTarget::Element { name, index }, AssignmentValue::Scalar(word)) => {
            let lhs = format!("{name}[{}]", format_word(index));
            let key = arrays::assigned_key(env, name, index);
            let key = abandoning(env, key)?;
            let value = expand_word_to_string(env, word)?;
            let traced = assignment_for(env, &lhs, assign.append, &value);
            trace_assignment(env, &traced);
            let written = arrays::write_element(env, name, key, &value, assign.append);
            Ok(Outcome::new(abandoning(env, written)?))
        }

        (AssignmentTarget::Name(name), AssignmentValue::Array(elements)) => {
            // Traced as the list was written, as bash traces it.
            let written: Vec<String> = elements
                .iter()
                .map(|e| match &e.index {
                    Some(index) => format!("[{}]={}", format_word(index), format_word(&e.value)),
                    None => format_word(&e.value),
                })
                .collect();
            if env.is_assoc(&env.ref_target_name(name))
                && let Some(bare) = arrays::unkeyed_element(elements)
            {
                arrays::refuse_unkeyed(env, name, bare, false);
                return Err(ShellError::Abandoned);
            }
            let items = arrays::literal_items_from_ast(env, elements)?;
            let list = written.join(" ");
            trace_assignment(env, &list_assignment_text(name, assign.append, &list));
            // Inside a function `a=(…)` is still global unless `local`/`declare` said otherwise.
            let assigned = arrays::assign_literal(env, name, items, assign.append);
            Ok(Outcome::new(abandoning(env, assigned)?))
        }

        // `a[0]=(1 2)` — bash rejects this too ("cannot assign list to array member"). Refusing is
        // the point: silently storing the source text is what this round exists to remove.
        (AssignmentTarget::Element { name, .. }, AssignmentValue::Array(_)) => Err(
            ShellError::ExpansionError(format!("{name}: cannot assign a list to an array element")),
        ),
    }
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
                append: false,
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

    /// On an associative array the subscript is the key, quoted or expanded, never arithmetic.
    #[test]
    fn an_associative_subscript_is_its_expanded_text() {
        use oslo_base::ast::{Assignment, AssignmentTarget, AssignmentValue, Word, WordPart};
        let mut env = Environment::new();
        env.declare_assoc("oslo_m");
        env.set_var("oslo_k", "a b", false);
        let element = |index: Vec<WordPart>, value: &str| Assignment {
            target: AssignmentTarget::Element {
                name: "oslo_m".to_string(),
                index: Word { parts: index },
            },
            value: AssignmentValue::Scalar(Word::from_literal(value)),
            append: false,
        };
        let quoted = vec![WordPart::DoubleQuoted(vec![WordPart::Literal(
            "c d".into(),
        )])];
        let variable = vec![WordPart::Variable {
            name: "oslo_k".into(),
            expansion_type: oslo_base::ast::ParamExpansion::Normal,
        }];
        for (index, value) in [(quoted, "1"), (variable, "2")] {
            let outcome = super::apply_assignment(&mut env, &element(index, value)).unwrap();
            assert!(outcome.assigned);
        }
        let map = env.get_assoc("oslo_m").unwrap();
        assert_eq!((map.get("c d"), map.get("a b")), (Some("1"), Some("2")));
    }
}
