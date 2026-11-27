//! What a pure script's results look like on standard output.

use super::{imported, print_one};
use oslo_base::value::{Table, Value};

fn printed(value: &Value) -> String {
    let mut out = Vec::new();
    print_one(&mut out, value, false).expect("prints");
    String::from_utf8(out).expect("utf-8")
}

#[test]
fn keys_come_out_in_canonical_order_whatever_order_they_went_in() {
    let mut table = Table::new();
    table.set_str("b", Value::int(2));
    table.set_str("a", Value::int(1));
    table.set(Value::int(10), Value::str("ten"));
    table.set(Value::int(2), Value::str("two"));
    assert_eq!(
        printed(&Value::table(table)),
        "{\"2\":\"two\",\"10\":\"ten\",\"a\":1,\"b\":2}\n"
    );
}

#[test]
fn a_sequence_is_an_array() {
    let mut table = Table::new();
    table.set(Value::int(1), Value::str("x"));
    table.set(Value::int(2), Value::str("y"));
    assert_eq!(printed(&Value::table(table)), "[\"x\",\"y\"]\n");
}

#[test]
fn scalars_print_as_tostring_and_nil_prints_nothing() {
    assert_eq!(printed(&Value::str("hi")), "hi\n");
    assert_eq!(printed(&Value::int(3)), "3\n");
    assert_eq!(printed(&Value::Bool(false)), "false\n");
    assert_eq!(printed(&Value::Nil), "");
}

#[test]
fn an_import_finds_the_lua_file_beside_the_script() {
    let dir = std::env::temp_dir().join(format!("oslo-func-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("lib.lua"), "return 1").expect("write");

    let found = (imported(&dir, "lib"), imported(&dir, "lib.lua"));
    let missing = imported(&dir, "missing");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(found.0.expect("without the extension"), b"return 1");
    assert_eq!(found.1.expect("with it"), b"return 1");
    assert!(missing.is_err(), "a missing file was imported");
}
