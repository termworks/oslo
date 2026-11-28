//! `declare` / `typeset` and `local`, and the one engine they share with `readonly` and `export`.
//!
//! Each operand is `name`, `name=value`, `name+=value` or `name[subscript]=value`. Inside a
//! function `declare` and `local` give the name a binding of the function's own; `-g` reaches past
//! any local to the global one; `readonly` and `export` work on whichever binding is visible. The
//! attributes are applied before the value, so `declare -i n=2+3` stores 5, except `-r`, which is
//! applied last so `declare -r x=1` gets its value in first.

mod listing;
mod print;
mod quote;
#[cfg(test)]
mod tests;
mod written;

pub(crate) use print::{assignment, print_matching, set_line};
pub(crate) use quote::key as quote_key;
#[cfg(feature = "universal")]
pub(crate) use quote::set_style;
pub(crate) use written::{Written, forget_written, note_written};

use super::arrays::{self, assign_literal, literal_items};
use crate::env::scope::nameref::is_reference_name;
use crate::env::scope::value::Attrs;
use crate::env::scope::{Environment, array_literal_body, is_valid_identifier};
use oslo_base::error::Result;

/// What the option letters asked for.
#[derive(Default, Clone)]
pub(crate) struct Spec {
    pub on: Attrs,
    pub off: Attrs,
    /// `-a`.
    pub indexed: bool,
    /// `-A`.
    pub assoc: bool,
    /// `+a` or `+A`, which bash refuses on an array.
    pub drop_array: bool,
    pub global: bool,
    pub print: bool,
    /// `-f`: functions, with their bodies.
    pub functions: bool,
    /// `-F`: functions, by name only.
    pub names_only: bool,
}

impl Spec {
    /// Whether any attribute letter was given, which turns a bare listing into a filtered one.
    fn filters(&self) -> bool {
        self.on != Attrs::empty() || self.indexed || self.assoc
    }
}

/// Where a declaration writes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// A binding of the running function's own.
    Local,
    /// The global binding, past any local that hides it.
    Global,
    /// Whatever binding the name resolves to now.
    Visible,
}

/// Parse the leading `-x`/`+x` words. `Err` carries the status of a refused letter.
pub(crate) fn parse_spec<'a>(
    args: &'a [String],
    builtin: &str,
    accepted: &str,
) -> std::result::Result<(Spec, &'a [String]), i32> {
    let mut spec = Spec::default();
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        let plus = arg.starts_with('+');
        if arg.len() < 2 || !(arg.starts_with('-') || plus) {
            break;
        }
        for c in arg[1..].chars() {
            if !accepted.contains(c) {
                let sign = if plus { '+' } else { '-' };
                crate::env::complain(
                    args,
                    arg,
                    &format!("{builtin}: {sign}{c}: invalid option"),
                    "not an option here",
                    Some("the attributes are -a -A -i -l -u -c -n -r -t -x, and -f -F -g -p"),
                );
                return Err(2);
            }
            let bit = match c {
                'x' => Attrs::EXPORT,
                'r' => Attrs::READONLY,
                'i' => Attrs::INTEGER,
                'l' => Attrs::LOWER,
                'u' => Attrs::UPPER,
                'c' => Attrs::CAPITAL,
                't' => Attrs::TRACE,
                'n' => Attrs::NAMEREF,
                _ => Attrs::empty(),
            };
            match (plus, c) {
                (false, 'a') => spec.indexed = true,
                (false, 'A') => spec.assoc = true,
                (true, 'a' | 'A') => spec.drop_array = true,
                (false, 'g') => spec.global = true,
                (false, 'p') => spec.print = true,
                (false, 'f') => spec.functions = true,
                (false, 'F') => spec.names_only = true,
                (false, _) => {
                    if bit.has(Attrs::CASE) {
                        spec.on.remove(Attrs::CASE);
                    }
                    spec.on.insert(bit);
                }
                (true, _) => spec.off.insert(bit),
            }
        }
        i += 1;
    }
    Ok((spec, &args[i.min(args.len())..]))
}

/// `declare [-aAfFgilnprtux] [name[=value] …]`, and `typeset`, its other name.
pub fn builtin_declare(env: &mut Environment, args: &[String]) -> Result<i32> {
    let written = Written::take();
    let builtin = args.first().map_or("declare", String::as_str).to_string();
    let (spec, operands) = match parse_spec(args, &builtin, "aAfFgilnprtuxc") {
        Ok(parsed) => parsed,
        Err(status) => return Ok(status),
    };
    if spec.functions || spec.names_only {
        return Ok(listing::functions(env, operands, &spec, &builtin));
    }
    if operands.is_empty() {
        listing::variables(env, &spec);
        return Ok(0);
    }
    if spec.print {
        return Ok(listing::named(env, operands, &builtin));
    }
    let scope = match (spec.global, env.in_function()) {
        (true, true) => Scope::Global,
        (false, true) => Scope::Local,
        (_, false) => Scope::Visible,
    };
    let mut status = 0;
    for (nth, operand) in operands.iter().enumerate() {
        let compound = written.operand(args, operands, nth);
        if !declare_one(env, &builtin, operand, &spec, scope, compound)? {
            status = 1;
        }
    }
    Ok(status)
}

/// `local [-aAilnprtux] [name[=value] …]`, `local -` and a bare `local`.
pub fn builtin_local(env: &mut Environment, args: &[String]) -> Result<i32> {
    let written = Written::take();
    if !env.in_function() {
        eprintln!(
            "{}local: can only be used in a function",
            crate::env::origin_now()
        );
        return Ok(1);
    }
    let (spec, operands) = match parse_spec(args, "local", "aAilnprtuxc") {
        Ok(parsed) => parsed,
        Err(status) => return Ok(status),
    };
    if operands.is_empty() {
        listing::locals(env, None);
        return Ok(0);
    }
    if spec.print {
        listing::locals(env, Some(operands));
        return Ok(0);
    }
    let mut status = 0;
    for (nth, operand) in operands.iter().enumerate() {
        if operand == "-" {
            env.save_options_locally();
            continue;
        }
        let compound = written.operand(args, operands, nth);
        if !declare_one(env, "local", operand, &spec, Scope::Local, compound)? {
            status = 1;
        }
    }
    Ok(status)
}

/// An operand split into the name it declares and what it assigns.
pub(crate) struct Operand<'a> {
    /// The operand as written, for a diagnostic to point at.
    pub text: &'a str,
    pub name: &'a str,
    pub subscript: Option<&'a str>,
    pub value: Option<&'a str>,
    pub append: bool,
    /// Written `name=(…)` with the parenthesis unquoted; see [`Written`].
    pub compound: bool,
}

/// Split `name[sub]+=value`, or `None` if `text` does not start with a valid name.
pub(crate) fn split_operand(text: &str) -> Option<Operand<'_>> {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    let name = &text[..end];
    let mut rest = &text[end..];
    let mut subscript = None;
    if rest.starts_with('[') {
        let mut depth = 0usize;
        let close = rest.char_indices().find_map(|(i, c)| {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(i)
        })?;
        subscript = Some(&rest[1..close]);
        rest = &rest[close + 1..];
    }
    let (value, append) = if rest.is_empty() {
        (None, false)
    } else if let Some(value) = rest.strip_prefix("+=") {
        (Some(value), true)
    } else {
        (Some(rest.strip_prefix('=')?), false)
    };
    (is_valid_identifier(name) && subscript != Some("")).then_some(Operand {
        text,
        name,
        subscript,
        value,
        append,
        compound: true,
    })
}

/// Say that `operand` cannot be declared, the way bash words each refusal: with the builtin's
/// name for `declare` and `local`, without it for the special builtins `export` and `readonly`.
fn refuse(builtin: &str, operand: &str, what: &str, reason: &str) {
    let body = match builtin {
        "export" | "readonly" => format!("{what}: {reason}"),
        _ => format!("{builtin}: {what}: {reason}"),
    };
    let words = crate::env::line(builtin, std::slice::from_ref(&operand.to_string()));
    crate::env::complain(&words, operand, &body, "refused", None);
}

/// Declare one operand. `Ok(false)` if it was refused, with the reason already printed.
///
/// `compound` is whether the operand was written `name=(…)`; one that was not makes an array
/// only of a name that is one already.
pub(crate) fn declare_one(
    env: &mut Environment,
    builtin: &str,
    operand: &str,
    spec: &Spec,
    scope: Scope,
    compound: bool,
) -> Result<bool> {
    let Some(mut op) = split_operand(operand) else {
        let shown = format!("`{}'", oslo_base::shown::shown(operand));
        refuse(builtin, operand, &shown, "not a valid identifier");
        return Ok(false);
    };
    op.compound = compound;
    match scope {
        Scope::Global => {
            let name = op.name.to_string();
            env.with_global_binding(&name, |env| {
                declare_here(env, builtin, &op, spec, Scope::Visible)
            })
        }
        _ => declare_here(env, builtin, &op, spec, scope),
    }
}

fn declare_here(
    env: &mut Environment,
    builtin: &str,
    op: &Operand<'_>,
    spec: &Spec,
    scope: Scope,
) -> Result<bool> {
    let nameref_op = spec.on.has(Attrs::NAMEREF) || spec.off.has(Attrs::NAMEREF);
    if spec.on.has(Attrs::NAMEREF) {
        if op.subscript.is_some() {
            refuse(
                builtin,
                op.text,
                op.name,
                "reference variable cannot be an array",
            );
            return Ok(false);
        }
        if let Some(value) = op.value {
            if !is_reference_name(value) {
                refuse(
                    builtin,
                    op.text,
                    &format!("`{value}'"),
                    "invalid variable name for name reference",
                );
                return Ok(false);
            }
            if value == op.name {
                refuse(
                    builtin,
                    op.text,
                    op.name,
                    "nameref variable self references not allowed",
                );
                return Ok(false);
            }
        }
    }
    if scope == Scope::Local && !env.make_local(op.name) {
        refuse(builtin, op.text, op.name, "readonly variable");
        return Ok(false);
    }
    let target = match nameref_op {
        true => op.name.to_string(),
        false => env.ref_target_name(op.name).into_owned(),
    };
    if !compatible(env, builtin, &target, op, spec) {
        return Ok(false);
    }

    let mut on = spec.on;
    on.remove(Attrs::READONLY);
    env.modify_binding_attrs(&target, on, spec.off);
    if spec.assoc {
        env.declare_assoc(&target);
    } else if spec.indexed || (op.subscript.is_some() && !env.is_assoc(&target)) {
        env.declare_array(&target);
    }

    let assigned = match (op.subscript, op.value) {
        (Some(subscript), Some(value)) => {
            assign_element(env, &target, subscript, value, op.append)?
        }
        (None, Some(value)) => assign_value(env, &target, op, value, spec)?,
        (_, None) => true,
    };
    if !assigned {
        return Ok(false);
    }
    if spec.on.has(Attrs::READONLY) {
        env.modify_binding_attrs(&target, Attrs::READONLY, Attrs::empty());
    }
    Ok(true)
}

/// The checks bash makes before it changes anything: a read-only name, and an array of one kind
/// asked to become the other.
fn compatible(
    env: &Environment,
    builtin: &str,
    target: &str,
    op: &Operand<'_>,
    spec: &Spec,
) -> bool {
    let Some(var) = env.variable(target) else {
        return true;
    };
    let reason = if var.readonly() && (op.value.is_some() || spec.off.has(Attrs::READONLY)) {
        "readonly variable"
    } else if spec.assoc && var.indexed().is_some() {
        "cannot convert indexed to associative array"
    } else if spec.indexed && var.assoc().is_some() {
        "cannot convert associative to indexed array"
    } else if spec.drop_array && (var.indexed().is_some() || var.assoc().is_some()) {
        "cannot destroy array variables in this way"
    } else {
        return true;
    };
    refuse(builtin, op.text, target, reason);
    false
}

/// `declare 'a[1]=x'`.
fn assign_element(
    env: &mut Environment,
    name: &str,
    subscript: &str,
    value: &str,
    append: bool,
) -> Result<bool> {
    let text = subscript.to_string();
    let key = match arrays::key_for(env, name, &text) {
        Ok(key) => key,
        Err(e) => {
            eprintln!("{}{e}", crate::env::origin_now());
            return Ok(false);
        }
    };
    match arrays::write_element(env, name, key, value, append) {
        Ok(done) => Ok(done),
        Err(e) => {
            eprintln!("{}{e}", crate::env::origin_now());
            Ok(false)
        }
    }
}

/// `declare name=value` once the attributes are in place.
fn assign_value(
    env: &mut Environment,
    name: &str,
    op: &Operand<'_>,
    value: &str,
    spec: &Spec,
) -> Result<bool> {
    let append = op.append;
    if spec.on.has(Attrs::NAMEREF) {
        return Ok(env.set_reference(name, value));
    }
    // A literal that names no element abandons the line, as the same literal written as an
    // assignment does. A quoted one is a literal only for a name that is an array already.
    let array = op.compound || env.get_array(name).is_some() || env.is_assoc(name);
    if let Some(body) = array_literal_body(value).filter(|_| array) {
        let items = literal_items(env, body)?;
        return match assign_literal(env, name, items, append) {
            Ok(done) => Ok(done),
            Err(e) => {
                eprintln!("{}{e}", crate::env::origin_now());
                Err(oslo_base::error::ShellError::Abandoned)
            }
        };
    }
    if append {
        return Ok(arrays::append_scalar(env, name, value));
    }
    Ok(env.set_var(name, value, false))
}
