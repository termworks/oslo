//! `--func`: a pure Lua script, run on luna's functional state.
//!
//! What the script returns is what it prints, one value to a line: a string as it is, a number or
//! a boolean as `tostring` writes it, and a table as JSON. Table keys come out in luna's canonical
//! order (booleans, numbers, then strings by bytes), so a script prints the same bytes every run.
//! `nil` prints nothing. See `oslo_luavm::functional` for what a pure script may not do.

use oslo_base::value::{Table, Value};
use std::cmp::Ordering;
use std::io::{IsTerminal, Write};
use std::path::Path;

/// How deep a returned table may nest before it is taken to contain itself.
const MAX_DEPTH: usize = 100;

/// Run a pure script and print what it returned, answering the exit status.
///
/// `import(name)` reads `name`, or `name.lua`, from the script's directory. JSON is indented on
/// a terminal and compact anywhere else.
pub fn run(source: &str, name: &str, args: &[String], jit: bool) -> i32 {
    let beside = Path::new(name)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let args: Vec<Value> = args.iter().map(Value::str).collect();
    let loader = move |wanted: &str| imported(&beside, wanted);
    let values = match oslo_luavm::functional::run(source, name, &args, jit, loader) {
        Ok(values) => values,
        Err(e) => {
            eprintln!("oslo: {e}");
            return 1;
        }
    };
    let pretty = std::io::stdout().is_terminal();
    let mut out = std::io::stdout().lock();
    for value in &values {
        if let Err(e) = print_one(&mut out, value, pretty) {
            eprintln!("oslo: {name}: {e}");
            return 1;
        }
    }
    0
}

/// Write one returned value and its newline.
fn print_one(out: &mut impl Write, value: &Value, pretty: bool) -> Result<(), String> {
    let bytes = match value {
        Value::Nil => return Ok(()),
        Value::Bytes(bytes) => bytes.to_vec(),
        Value::Table(_) => crate::lua::api::json::encode(&canonical(value, 0)?, pretty)
            .map_err(|e| e.to_string())?
            .into_bytes(),
        Value::Function(_) => return Err("a function has no printed form".to_string()),
        other => other.to_display().into_bytes(),
    };
    out.write_all(&bytes)
        .and_then(|()| out.write_all(b"\n"))
        .map_err(|e| e.to_string())
}

/// `value` with the keys of every table in it in canonical order.
fn canonical(value: &Value, depth: usize) -> Result<Value, String> {
    let Value::Table(table) = value else {
        return Ok(value.clone());
    };
    if depth > MAX_DEPTH {
        return Err("the value is too deeply nested, or contains itself".to_string());
    }
    let mut pairs = table.borrow().pairs();
    pairs.sort_by(|(a, _), (b, _)| key_order(a, b));
    let mut sorted = Table::new();
    for (key, item) in pairs {
        sorted.set(key, canonical(&item, depth + 1)?);
    }
    Ok(Value::table(sorted))
}

/// luna's canonical key order: booleans, then numbers by value, then strings by bytes.
fn key_order(a: &Value, b: &Value) -> Ordering {
    let rank = |value: &Value| match value {
        Value::Bool(_) => 0,
        Value::Number(_) => 1,
        Value::Str(_) | Value::Bytes(_) => 2,
        _ => 3,
    };
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Number(x), Value::Number(y)) => match (x.as_int(), y.as_int()) {
            (Some(x), Some(y)) => x.cmp(&y),
            _ => x.as_float().total_cmp(&y.as_float()),
        },
        (Value::Str(x), Value::Str(y)) => x.as_bytes().cmp(y.as_bytes()),
        _ => rank(a).cmp(&rank(b)),
    }
}

/// The text `import(wanted)` evaluates: the file beside the script, or the same name with `.lua`.
fn imported(beside: &Path, wanted: &str) -> Result<Vec<u8>, String> {
    let path = beside.join(wanted);
    std::fs::read(&path)
        .or_else(|first| match path.extension() {
            None => std::fs::read(path.with_extension("lua")).map_err(|_| first),
            Some(_) => Err(first),
        })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
