//! What `declare -p`, `local`, `export -p`, `readonly -p` and a bare `declare` print.
//!
//! Every form is bash's byte for byte, so a listing reads back as what was declared and a script
//! that parses one — there are many — finds what it expects. `utf8` is whether the locale shows
//! multibyte characters as themselves; see `quote`.

use super::quote;
use crate::env::Environment;
use crate::env::scope::value::{Value, Var};

/// Whether the locale `env` is in shows UTF-8 text as itself.
pub fn utf8(env: &Environment) -> bool {
    crate::expand::locale::is_utf8(env)
}

/// The `-FLAGS` word of a `declare` line, `--` when there are none.
fn flags_word(letters: &str) -> String {
    match letters.is_empty() {
        true => "--".to_string(),
        false => format!("-{letters}"),
    }
}

/// One `declare -FLAGS name[=value]` line.
pub fn declaration(name: &str, var: &Var, utf8: bool) -> String {
    let flags = flags_word(&var.flag_letters());
    match assignment(var, utf8) {
        Some(value) => format!("declare {flags} {name}={value}"),
        None => format!("declare {flags} {name}"),
    }
}

/// The `declare` line of a special variable the shell computes rather than stores.
pub fn special_declaration(env: &Environment, name: &str) -> String {
    let mut letters = String::new();
    for (on, letter) in [
        (env.is_integer(name), 'i'),
        (env.is_readonly(name), 'r'),
        (env.is_exported(name), 'x'),
    ] {
        if on {
            letters.push(letter);
        }
    }
    let value = env.special_value(name).unwrap_or_default();
    format!(
        "declare {} {name}={}",
        flags_word(&letters),
        quote::value(&value, utf8(env))
    )
}

/// The right-hand side `declare -p` writes, or `None` for a binding with no value.
pub fn assignment(var: &Var, utf8: bool) -> Option<String> {
    if !var.is_set() {
        return None;
    }
    Some(match &var.value {
        Value::Scalar(text) => quote::value(text, utf8),
        Value::Indexed(array) => {
            let elements: Vec<String> = array
                .iter()
                .map(|(i, v)| format!("[{i}]={}", quote::value(v, utf8)))
                .collect();
            format!("({})", elements.join(" "))
        }
        Value::Assoc(map) => {
            let mut out = String::from("(");
            for (key, value) in map.iter() {
                let (key, value) = (quote::key(key, utf8), quote::value(value, utf8));
                out.push_str(&format!("[{key}]={value} "));
            }
            out.push(')');
            out
        }
    })
}

/// `name=value` as a bare `declare` and `set` list a binding, or `None` for one with no value.
pub fn set_line(name: &str, var: &Var, utf8: bool) -> Option<String> {
    if !var.is_set() {
        return None;
    }
    match &var.value {
        Value::Scalar(text) => Some(format!("{name}={}", quote::set_style(text, utf8))),
        _ => assignment(var, utf8).map(|value| format!("{name}={value}")),
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
pub fn posix_declaration(builtin: &str, name: &str, var: &Var, utf8: bool) -> String {
    let flags = match var.value {
        Value::Indexed(_) => "-a ",
        Value::Assoc(_) => "-A ",
        Value::Scalar(_) => "",
    };
    match assignment(var, utf8) {
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
    let utf8 = utf8(env);
    for (name, var) in env.listing() {
        if crate::env::scope::is_valid_identifier(&name) && keep(&name, &var) {
            match posix_as {
                Some(builtin) => println!("{}", posix_declaration(builtin, &name, &var, utf8)),
                None => println!("{}", declaration(&name, &var, utf8)),
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
        assert_eq!(declaration("x", &scalar, true), r#"declare -x x="1""#);
        assert_eq!(
            declaration("y", &Var::unset(Attrs::empty()), true),
            "declare -- y"
        );
        let mut array = Var::scalar("", Attrs::empty());
        array.value = Value::Indexed(ShellArray::default());
        assert_eq!(declaration("a", &array, true), "declare -a a=()");
        let mut map = AssocArray::default();
        map.set("b", "2");
        map.set("a", "1");
        let assoc = Var {
            value: Value::Assoc(map),
            attrs: Attrs::empty(),
        };
        assert_eq!(
            declaration("m", &assoc, true),
            r#"declare -A m=([b]="2" [a]="1" )"#
        );
    }
}
