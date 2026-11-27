use super::{builtin_declare, builtin_local, split_operand};
use crate::env::Environment;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

fn declare(env: &mut Environment, words: &[&str]) -> i32 {
    let mut args = vec!["declare"];
    args.extend_from_slice(words);
    builtin_declare(env, &argv(&args)).unwrap()
}

#[test]
fn a_plain_declaration_assigns() {
    let mut env = Environment::new();
    assert_eq!(declare(&mut env, &["oslo_d1=value"]), 0);
    assert_eq!(env.get_var("oslo_d1"), Some("value"));
}

#[test]
fn export_and_readonly_attributes_are_applied() {
    let mut env = Environment::new();
    assert_eq!(declare(&mut env, &["-rx", "oslo_d2=v"]), 0);
    assert!(env.get_exported_vars().contains_key("oslo_d2"));
    assert!(env.is_readonly("oslo_d2"));
    assert_eq!(env.get_var("oslo_d2"), Some("v"));
}

/// Inside a function a declaration is local and is undone on exit; `-g` escapes the frame.
#[test]
fn a_declaration_inside_a_function_is_local_unless_global() {
    let mut env = Environment::new();
    env.set_var("oslo_d4", "outer", false);
    env.enter_function().unwrap();
    env.push_scope();
    declare(&mut env, &["oslo_d4=inner"]);
    declare(&mut env, &["-g", "oslo_d5=global"]);
    assert_eq!(env.get_var("oslo_d4"), Some("inner"));
    env.pop_scope();
    env.exit_function();
    assert_eq!(env.get_var("oslo_d4"), Some("outer"));
    assert_eq!(env.get_var("oslo_d5"), Some("global"));
}

/// `declare -g` reaches the global even when a local of the same name hides it.
#[test]
fn global_writes_past_a_local() {
    let mut env = Environment::new();
    env.set_var("oslo_d6", "1", false);
    env.enter_function().unwrap();
    env.push_scope();
    declare(&mut env, &["oslo_d6=2"]);
    declare(&mut env, &["-g", "oslo_d6=3"]);
    assert_eq!(env.get_var("oslo_d6"), Some("2"));
    env.pop_scope();
    env.exit_function();
    assert_eq!(env.get_var("oslo_d6"), Some("3"));
}

#[test]
fn the_integer_and_case_attributes_shape_the_value() {
    let mut env = Environment::new();
    declare(&mut env, &["-i", "oslo_d8=2+3"]);
    assert_eq!(env.get_var("oslo_d8"), Some("5"));
    declare(&mut env, &["-u", "oslo_d9=abc"]);
    assert_eq!(env.get_var("oslo_d9"), Some("ABC"));
    declare(&mut env, &["-ai", "oslo_d10=(1+1 2*3)"]);
    assert_eq!(env.get_array("oslo_d10").unwrap().joined(" "), "2 6");
}

#[test]
fn associative_arrays_are_declared() {
    let mut env = Environment::new();
    assert_eq!(declare(&mut env, &["-A", "oslo_m=([a]=1 [b]=2)"]), 0);
    let map = env.get_assoc("oslo_m").expect("an associative array");
    assert_eq!(map.get("b"), Some("2"));
    // An indexed array cannot become an associative one.
    declare(&mut env, &["-a", "oslo_ix"]);
    assert_eq!(declare(&mut env, &["-A", "oslo_ix"]), 1);
}

#[test]
fn a_nameref_points_at_another_name() {
    let mut env = Environment::new();
    env.set_var("oslo_t", "1", false);
    assert_eq!(declare(&mut env, &["-n", "oslo_r=oslo_t"]), 0);
    assert_eq!(env.get_var("oslo_r"), Some("1"));
    env.set_var("oslo_r", "2", false);
    assert_eq!(env.get_var("oslo_t"), Some("2"));
    assert_eq!(declare(&mut env, &["-n", "oslo_s=oslo_s"]), 1);
}

#[test]
fn an_element_operand_assigns_the_element() {
    let mut env = Environment::new();
    declare(&mut env, &["oslo_e[1]=x"]);
    assert_eq!(env.get_array("oslo_e").unwrap().get(1), Some("x"));
}

#[test]
fn an_invalid_name_is_reported() {
    let mut env = Environment::new();
    assert_eq!(declare(&mut env, &["1bad=x"]), 1);
    assert_eq!(declare(&mut env, &["-p", "oslo_no_such_var"]), 1);
    assert_eq!(declare(&mut env, &["-f", "nosuch"]), 1);
}

#[test]
fn operands_split_into_name_subscript_and_value() {
    let op = split_operand("m[a b]+=x").unwrap();
    assert_eq!(
        (op.name, op.subscript, op.value, op.append),
        ("m", Some("a b"), Some("x"), true)
    );
    assert!(split_operand("m[x=1").is_none());
    assert!(split_operand("x y").is_none());
}

#[test]
fn local_outside_a_function_is_refused() {
    let mut env = Environment::new();
    assert_eq!(
        builtin_local(&mut env, &argv(&["local", "x=1"])).unwrap(),
        1
    );
    assert!(env.get_var("x").is_none());
}

/// `local -r x=1` used to declare two variables, one of them called `-r`.
#[test]
fn local_options_are_not_names() {
    let mut env = Environment::new();
    env.enter_function().unwrap();
    env.push_scope();
    assert_eq!(
        builtin_local(&mut env, &argv(&["local", "-r", "LOCAL_RO=1"])).unwrap(),
        0
    );
    assert!(env.get_var("-r").is_none());
    assert_eq!(env.get_var("LOCAL_RO"), Some("1"));
    assert!(env.is_readonly("LOCAL_RO"));
    env.pop_scope();
    assert!(!env.is_readonly("LOCAL_RO"));
}

/// `local -a x` declares an array, which is what `x+=(a b)` on the next line needs.
#[test]
fn local_dash_a_declares_an_array() {
    let mut env = Environment::new();
    env.enter_function().unwrap();
    env.push_scope();
    assert_eq!(
        builtin_local(&mut env, &argv(&["local", "-a", "ARR"])).unwrap(),
        0
    );
    assert_eq!(env.get_array("ARR").map(|a| a.len()), Some(0));
    env.pop_scope();
}
