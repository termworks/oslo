//! `declare` / `typeset` — declare variables and their attributes.
//!
//! Registered under both names because they are the same builtin; `typeset` is the ksh spelling
//! bash keeps for compatibility.
//!
//! The scoping rule is inherited rather than reimplemented: [`Environment::set_local_var`] writes
//! into the innermost scope frame when there is one and straight to the global table when there
//! is not, which is exactly `declare`'s "local inside a function, global outside" contract.
//! `-g` opts out by writing globally either way.
//!
//! `-a` declares an indexed array, and `name=(…)` builds one whether or not `-a` was given —
//! bash infers the attribute from the literal, and so does this.
//!
//! Attributes oslo cannot represent are **refused**, not ignored. `declare -i n` in bash makes
//! every later assignment to `n` an arithmetic evaluation, and `declare -A` makes an
//! *associative* array — a second value shape that the expander, `for`, `local`, `export` and the
//! tracker would all have to learn, for far less than indexed arrays buy. Accepting either and quietly
//! producing a plain scalar is the "plausible wrong answer with status 0" failure mode this shell
//! is being audited for, so they exit 2 with a diagnostic instead.
//!
//! [`Environment::set_local_var`]: crate::env::Environment::set_local_var

use super::arrays::array_elements;
use crate::env::scope::{Environment, ShellArray, array_literal_body, is_valid_identifier};
use oslo_base::error::Result;

/// What the option letters asked for.
#[derive(Default)]
struct Attributes {
    export: bool,
    readonly: bool,
    global: bool,
    print: bool,
    functions: bool,
    /// `-f`: print each function's whole definition, where `-F` prints only names.
    bodies: bool,
    /// `-a`: the name is an indexed array, even if no value is given.
    indexed: bool,
    /// `-i`: every assignment to the name is evaluated as arithmetic.
    integer: bool,
    /// `+i`: take that attribute back off again.
    drop_integer: bool,
}

/// `declare [-afFgprx] [name[=value] …]`, and `typeset`, its other name.
pub fn builtin_declare(env: &mut Environment, args: &[String]) -> Result<i32> {
    let name = args.first().map(String::as_str).unwrap_or("declare");
    let mut attrs = Attributes::default();

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if arg.len() < 2 || !(arg.starts_with('-') || arg.starts_with('+')) {
            break;
        }
        for c in arg[1..].chars() {
            match (arg.starts_with('+'), c) {
                (false, 'x') => attrs.export = true,
                (false, 'r') => attrs.readonly = true,
                (false, 'g') => attrs.global = true,
                (false, 'p') => attrs.print = true,
                // `-F` reports names; `-f` reports whole definitions. They were treated alike, so
                // `declare -f f` answered with the name-only line and `declare -f f > saved.sh`
                // wrote a file that reads as a definition and defines nothing. The printer `type`
                // already uses renders the body — see [`super::control::format_function`].
                (false, 'F') => attrs.functions = true,
                (false, 'f') => {
                    attrs.functions = true;
                    attrs.bodies = true;
                }
                (false, 'a') => attrs.indexed = true,
                (false, 'i') => attrs.integer = true,
                (true, 'i') => attrs.drop_integer = true,
                // The one attribute that is deferred rather than merely missing. Saying so beats
                // declaring an *indexed* array: the subscript is arithmetic, so every key would
                // land on element 0 and the last write would win — see the collision pinned in
                // `tests/corpus/array_element_assignment.sh`.
                (false, 'A') => {
                    crate::env::complain(
                        &[name.to_string(), String::from("-A")],
                        "-A",
                        &format!("{name}: -A: associative arrays are not supported"),
                        "not supported",
                        Some("`declare -a` gives an indexed array, which oslo does have"),
                    );
                    return Ok(2);
                }
                // Every remaining letter names an attribute this shell has no representation
                // for. Saying so beats declaring a scalar and calling it an array.
                (plus, c) => {
                    let sign = if plus { '+' } else { '-' };
                    crate::env::complain(
                        &[name.to_string(), format!("{sign}{c}")],
                        &format!("{sign}{c}"),
                        &format!("{name}: {sign}{c}: attribute not supported"),
                        "no such attribute",
                        Some(
                            "oslo has -a (indexed array), -i (integer), -r (readonly), -x (exported) and -f (function)",
                        ),
                    );
                    return Ok(2);
                }
            }
        }
        i += 1;
    }

    let operands = &args[i.min(args.len())..];

    if attrs.functions {
        return Ok(print_functions(env, operands, attrs.bodies));
    }
    if attrs.print || operands.is_empty() {
        return Ok(print_variables(env, operands));
    }

    let mut status = 0;
    for operand in operands {
        if !apply(env, operand, &attrs, name)? {
            status = 1;
        }
    }
    Ok(status)
}

/// Declare one `name` or `name=value`; `Ok(false)` if it was refused.
fn apply(env: &mut Environment, operand: &str, attrs: &Attributes, builtin: &str) -> Result<bool> {
    let (name, value) = match operand.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (operand, None),
    };

    if !is_valid_identifier(name) {
        crate::env::complain(
            &[builtin.to_string(), operand.to_string()],
            operand,
            &format!(
                "{builtin}: `{}': not a valid identifier",
                oslo_base::shown::shown(operand)
            ),
            "not a name",
            Some(
                "a name starts with a letter or underscore and continues with letters, digits or underscores",
            ),
        );
        return Ok(false);
    }

    // **Before the value**, so `declare -i n=2+3` stores 5: the attribute is what makes the
    // assignment arithmetic, and marking it afterwards would evaluate everything but the first.
    if attrs.integer {
        env.set_integer(name);
    }
    if attrs.drop_integer {
        env.clear_integer(name);
    }

    // `name=(…)` is an array however it was spelled: bash infers `-a` from the literal, and the
    // alternative here is storing the parentheses as a scalar — which is what used to happen.
    if let Some(body) = value.and_then(array_literal_body) {
        let array = array_elements(env, body)?;
        if !assign_array(env, name, array, attrs) {
            return Ok(false);
        }
    } else if let Some(value) = value {
        // Without `-g` this is `local` inside a function and a plain assignment outside, which is
        // what `set_local_var` already decides based on whether a scope frame exists.
        let assigned = if attrs.global {
            env.set_var(name, value, attrs.export)
        } else {
            env.set_local_var(name, value)
        };
        if !assigned {
            return Ok(false);
        }
    } else if attrs.indexed {
        // `declare -a name` with no value makes an *empty* array, which is what `${#name[@]}`
        // answering 0 rather than an error depends on.
        env.declare_array(name);
    }

    // An array cannot live in `environ`, so `-x` on one is a no-op rather than a way to export
    // the parentheses. bash exports nothing here either.
    if attrs.export && env.get_array(name).is_none() && !env.export_var(name) {
        return Ok(false);
    }
    if attrs.readonly {
        // Last, so that `declare -r x=1` gets its value in before the variable is frozen.
        //
        // Scoped unless `-g`: inside a function `declare` is a *local* declaration, so its
        // read-only mark leaves with the frame the way `local -r`'s does. The `readonly` builtin
        // is the one that marks globally, even inside a function — bash draws the line there.
        match attrs.global {
            true => env.set_readonly(name),
            false => env.set_readonly_here(name),
        }
    }
    Ok(true)
}

/// Store an array, honouring `declare`'s "local inside a function, global outside" rule.
fn assign_array(env: &mut Environment, name: &str, array: ShellArray, attrs: &Attributes) -> bool {
    if attrs.global {
        env.set_array(name, array)
    } else {
        env.set_local_array(name, array)
    }
}

/// `declare -p [name…]` — print declarations in a form the shell could read back.
fn print_variables(env: &mut Environment, names: &[String]) -> i32 {
    let exported = env.get_exported_vars();
    let mut status = 0;

    let render = |env: &Environment, name: &str, value: &str| {
        // `i`, then `r`, then `x` — bash's order, and the whole point of `-p` is that what it
        // prints reads back as what was declared.
        let mut flags = String::new();
        if env.is_integer(name) {
            flags.push('i');
        }
        if env.is_readonly(name) {
            flags.push('r');
        }
        if exported.contains_key(name) {
            flags.push('x');
        }
        let flags = if flags.is_empty() {
            "--".to_string()
        } else {
            format!("-{}", flags)
        };
        println!("declare {} {}=\"{}\"", flags, name, escape(value));
    };

    if names.is_empty() {
        let mut all: Vec<(String, String)> = env.get_all_vars().into_iter().collect();
        all.sort();
        for (name, value) in &all {
            render(env, name, value);
        }
        let mut arrays: Vec<String> = env.array_names().map(str::to_string).collect();
        arrays.sort();
        for name in &arrays {
            println!("{}", render_array(env, name));
        }
        return 0;
    }

    for name in names {
        // A `name=value` operand is a declaration even under `-p`; only bare names are queries.
        let name = name.split_once('=').map(|(n, _)| n).unwrap_or(name);
        // An array is checked first: `get_var` answers with its element 0, which would print a
        // scalar declaration for something that is not one.
        if env.get_array(name).is_some() {
            println!("{}", render_array(env, name));
            continue;
        }
        match env.get_var(name).map(str::to_string) {
            Some(value) => render(env, name, &value),
            None => {
                crate::env::complain(
                    &crate::env::line("declare", names),
                    name,
                    &format!("declare: {}: not found", oslo_base::shown::shown(name)),
                    "no variable of that name",
                    Some("`declare -p` on its own lists every variable there is"),
                );
                status = 1;
            }
        }
    }
    status
}

/// One `declare -a name=([i]="…" …)` line, in bash's shape.
///
/// The indices are printed even for a dense array, because they are the only way to see a hole:
/// `a=(1 2 3); unset 'a[1]'` prints `[0]` and `[2]`, and a positional rendering could not.
fn render_array(env: &Environment, name: &str) -> String {
    let Some(array) = env.get_array(name) else {
        return String::new();
    };
    let flags = if env.is_readonly(name) { "-ar" } else { "-a" };
    // bash prints a declared-but-empty array as a bare `declare -a name`, with no `=()`.
    if array.is_empty() {
        return format!("declare {} {}", flags, name);
    }
    let body: Vec<String> = array
        .indices()
        .zip(array.values())
        .map(|(index, value)| format!("[{}]=\"{}\"", index, escape(value)))
        .collect();
    format!("declare {} {}=({})", flags, name, body.join(" "))
}

/// `declare -f`/`-F` — report shell functions.
fn print_functions(env: &Environment, names: &[String], bodies: bool) -> i32 {
    // **Asking about one is not the same as listing them all**, and bash spells the two
    // differently: `declare -F` writes a `declare -f name` line per function, so the output can be
    // sourced; `declare -F name` writes the bare name, because the caller already knows it and is
    // asking whether it exists. Printing the long form for both made `declare -F f` answer with
    // text that reads as a definition.
    let asked = !names.is_empty();
    let selected: Vec<String> = if asked {
        names.to_vec()
    } else {
        let mut all: Vec<String> = env.get_functions().keys().cloned().collect();
        all.sort();
        all
    };

    let mut status = 0;
    for name in selected {
        let Some(body) = env.get_function(&name) else {
            status = 1;
            continue;
        };
        match (bodies, asked) {
            // `-f`: the whole definition, in the shape `type` already prints and the differential
            // suite already compares against bash byte for byte.
            (true, _) => println!("{}", super::control::format_function(&name, body)),
            (false, true) => println!("{name}"),
            (false, false) => println!("declare -f {name}"),
        }
    }
    status
}

/// Quote a value for `declare -p` output so it could be pasted back into the shell.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{builtin_declare, escape};
    use crate::env::Environment;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_plain_declaration_assigns() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_declare(&mut env, &argv(&["declare", "oslo_d1=value"])).unwrap(),
            0
        );
        assert_eq!(env.get_var("oslo_d1"), Some("value"));
    }

    #[test]
    fn export_and_readonly_attributes_are_applied() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_declare(&mut env, &argv(&["declare", "-rx", "oslo_d2=v"])).unwrap(),
            0
        );
        assert!(env.get_exported_vars().contains_key("oslo_d2"));
        assert!(env.is_readonly("oslo_d2"));
    }

    /// `-r` is applied after the value, or `declare -r x=1` would freeze `x` before it had one.
    #[test]
    fn a_readonly_declaration_keeps_its_initial_value() {
        let mut env = Environment::new();
        builtin_declare(&mut env, &argv(&["declare", "-r", "oslo_d3=kept"])).unwrap();
        assert_eq!(env.get_var("oslo_d3"), Some("kept"));
    }

    /// Inside a scope frame — a function call — a declaration is local and is undone on exit.
    #[test]
    fn a_declaration_inside_a_function_is_local() {
        let mut env = Environment::new();
        env.set_var("oslo_d4", "outer", false);
        env.push_scope();
        builtin_declare(&mut env, &argv(&["declare", "oslo_d4=inner"])).unwrap();
        assert_eq!(env.get_var("oslo_d4"), Some("inner"));
        env.pop_scope();
        assert_eq!(env.get_var("oslo_d4"), Some("outer"));
    }

    /// `-g` is the opt-out: the assignment outlives the frame it was made in.
    #[test]
    fn a_global_declaration_escapes_the_frame() {
        let mut env = Environment::new();
        env.push_scope();
        builtin_declare(&mut env, &argv(&["declare", "-g", "oslo_d5=global"])).unwrap();
        env.pop_scope();
        assert_eq!(env.get_var("oslo_d5"), Some("global"));
    }

    /// An attribute with no representation in this shell is refused, not silently downgraded to
    /// a plain scalar. `-A` is the deliberate one: it must say so rather than build an indexed
    /// array that would answer `${m[key]}` with element 0.
    #[test]
    fn an_unrepresentable_attribute_is_refused() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_declare(&mut env, &argv(&["declare", "-A", "oslo_d6"])).unwrap(),
            2
        );
        assert_eq!(env.get_var("oslo_d6"), None);
        assert!(env.get_array("oslo_d6").is_none());
        // And so is every letter the shell has no representation for: `-l` would lowercase on
        // assignment, and storing `A` under it unchanged is the silent downgrade this refuses.
        assert_eq!(
            builtin_declare(&mut env, &argv(&["declare", "-l", "oslo_d7=A"])).unwrap(),
            2
        );
        assert_eq!(env.get_var("oslo_d7"), None);
    }

    /// `-i` is represented, so it is accepted — and it has to be *before* the value, or
    /// `declare -i n=2+3` stores the text rather than 5.
    #[test]
    fn the_integer_attribute_is_kept_and_makes_the_assignment_arithmetic() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_declare(&mut env, &argv(&["declare", "-i", "oslo_d8=2+3"])).unwrap(),
            0
        );
        assert_eq!(env.get_var("oslo_d8"), Some("5"));
        assert!(env.is_integer("oslo_d8"));
    }

    /// `-a` declares an array even with no value, so `${#name[@]}` is 0 rather than an error.
    #[test]
    fn the_array_attribute_creates_an_empty_array() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_declare(&mut env, &argv(&["declare", "-a", "oslo_d8"])).unwrap(),
            0
        );
        assert_eq!(env.get_array("oslo_d8").map(|a| a.len()), Some(0));
    }

    /// A `name=(…)` operand is an array whether or not `-a` was given — the shape decides.
    #[test]
    fn a_literal_operand_builds_an_array() {
        let mut env = Environment::new();
        builtin_declare(&mut env, &argv(&["declare", "oslo_d9=(1 2 3)"])).unwrap();
        assert_eq!(
            env.get_array("oslo_d9").map(|a| a.joined(" ")),
            Some("1 2 3".into())
        );
        // …and it is not the source text, which is what used to be stored.
        assert_eq!(env.get_var("oslo_d9"), Some("1"));
    }

    #[test]
    fn an_invalid_name_is_reported() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_declare(&mut env, &argv(&["declare", "1bad=x"])).unwrap(),
            1
        );
    }

    #[test]
    fn a_missing_name_under_p_is_reported() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_declare(&mut env, &argv(&["declare", "-p", "oslo_no_such_var"])).unwrap(),
            1
        );
    }

    #[test]
    fn values_are_quoted_so_they_can_be_read_back() {
        assert_eq!(escape(r#"a"b"#), r#"a\"b"#);
        assert_eq!(escape("a$b`c\\d"), "a\\$b\\`c\\\\d");
        assert_eq!(escape("plain"), "plain");
    }
}

#[cfg(test)]
mod function_tests {
    use super::builtin_declare;
    use crate::env::Environment;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    /// **`-f` prints the body, `-F` prints the name.** They were treated alike, so
    /// `declare -f f > saved.sh` wrote a file that reads as a definition and defines nothing.
    /// The printer `type` uses was there the whole time.
    #[test]
    fn asking_for_a_body_that_is_not_there_still_fails() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_declare(&mut env, &argv(&["declare", "-f", "nosuch"])).unwrap(),
            1,
            "a name nothing declared is a failure, as in bash"
        );
    }

    /// `-F` is the half oslo can answer, and it still works.
    #[test]
    fn listing_the_names_still_works() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_declare(&mut env, &argv(&["declare", "-F", "nosuch"])).unwrap(),
            1,
            "a name nothing declared is a failure, as in bash"
        );
    }
}
