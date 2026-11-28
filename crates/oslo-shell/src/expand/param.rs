//! Parameter expansion: `$x`, `${x}`, and every `${x<op>word}` operator.
//!
//! Two rules shape this module.
//!
//! The first is that an *operand is a word, not a string*. `${x:-$HOME}` has to expand its
//! default and `${x:=$y}` has to persist the expanded text, so every payload arrives as a
//! [`oslo_base::ast::Word`] and is expanded here, with quote removal, when the operator needs it.
//!
//! The second is that an unrecognised `${...}` body is an **error**. The lexer hands anything it
//! cannot parse through as a parameter *name*; looking that up would answer `""` for every form
//! the shell does not implement, and a wrong-but-quiet empty string is precisely how nine
//! separate unimplemented operators went unnoticed. Rejecting a body that is not a name turns
//! that fallback into a diagnostic instead.

mod array;
pub mod case;
mod indirect;
mod lists;
mod operators;
mod pattern;
mod positional;
pub(crate) mod transform;
mod whole;

use crate::env::Environment;
use crate::env::builtins::arrays::Key;
use crate::expand::word::{Field, Origin, Run};
use oslo_base::ast::ParamExpansion;
use oslo_base::error::{Result, ShellError};

pub use array::expand_array_ref;

/// What an expansion reads from, and what the assigning `${x:=v}` forms write back to.
///
/// A scalar and an array element differ in exactly those two operations; every operator in
/// `operators` is written against this rather than against a variable name, so `${a[0]%.c}`
/// cannot drift away from `${v%.c}`.
pub(crate) enum Target<'a> {
    /// A parameter by name: a variable, a positional, or a special parameter.
    Param(&'a str),
    /// One element, with its subscript already evaluated: an index, or an associative key.
    Element { name: &'a str, key: Key },
}

impl Target<'_> {
    /// The value as it stands, or `None` when the parameter or element does not exist.
    fn value(&self, env: &Environment) -> Option<String> {
        match self {
            Self::Param(name) => env.get_param(name),
            Self::Element {
                name,
                key: Key::Name(key),
            } => env.get_assoc(name)?.get(key).map(str::to_string),
            // **A scalar is an array of one.** `v=x; echo ${v[0]}` answers `x` in bash.
            Self::Element {
                name,
                key: Key::Index(index),
            } => match env.get_array(name) {
                Some(array) => array.get(*index).map(str::to_string),
                None if *index == 0 => env.get_var(name).map(str::to_string),
                None => None,
            },
        }
    }

    /// Persist `text` — what `${x:=v}` does. An element assignment writes the element, not the
    /// whole variable, so `${a[2]:=x}` leaves the rest of the array alone.
    ///
    /// A positional or special parameter cannot be assigned this way, and neither can a readonly
    /// name; both are fatal expansion errors.
    fn assign(&self, env: &mut Environment, text: &str) -> Result<()> {
        let name = match self {
            Self::Param(name) | Self::Element { name, .. } => *name,
        };
        if !crate::env::scope::is_valid_identifier(name) {
            return Err(ShellError::MalformedExpansion(format!(
                "${name}: cannot assign in this way"
            )));
        }
        if env.is_readonly(name) {
            return Err(ShellError::MalformedExpansion(format!(
                "{name}: readonly variable"
            )));
        }
        match self {
            Self::Param(name) => {
                env.set_var(name, text, false);
            }
            Self::Element {
                name,
                key: Key::Index(index),
            } => {
                env.set_array_element(name, *index, text);
            }
            Self::Element {
                name,
                key: Key::Name(key),
            } => {
                env.set_assoc_element(name, key, text);
            }
        }
        Ok(())
    }

    /// How the reference is named in a diagnostic — as the script wrote it.
    fn display(&self) -> String {
        match self {
            Self::Param(name) => (*name).to_string(),
            Self::Element {
                name,
                key: Key::Index(index),
            } => format!("{name}[{index}]"),
            Self::Element {
                name,
                key: Key::Name(key),
            } => format!("{name}[{key}]"),
        }
    }
}

/// Expand one parameter reference into the fields it contributes.
///
/// `in_quotes` is the enclosing double-quote context: it decides both whether the result is
/// eligible for field splitting and, for `$@`, whether the positionals stay apart.
pub fn expand_param(
    env: &mut Environment,
    name: &str,
    expansion_type: &ParamExpansion,
    in_quotes: bool,
) -> Result<Vec<Field>> {
    let origin = origin_of(in_quotes);

    // A stored variable is a line until somebody reads the name, and a value from then on.
    materialise(env, name);

    // `$@` and `$*` are the only parameters that stand for a *list*, and only `$@` keeps its
    // members apart. Everything list-valued has to be answered here: falling through to the
    // scalar path below joins the positionals with a space first, which is how `"${@:2}"` came
    // to hand back one character-sliced field instead of the arguments it was forwarding.
    if matches!(name, "@" | "*") {
        if matches!(expansion_type, ParamExpansion::Normal) {
            return Ok(positional_fields(env, name, origin));
        }
        if let Some(fields) = array::positional_list(env, name, expansion_type, origin)? {
            return Ok(fields);
        }
        if let Some(fields) = positional::operator_fields(env, name, expansion_type, in_quotes)? {
            return Ok(fields);
        }
    }

    if let ParamExpansion::Indirect(inner) = expansion_type
        && let Some(fields) = indirect::fields(env, name, inner, in_quotes)?
    {
        return Ok(fields);
    }

    if let Some(fields) = payload_fields(env, name, expansion_type, in_quotes)? {
        return Ok(fields);
    }

    let text = expand_to_string(env, name, expansion_type)?;
    Ok(vec![vec![Run::new(text, origin)]])
}

/// Run the recipe behind a stored variable, the first time its name is read.
///
/// `oslo macros add --var GITHUB_TOKEN '$(oslo secret get gh-token)'` gives the shell that line and
/// nothing else. Here it becomes `export GITHUB_TOKEN=$(oslo secret get gh-token)` and is run — by
/// the shell's own parser and evaluator, because the alternative is deciding by hand whether the
/// body needs quoting, and an assignment is exactly the context where a command substitution does
/// *not* word-split. What the person wrote means what it would mean typed.
///
/// **Nothing runs until something asks.** A shell that never mentions the name never decrypts
/// anything, and the second mention finds an ordinary exported variable, because
/// [`Environment::take_lazy_var`] hands the recipe over once.
///
/// A failure is the command's own to report — it printed whatever it printed on its way to failing
/// — and leaves the variable unset, which is what an empty `$(…)` would have left anyway.
fn materialise(env: &mut Environment, name: &str) {
    // Already a value, or never a recipe: the common case, and one hash lookup.
    if env.get_var(name).is_some() || env.lazy_var_names().is_empty() {
        return;
    }
    let Some(body) = env.take_lazy_var(name) else {
        return;
    };
    // Parsed without alias substitution: the body is a value, and a stored alias called `git`
    // rewriting somebody's `$(git rev-parse)` would be a surprise nobody could see coming.
    let line = format!("export {name}={body}");
    if let Ok(ast) = crate::syntax::parse_with_aliases(&line, false, &|_| None) {
        let _ = crate::exec::eval_command_list(env, &ast);
    }
}

/// The fields a `${x-word}` or `${x+word}` payload contributes, when the payload is what wins.
///
/// These two operators are the only ones whose operand becomes the expansion's *result* rather
/// than a pattern, a subscript or a message — so the result can be more than one field.
/// `${1+"$@"}` is the reason this exists: it is the pre-POSIX way to forward arguments, still what
/// modernish's own diagnostics are written with, and collapsing it to a single string joined the
/// arguments on a space and then re-split that join on `IFS`. `printf '%s\n' ${1+"$@"}` printed
/// one line per *word* instead of one per argument.
///
/// `${x:=word}` is deliberately not here: it has to persist a single string, and routing it
/// through the fields path would leave the two halves of the operator disagreeing about what it
/// assigned.
fn payload_fields(
    env: &mut Environment,
    name: &str,
    expansion_type: &ParamExpansion,
    in_quotes: bool,
) -> Result<Option<Vec<Field>>> {
    // Read only for the two operators that need it: reading `$RANDOM` draws a number.
    if !matches!(
        expansion_type,
        ParamExpansion::UseAlternative { .. }
            | ParamExpansion::DefaultValue {
                assign_if_unset: false,
                ..
            }
    ) {
        return Ok(None);
    }
    let value = Target::Param(name).value(env);
    let payload = match expansion_type {
        ParamExpansion::UseAlternative {
            alternative,
            test_null,
        } if operators::is_present(&value, *test_null) => alternative,
        ParamExpansion::DefaultValue {
            default,
            assign_if_unset: false,
            test_null,
        } if !operators::is_present(&value, *test_null) => default,
        _ => return Ok(None),
    };
    payload_word_fields(env, payload, in_quotes).map(Some)
}

/// Expand the payload of `${x-word}` or `${x+word}` where it stands.
pub(super) fn payload_word_fields(
    env: &mut Environment,
    payload: &oslo_base::ast::Word,
    in_quotes: bool,
) -> Result<Vec<Field>> {
    let mut fields = crate::expand::word::expand_word_fields_in(env, payload, in_quotes)?;
    // A quoted expansion is one field even when it is empty: a payload with no parts expands to
    // no fields at all, so `"${x-}"` vanished instead of becoming an empty word. Unquoted it
    // really is zero fields, and `"$@"` with no positionals still is too — that is a word part in
    // its own right and never reaches here. `tests/corpus/expand_quoted_empty_default.sh` has the
    // rest, including the `[[ ]]` parse error this caused.
    if in_quotes && fields.is_empty() {
        fields.push(vec![Run::new("", Origin::Quoted)]);
    }
    if !in_quotes {
        // Unquoted, the payload's own literal text is part of an expansion result, and POSIX
        // splits results: `IFS=:; ${x-a:b}` is two fields. Only the payload's *quoted* runs are
        // exempt, and those already carry `Origin::Quoted`.
        for run in fields.iter_mut().flatten() {
            if run.origin == Origin::Literal {
                run.origin = Origin::Expanded;
            }
        }
    }
    Ok(fields)
}

/// Where an expansion's output came from, which decides whether it may be field-split.
fn origin_of(in_quotes: bool) -> Origin {
    if in_quotes {
        Origin::Quoted
    } else {
        Origin::Expanded
    }
}

/// The single string a `${name<op>}` reference stands for.
fn expand_to_string(
    env: &mut Environment,
    name: &str,
    expansion_type: &ParamExpansion,
) -> Result<String> {
    if !is_param_name(name) {
        return Err(ShellError::MalformedExpansion(format!(
            "${{{name}}}: bad substitution"
        )));
    }
    operators::apply(env, &Target::Param(name), expansion_type)
}

/// `${!v}` where `v` holds `name[subscript]` — one string, joined as `$*` would.
///
/// **The result is one field, not the array's.** An indirection is expanded in a string context by
/// the time it reaches here, and the caller wants a value rather than a list. `"${!s}"` in bash
/// with `s="a[@]"` is likewise a single word.
pub(crate) fn expand_indirect_array(
    env: &mut Environment,
    reference: &str,
    expansion_type: &ParamExpansion,
) -> Result<String> {
    let (name, subscript) = reference
        .split_once('[')
        .map(|(name, rest)| (name, rest.trim_end_matches(']')))
        .unwrap_or((reference, "@"));
    let parsed = match subscript {
        "@" => oslo_base::ast::Subscript::All,
        "*" => oslo_base::ast::Subscript::Joined,
        index => oslo_base::ast::Subscript::Index(oslo_base::ast::Word::from_literal(index)),
    };
    let fields = array::expand_array_ref(env, name, &parsed, expansion_type, true)?;
    Ok(fields
        .into_iter()
        .map(|field| {
            field
                .into_iter()
                .map(|run| run.text)
                .collect::<Vec<_>>()
                .join("")
        })
        .collect::<Vec<_>>()
        .join(" "))
}

/// Can `name` be looked up as a parameter at all?
///
/// This is the guard that turns an unimplemented `${...}` form into a diagnostic. The lexer's
/// fallback for a body it cannot parse is to treat the whole body as a name, and every body that
/// contains an operator fails here — `v:2:3`, `v/x/y`, `v^^` are not names.
fn is_param_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    // The special parameters, plus `$-`, are each a single character and none is a name.
    if name.chars().count() == 1 && matches!(first, '?' | '$' | '!' | '#' | '*' | '@' | '-') {
        return true;
    }
    // `${12}` is the twelfth positional: all digits, or a name.
    if first.is_ascii_digit() {
        return name.chars().all(|c| c.is_ascii_digit());
    }
    (first.is_alphabetic() || first == '_') && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// `$@` and `$*` as fields.
///
/// `"$@"` is one field per positional — the difference between forwarding `"a b" c` as two
/// arguments and as three — and *no* field when there are none, so `cmd "$@"` with nothing set
/// runs `cmd` rather than `cmd ""`. `$*` is always exactly one field, joined by the first
/// character of IFS.
fn positional_fields(env: &Environment, name: &str, origin: Origin) -> Vec<Field> {
    let star_apart = name == "*" && array::star_stays_apart(env, origin);
    let fields = array::list_fields(
        env.get_positional(),
        name == "@" || star_apart,
        &env.ifs_separator(),
        origin,
    );
    if star_apart {
        return crate::expand::join::marked(fields, Origin::Glue);
    }
    fields
}

#[cfg(test)]
#[path = "param/tests.rs"]
mod tests;
