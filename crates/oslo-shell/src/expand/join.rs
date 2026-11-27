//! Joining a list expansion where the context wants one string.
//!
//! An assignment, a here-string, a here-document, a `case` word and a `[[ ]]` operand are never
//! field-split, so a list expansion in one of them is joined back into a single string. bash
//! joins on a **space** for `"$@"` and `${a[@]}`, whatever IFS holds, and differently for two
//! other list forms:
//!
//! ```text
//!   IFS=,; set -- a b; a=(a b)
//!   x="$@"         a b          x="${a[@]}"      a b
//!   x="${@:1}"     a,b          x="${a[@]#z}"    a,b     a quoted operator form: Seam
//!   IFS=; x=$*     ab           [[ $* == ab ]]           an unquoted `$*`, null IFS: Glue
//! ```
//!
//! Each field after the first of those forms opens with an empty run whose [`Origin`] is the
//! mark, and `join_fields` reads it. A `Seam` joins on IFS's first character (a space when IFS
//! is null), and only an assignment or a here-string honours it: `[[ ]]` and `case` join on a
//! space. A `Glue` joins with nothing, everywhere.

use crate::env::Environment;
use crate::expand::word::{Field, Origin, Run, field_text};
use oslo_base::ast::{Word, WordPart};
use oslo_base::error::Result;

/// Open every field after the first with an empty run of `mark`.
pub(crate) fn marked(fields: Vec<Field>, mark: Origin) -> Vec<Field> {
    fields
        .into_iter()
        .enumerate()
        .map(|(i, mut field)| {
            if i > 0 {
                field.insert(0, Run::new("", mark));
            }
            field
        })
        .collect()
}

/// Whether IFS is set and empty, which makes an unquoted `$*` one field per element.
pub(crate) fn ifs_is_null(env: &Environment) -> bool {
    env.get_var("IFS") == Some("")
}

/// Which single-string context is joining.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Joining {
    /// An assignment, a here-string, a declaration's operand: a `Seam` is honoured.
    Value,
    /// A `[[ ]]` operand or a `case` word: a `Seam` is a space.
    Test,
}

/// What goes between `field` and the field before it.
fn separator(env: &Environment, field: &[Run], joining: Joining) -> String {
    match field.first().map(|run| run.origin) {
        Some(Origin::Glue) => String::new(),
        Some(Origin::Seam) if joining == Joining::Value => {
            let sep = env.ifs_separator();
            if sep.is_empty() { " ".to_string() } else { sep }
        }
        _ => " ".to_string(),
    }
}

/// The fields of a list expansion as the one string a non-splitting context sees.
pub(crate) fn join_fields(env: &Environment, fields: &[Field], joining: Joining) -> String {
    let mut out = String::new();
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push_str(&separator(env, field, joining));
        }
        out.push_str(&field_text(field));
    }
    out
}

/// [`join_fields`] for a pattern, keeping the runs: the separators are quoted text.
pub(crate) fn join_runs(env: &Environment, fields: Vec<Field>) -> Vec<Run> {
    let mut out = Vec::new();
    for (i, field) in fields.into_iter().enumerate() {
        if i > 0 {
            out.push(Run::new(
                separator(env, &field, Joining::Value),
                Origin::Quoted,
            ));
        }
        out.extend(field);
    }
    out
}

/// Expand a here-document body.
///
/// `$*` there joins on a space whatever IFS holds, unless IFS is null, which is how `$@` joins;
/// so under a non-null IFS each `$*` is expanded as `$@`.
pub fn expand_heredoc_to_string(env: &mut Environment, body: &Word) -> Result<String> {
    let is_star = |part: &WordPart| matches!(part, WordPart::Variable { name, .. } if name == "*");
    if ifs_is_null(env) || !body.parts.iter().any(is_star) {
        return crate::expand::word::expand_word_to_string(env, body);
    }
    let parts = body.parts.iter().map(|part| match part {
        WordPart::Variable {
            name,
            expansion_type,
        } if name == "*" => WordPart::Variable {
            name: "@".to_string(),
            expansion_type: expansion_type.clone(),
        },
        other => other.clone(),
    });
    let body = Word {
        parts: parts.collect(),
    };
    crate::expand::word::expand_word_to_string(env, &body)
}
