//! `oslo.json` — JSON, in the binary rather than as a library.
//!
//! A shell that talks to anything modern reads JSON, and on a static musl build the usual Lua
//! answers are unavailable: `lua-cjson` is a C module and cannot be `dlopen`ed, and the pure-Lua
//! ones are slow enough to notice on a large document. So this is a core capability.
//!
//! **The empty-table problem is the interesting one.** Lua has one table type, so `{}` could mean
//! `[]` or `{}` and the encoder cannot tell. It is written as `[]`, because a shell script's
//! empty collection is nearly always a list — and [`decode`] marks what it decoded, so a document
//! that round-trips through Lua keeps its shape.

use super::util::{failed, ok, put, text};
use oslo_base::value::{LuaError, LuaResult};
use oslo_base::value::{Number, Table, Value};
use serde_json::Value as Json;

/// Key an encoder looks at to decide between an array and an object.
///
/// Set by [`decode`] on every table it produces, so `oslo.json.encode(oslo.json.decode(s))` keeps
/// an empty object an object. A script can set it by hand for the same reason.
const KIND: &str = "__json";

pub fn build() -> Value {
    let mut json = Table::new();

    // oslo.json.decode(text) -> value, or nil + message
    put(&mut json, "decode", |_, args| {
        let source = text(&args, 1, "oslo.json.decode")?;
        match serde_json::from_str::<Json>(&source) {
            Ok(parsed) => ok(from_json(&parsed)),
            Err(e) => failed("oslo.json.decode", e),
        }
    });

    // oslo.json.encode(value, indent) -> text
    //
    // A second argument asks for indented output. Compact by default, because the common use is
    // handing a document to another program rather than to a person.
    put(&mut json, "encode", |_, args| {
        let value = args.first().cloned().unwrap_or(Value::Nil);
        let encoded = to_json(&value, 0)?;
        let pretty = args.get(1).is_some_and(Value::truthy);
        let text = if pretty {
            serde_json::to_string_pretty(&encoded)
        } else {
            serde_json::to_string(&encoded)
        };
        match text {
            Ok(text) => ok(Value::str(text)),
            Err(e) => failed("oslo.json.encode", e),
        }
    });

    Value::table(json)
}

/// A decoded JSON document as Lua values.
///
/// Shared with `oslo.nix`, which decodes what `nix --json` writes. **One decoder, not two**: the
/// `null` and empty-table decisions above are the sort a second implementation would make
/// differently without anyone noticing until a document round-tripped wrong.
pub(super) fn from_json(value: &Json) -> Value {
    match value {
        // JSON `null` decodes to `false`, not `nil`. `nil` in a table is indistinguishable from
        // an absent key, so a null field would silently disappear from the document — and a
        // script checking `if doc.field == nil` could not tell "not present" from "present and
        // null", which is often exactly the distinction it needs.
        Json::Null => Value::Bool(false),
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => match n.as_i64() {
            // An integral JSON number becomes a Lua integer, so `doc.count` can index a table and
            // prints as `3` rather than `3.0`.
            Some(i) => Value::int(i),
            None => Value::float(n.as_f64().unwrap_or(f64::NAN)),
        },
        Json::String(s) => Value::str(s),
        Json::Array(items) => {
            let mut table = Table::new();
            for (i, item) in items.iter().enumerate() {
                table.set(Value::int(i as i64 + 1), from_json(item));
            }
            table.set(Value::str(KIND), Value::str("array"));
            Value::table(table)
        }
        Json::Object(fields) => {
            let mut table = Table::new();
            for (key, item) in fields {
                table.set(Value::str(key), from_json(item));
            }
            table.set(Value::str(KIND), Value::str("object"));
            Value::table(table)
        }
    }
}

/// `value` as JSON text, as `oslo.json.encode` writes it.
pub(crate) fn encode(value: &Value, pretty: bool) -> LuaResult<String> {
    let encoded = to_json(value, 0)?;
    let text = match pretty {
        true => serde_json::to_string_pretty(&encoded),
        false => serde_json::to_string(&encoded),
    };
    text.map_err(|e| LuaError::new(format!("oslo.json.encode: {e}")))
}

/// How deep encoding may go before a cyclic table is reported rather than recursed into.
const MAX_DEPTH: usize = 100;

/// A Lua value as JSON.
fn to_json(value: &Value, depth: usize) -> LuaResult<Json> {
    if depth > MAX_DEPTH {
        // A table containing itself is legal Lua and has no JSON form. Reporting it beats
        // recursing until the stack gives out, which is what a naive encoder does.
        return Err(LuaError::new(
            "oslo.json.encode: the value is too deeply nested, or contains itself",
        ));
    }
    Ok(match value {
        Value::Nil => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Number(Number::Int(i)) => Json::from(*i),
        Value::Number(Number::Float(f)) => serde_json::Number::from_f64(*f)
            .map(Json::Number)
            // JSON has no infinity and no NaN. Null is what every other encoder writes for them.
            .unwrap_or(Json::Null),
        Value::Str(s) => Json::String(s.to_string()),
        // **Refused rather than mangled.** A JSON string is a sequence of Unicode characters, so
        // there is no encoding of these bytes that is still the same bytes — writing them lossily
        // would produce a document that parses and no longer says what was handed over.
        // `oslo.fs.read` on a binary file lands here, and this is what tells you so.
        Value::Bytes(_) => {
            return Err(LuaError::new(
                "oslo.json.encode: the value is bytes rather than text, and JSON has no form for it",
            ));
        }
        Value::Table(t) => table_to_json(t, depth)?,
        Value::Function(_) => {
            return Err(LuaError::new(
                "oslo.json.encode: a function has no JSON representation",
            ));
        }
    })
}

fn table_to_json(t: &std::rc::Rc<std::cell::RefCell<Table>>, depth: usize) -> LuaResult<Json> {
    let table = t.borrow();
    let marked = table.get(&Value::str(KIND));
    let sequence = table.sequence();
    // Everything outside the sequence part, discounting the marker itself.
    let named = table.pairs().len() - sequence.len() - usize::from(!matches!(marked, Value::Nil));
    // An explicit mark wins; otherwise anything with a named key is an object, and everything
    // else — including the empty table — is an array.
    let as_array = match &marked {
        Value::Str(s) if &**s == "object" => false,
        Value::Str(s) if &**s == "array" => true,
        _ => named == 0,
    };

    if as_array {
        let mut items = Vec::with_capacity(sequence.len());
        for item in sequence {
            items.push(to_json(item, depth + 1)?);
        }
        return Ok(Json::Array(items));
    }

    let mut object = serde_json::Map::new();
    for (key, item) in table.pairs() {
        // The marker is bookkeeping, not data.
        if matches!(&key, Value::Str(s) if &**s == KIND) {
            continue;
        }
        // A JSON key is a string. A number key is written as its digits — which is what every
        // other encoder does and is at least reversible — and anything else is refused, because
        // `table: 0x55f…` as a key is data loss dressed up as success.
        let name = match &key {
            Value::Str(s) => s.to_string(),
            Value::Number(n) => n.to_string(),
            other => {
                return Err(LuaError::new(format!(
                    "oslo.json.encode: a {} cannot be a JSON key",
                    other.type_name()
                )));
            }
        };
        object.insert(name, to_json(&item, depth + 1)?);
    }
    Ok(Json::Object(object))
}
