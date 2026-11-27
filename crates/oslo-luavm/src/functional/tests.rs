//! What a `--func` script may and may not do.

use super::run;
use oslo_base::value::{LuaResult, Value};

/// `source` with no arguments and no files to import.
fn ran(source: &str) -> LuaResult<Vec<Value>> {
    run(source, "=pure", &[], false, |name| {
        Err(format!("no file {name}"))
    })
}

/// The message of a run that was refused.
fn refused(source: &str) -> String {
    match ran(source) {
        Ok(values) => panic!("`{source}` ran and returned {values:?}"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn a_pure_chunk_answers_what_it_returned() {
    let out = run(
        "local a, b = ... return a + b, 'done'",
        "=pure",
        &[Value::str("2"), Value::str("3")],
        false,
        |_| Err("unused".to_string()),
    )
    .expect("runs");
    assert_eq!(
        format!("{out:?}"),
        format!("{:?}", vec![Value::int(5), Value::str("done")])
    );
}

#[test]
fn assigning_a_captured_variable_does_not_compile() {
    let message = refused("local n = 0 local function bump() n = n + 1 end bump() return n");
    assert!(
        message.contains("captured variable"),
        "the wrong refusal: {message}"
    );
}

#[test]
fn the_library_is_frozen() {
    let message = refused("string.upper = nil return 1");
    assert!(!message.is_empty(), "changing the string library ran");
}

#[test]
fn nothing_reaches_the_outside_world() {
    let out = ran("return io == nil and os == nil and print == nil and oslo == nil").expect("runs");
    assert_eq!(format!("{out:?}"), format!("{:?}", vec![Value::Bool(true)]));
}

#[test]
fn an_import_is_evaluated_once_and_frozen() {
    let loader = |name: &str| match name {
        "lib" => Ok(b"return { two = 2 }".to_vec()),
        other => Err(format!("no file {other}")),
    };
    let out = run(
        "return import('lib').two + import('lib').two",
        "=pure",
        &[],
        false,
        loader,
    )
    .expect("runs");
    assert_eq!(format!("{out:?}"), format!("{:?}", vec![Value::int(4)]));

    let changed = run(
        "import('lib').two = 3 return 1",
        "=pure",
        &[],
        false,
        loader,
    );
    assert!(changed.is_err(), "an imported table could be changed");
}

#[test]
fn the_jit_gives_the_same_answer() {
    let source = "local s = 0 for i = 1, 200000 do s = s + i end return s";
    let interpreted = ran(source).expect("runs");
    let native = run(source, "=pure", &[], true, |_| Err("unused".to_string())).expect("runs");
    assert_eq!(format!("{interpreted:?}"), format!("{native:?}"));
}
