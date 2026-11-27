//! The listing forms of `declare` and `local`: everything, one kind, some names, or functions.

use super::Spec;
use super::print::{declaration, function_flags, print_matching, set_line};
use crate::env::Environment;
use crate::env::scope::value::Attrs;

/// `declare` with no names: every variable as `name=value` and every function, or with `-p` or
/// an attribute letter, every matching variable as a `declare` line.
pub(super) fn variables(env: &Environment, spec: &Spec) {
    if !spec.print && !spec.filters() {
        for (name, var) in env.bindings() {
            if let Some(line) = set_line(name, var).filter(|_| is_listable(name)) {
                println!("{line}");
            }
        }
        let mut names: Vec<&String> = env.get_functions().keys().collect();
        names.sort();
        for name in names {
            if let Some(definition) = env.function_definition(name) {
                println!("{definition}");
            }
        }
        return;
    }
    print_matching(env, None, |_, var| {
        let attrs = spec.on;
        let kind =
            (!spec.indexed || var.indexed().is_some()) && (!spec.assoc || var.assoc().is_some());
        kind && [
            Attrs::EXPORT,
            Attrs::READONLY,
            Attrs::INTEGER,
            Attrs::LOWER,
            Attrs::UPPER,
            Attrs::CAPITAL,
            Attrs::TRACE,
            Attrs::NAMEREF,
        ]
        .iter()
        .all(|bit| !attrs.has(*bit) || var.attrs.has(*bit))
    });
}

fn is_listable(name: &str) -> bool {
    crate::env::scope::is_valid_identifier(name)
}

/// `declare -p name…`: each name's declaration, without following a nameref.
pub(super) fn named(env: &Environment, names: &[String], builtin: &str) -> i32 {
    let mut status = 0;
    for operand in names {
        let name = operand.split_once('=').map_or(operand.as_str(), |(n, _)| n);
        match env.variable(name) {
            Some(var) => println!("{}", declaration(name, var)),
            None if env.special_value(name).is_some() => {
                println!("{}", super::print::special_declaration(env, name));
            }
            None => {
                crate::env::complain(
                    &crate::env::line(builtin, names),
                    operand,
                    &format!("{builtin}: {}: not found", oslo_base::shown::shown(name)),
                    "no variable of that name",
                    Some("`declare -p` on its own lists every variable there is"),
                );
                status = 1;
            }
        }
    }
    status
}

/// `local` and `local -p`: the running function's locals, or only the named ones.
pub(super) fn locals(env: &Environment, only: Option<&[String]>) {
    for (name, var) in env.local_bindings() {
        if only.is_none_or(|names| names.iter().any(|n| n == name)) {
            println!("{}", declaration(name, var));
        }
    }
}

/// `declare -f` and `declare -F`, listing functions or giving them attributes.
///
/// Asking about names prints only those: `-F name` is the bare name, because the caller already
/// knows it and is asking whether it exists. `-f` prints whole definitions; `-F` wins over `-f`.
pub(super) fn functions(
    env: &mut Environment,
    names: &[String],
    spec: &Spec,
    builtin: &str,
) -> i32 {
    let setting = spec.on.has(Attrs::EXPORT) || spec.on.has(Attrs::READONLY);
    if setting && !names.is_empty() {
        return set_function_attrs(env, names, spec, builtin);
    }
    let asked = !names.is_empty();
    let selected: Vec<String> = match asked {
        true => names.to_vec(),
        false => {
            let mut all: Vec<String> = env.get_functions().keys().cloned().collect();
            all.sort();
            all
        }
    };
    let mut status = 0;
    for name in selected {
        let Some(definition) = env.function_definition(&name) else {
            status = 1;
            continue;
        };
        if !asked && !function_matches(env, &name, spec) {
            continue;
        }
        match (spec.names_only, asked) {
            (true, true) => println!("{name}"),
            (true, false) => println!("declare {} {name}", function_flags(env, &name)),
            (false, _) => {
                println!("{definition}");
                // A listing, or `-p`, follows the body with the attributes beyond `-f`.
                let flags = function_flags(env, &name);
                if (!asked || spec.print) && flags != "-f" {
                    println!("declare {flags} {name}");
                }
            }
        }
    }
    status
}

/// Whether a function carries every attribute the listing asked for.
fn function_matches(env: &Environment, name: &str, spec: &Spec) -> bool {
    (!spec.on.has(Attrs::EXPORT) || env.is_function_exported(name))
        && (!spec.on.has(Attrs::READONLY) || env.is_function_readonly(name))
}

fn set_function_attrs(env: &mut Environment, names: &[String], spec: &Spec, builtin: &str) -> i32 {
    let mut status = 0;
    for name in names {
        if env.get_function(name).is_none() {
            crate::env::complain(
                &crate::env::line(builtin, names),
                name,
                &format!("{builtin}: {name}: not found"),
                "no function of that name",
                Some("`declare -F` lists the functions there are"),
            );
            status = 1;
            continue;
        }
        if spec.on.has(Attrs::EXPORT) {
            env.export_function(name);
        }
        if spec.on.has(Attrs::READONLY) {
            env.set_function_readonly(name);
        }
    }
    status
}
