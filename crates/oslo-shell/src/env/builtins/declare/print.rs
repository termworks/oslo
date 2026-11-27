//! What `declare -p`, `local`, `export -p`, `readonly -p` and a bare `declare` print.
//!
//! Every form is bash's byte for byte, so a listing reads back as what was declared and a script
//! that parses one — there are many — finds what it expects.

use super::quote;
use crate::env::Environment;
use crate::env::scope::value::{Value, Var};

/// One `declare -FLAGS name[=value]` line.
pub fn declaration(name: &str, var: &Var) -> String {
    let flags = var.flag_letters();
    let flags = match flags.is_empty() {
        true => "--".to_string(),
        false => format!("-{flags}"),
    };
    match assignment(var) {
        Some(value) => format!("declare {flags} {name}={value}"),
        None => format!("declare {flags} {name}"),
    }
}

/// The `declare` line of a special variable the shell computes rather than stores.
pub fn special_declaration(env: &Environment, name: &str) -> String {
    let mut flags = String::new();
    for (on, letter) in [
        (env.is_integer(name), 'i'),
        (env.is_readonly(name), 'r'),
        (env.is_exported(name), 'x'),
    ] {
        if on {
            flags.push(letter);
        }
    }
    let flags = match flags.is_empty() {
        true => "--".to_string(),
        false => format!("-{flags}"),
    };
    let value = env.special_value(name).unwrap_or_default();
    format!("declare {flags} {name}={}", quote::value(&value))
}

/// The right-hand side `declare -p` writes, or `None` for a binding with no value.
fn assignment(var: &Var) -> Option<String> {
    if !var.is_set() {
        return None;
    }
    Some(match &var.value {
        Value::Scalar(text) => quote::value(text),
        Value::Indexed(array) => {
            let elements: Vec<String> = array
                .iter()
                .map(|(i, v)| format!("[{i}]={}", quote::value(v)))
                .collect();
            format!("({})", elements.join(" "))
        }
        Value::Assoc(map) => {
            let mut out = String::from("(");
            for (key, value) in map.iter() {
                out.push_str(&format!("[{}]={} ", quote::key(key), quote::value(value)));
            }
            out.push(')');
            out
        }
    })
}

/// `name=value` as a bare `declare` and `set` list a binding, or `None` for one with no value.
pub fn set_line(name: &str, var: &Var) -> Option<String> {
    if !var.is_set() {
        return None;
    }
    match &var.value {
        Value::Scalar(text) => Some(format!("{name}={}", quote::set_style(text))),
        _ => assignment(var).map(|value| format!("{name}={value}")),
    }
}

/// The `-f…` letters of a function's line under `declare -F`.
pub fn function_flags(env: &Environment, name: &str) -> String {
    let mut flags = String::from("-f");
    if env.is_function_readonly(name) {
        flags.push('r');
    }
    if env.is_function_exported(name) {
        flags.push('x');
    }
    flags
}

/// One line of `export -p` or `readonly -p` in POSIX mode: the builtin's own name, and of the
/// attributes only the array letters.
pub fn posix_declaration(builtin: &str, name: &str, var: &Var) -> String {
    let flags = match var.value {
        Value::Indexed(_) => "-a ",
        Value::Assoc(_) => "-A ",
        Value::Scalar(_) => "",
    };
    match assignment(var) {
        Some(value) => format!("{builtin} {flags}{name}={value}"),
        None => format!("{builtin} {flags}{name}"),
    }
}

/// Print every binding `keep` selects, sorted, as `declare` lines — or, for `export` and
/// `readonly` in POSIX mode, as lines in that builtin's own name.
pub fn print_matching(
    env: &Environment,
    posix_as: Option<&str>,
    keep: impl Fn(&str, &Var) -> bool,
) {
    let posix_as = posix_as.filter(|_| env.posix());
    for (name, var) in env.bindings() {
        if crate::env::scope::is_valid_identifier(name) && keep(name, var) {
            match posix_as {
                Some(builtin) => println!("{}", posix_declaration(builtin, name, var)),
                None => println!("{}", declaration(name, var)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::declaration;
    use crate::env::scope::value::{Attrs, Value, Var};
    use crate::env::scope::{AssocArray, ShellArray};

    #[test]
    fn every_shape_prints_as_bash_prints_it() {
        let scalar = Var::scalar("1", Attrs::EXPORT);
        assert_eq!(declaration("x", &scalar), r#"declare -x x="1""#);
        assert_eq!(
            declaration("y", &Var::unset(Attrs::empty())),
            "declare -- y"
        );
        let mut array = Var::scalar("", Attrs::empty());
        array.value = Value::Indexed(ShellArray::default());
        assert_eq!(declaration("a", &array), "declare -a a=()");
        let mut map = AssocArray::default();
        map.set("b", "2");
        map.set("a", "1");
        let assoc = Var {
            value: Value::Assoc(map),
            attrs: Attrs::empty(),
        };
        assert_eq!(
            declaration("m", &assoc),
            r#"declare -A m=([b]="2" [a]="1" )"#
        );
    }
}
