//! A store whose *storage* is Lua's — `oslo.secret.define`.
//!
//! ```lua
//! oslo.secret.define("vault", {
//!   get    = function(name) return db:get(name) end,   -- base64 in and out
//!   set    = function(name, sealed) db:set(name, sealed) return true end,
//!   list   = function() return db:keys() end,
//!   forget = function(name) db:delete(name) return true end,   -- optional
//! })
//! ```
//!
//! # Two axes, and this is only one of them
//!
//! Storage is where the bytes live: a database, a file somewhere else, a network call, another
//! program. Crypto is what those bytes are — and it stays whatever the store *declares*, so by
//! default a defined store is sealed with your ordinary key and the Lua storing it never sees a
//! plaintext. `crypto hook` in `secrets.conf` is how the other axis gets replaced too.
//!
//! # base64 both ways
//!
//! What crosses is ciphertext, which is binary, and a Lua string is UTF-8. So `get` answers base64
//! and `set` is handed base64 — the same shape `oslo.secret.seal` speaks, which is what lets a
//! handler pass one straight to the other.
//!
//! # What may not be defined
//!
//! `user`, and anything under `plugin.`. The first is the store the command line means, and a
//! definition that shadowed it would make `oslo.secret.get` and `oslo secret get` disagree about
//! what your own secrets are. The second is reached by `oslo.secret.mine()` and is not a name Lua
//! writes. Everything else is open, and is worth knowing about: a config or a plugin that defines
//! `work` decides what `oslo.secret.open("work")` reads *in this shell*. It does not touch what
//! `oslo secret --store work` reads, because that process has no Lua.

use super::super::util::{ok, put, text};
use oslo_base::secrets::{self, Store};
use oslo_base::value::{Table, Value};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// The storage handlers, by store name.
    static DEFINED: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
}

/// `oslo.secret.define(name, handlers)`.
pub fn define(args: &[Value]) -> oslo_base::value::LuaResult<Vec<Value>> {
    let name = text(args, 1, "oslo.secret.define")?;
    if name == secrets::USER || name.starts_with(secrets::PLUGIN) {
        return Ok(vec![
            Value::Nil,
            Value::str(format!("{name}: not a store Lua may define")),
        ]);
    }
    if let Err(e) = secrets::conf::valid_store_name(&name) {
        return Ok(vec![Value::Nil, Value::str(e)]);
    }
    let Some(handlers @ Value::Table(_)) = args.get(1) else {
        return Ok(vec![
            Value::Nil,
            Value::str("oslo.secret.define: the second argument is a table of handlers"),
        ]);
    };
    for wanted in ["get", "set", "list"] {
        if !has(handlers, wanted) {
            return Ok(vec![
                Value::Nil,
                Value::str(format!("oslo.secret.define: no `{wanted}` handler")),
            ]);
        }
    }
    DEFINED.with(|slot| {
        slot.borrow_mut().insert(name.clone(), handlers.clone());
    });
    ok(Value::Bool(true))
}

/// Whether Lua has taken over this name's storage.
pub fn holds(name: &str) -> bool {
    DEFINED.with(|slot| slot.borrow().contains_key(name))
}

/// Every name Lua has defined, sorted.
pub fn names() -> Vec<String> {
    DEFINED.with(|slot| {
        let mut names: Vec<String> = slot.borrow().keys().cloned().collect();
        names.sort();
        names
    })
}

/// The handle `oslo.secret.open` answers with for a defined store.
///
/// The crypto is the declared store's — so `[vault] crypto hook` replaces that half too, and a
/// `[vault]` with nothing in it seals with your ordinary key. `may` is the plugin filter
/// `super::handle` takes, applied to the same four verbs.
pub fn handle(store: Store, may: Option<Vec<String>>) -> Value {
    let mut table = Table::new();

    let (it, granted) = (store.clone(), may.clone());
    put(&mut table, "get", move |_, args| {
        let name = text(&args, 2, "store:get")?;
        if let Some(refused) = super::refusal(&name, granted.clone()) {
            return Ok(vec![Value::Nil, Value::str(refused)]);
        }
        // Watched here as well as in `Store`, because a defined store never goes through it — and a
        // store somebody else supplied is the one an audit hook most wants to see.
        secrets::hooked::touched(true, &it.name, &name, false);
        let Some(sealed) = call(&it.name, "get", vec![Value::str(name.clone())])? else {
            return Ok(vec![Value::Nil]);
        };
        let Some(bytes) = super::unbase64(&sealed) else {
            return Ok(vec![
                Value::Nil,
                Value::str("its `get` did not answer base64"),
            ]);
        };
        let value = it.unseal_named(&name, &bytes);
        secrets::hooked::touched(false, &it.name, &name, false);
        Ok(match value {
            Ok(value) => vec![super::handed(&value)],
            Err(message) => vec![Value::Nil, Value::str(message)],
        })
    });

    let (it, granted) = (store.clone(), may.clone());
    put(&mut table, "set", move |_, args| {
        let name = text(&args, 2, "store:set")?;
        let value = text(&args, 3, "store:set")?;
        if let Some(refused) = super::refusal(&name, granted.clone()) {
            return Ok(vec![Value::Nil, Value::str(refused)]);
        }
        secrets::hooked::touched(true, &it.name, &name, true);
        let sealed = match it.seal_named(&name, value.as_bytes()) {
            Ok(sealed) => sealed,
            Err(message) => return Ok(vec![Value::Nil, Value::str(message)]),
        };
        call(
            &it.name,
            "set",
            vec![Value::str(name.clone()), Value::str(super::base64(&sealed))],
        )?;
        secrets::hooked::touched(false, &it.name, &name, true);
        ok(Value::Bool(true))
    });

    let (it, granted) = (store.clone(), may.clone());
    put(&mut table, "forget", move |_, args| {
        let name = text(&args, 2, "store:forget")?;
        if let Some(refused) = super::refusal(&name, granted.clone()) {
            return Ok(vec![Value::Nil, Value::str(refused)]);
        }
        match call(&it.name, "forget", vec![Value::str(name)])? {
            Some(_) => ok(Value::Bool(true)),
            None => Ok(vec![Value::Nil, Value::str("it has no `forget` handler")]),
        }
    });

    let it = store.clone();
    put(&mut table, "list", move |_, _| {
        let listed =
            DEFINED.with(|slot| slot.borrow().get(&it.name).and_then(|h| field(h, "list")));
        let Some(f) = listed else {
            return ok(Value::table(Table::new()));
        };
        let answered = match crate::lua::engine::call_here(&f, Vec::new()) {
            Ok(answers) => answers.into_iter().next().unwrap_or(Value::Nil),
            Err(problem) => return Ok(vec![Value::Nil, Value::str(problem.to_string())]),
        };
        if may.is_none() {
            return Ok(vec![answered]);
        }
        let Value::Table(names) = answered else {
            return ok(Value::table(Table::new()));
        };
        let names: Vec<String> = names
            .borrow()
            .sequence()
            .iter()
            .filter_map(|name| match name {
                Value::Str(text) => Some(text.to_string()),
                _ => None,
            })
            .collect();
        ok(super::list(&super::visible(names, &may)))
    });

    let it = store;
    put(&mut table, "where", move |_, _| {
        ok(Value::str(format!("{}: defined in Lua", it.name)))
    });

    Value::table(table)
}

/// Call one of a defined store's handlers, if it has that one.
fn call(store: &str, which: &str, args: Vec<Value>) -> oslo_base::value::LuaResult<Option<String>> {
    let handler = DEFINED.with(|slot| slot.borrow().get(store).and_then(|h| field(h, which)));
    let Some(f) = handler else {
        return Ok(None);
    };
    match crate::lua::engine::call_here(&f, args) {
        Ok(answers) => Ok(match answers.into_iter().next() {
            Some(Value::Str(text)) => Some(text.to_string()),
            // Anything that is not a string is not bytes this store can have kept, and treating it
            // as absent is the answer that does not invent a value.
            _ => None,
        }),
        Err(problem) => Err(oslo_base::value::LuaError::new(format!(
            "{store}: its `{which}` failed: {problem}"
        ))),
    }
}

fn field(table: &Value, name: &str) -> Option<Value> {
    let Value::Table(t) = table else { return None };
    match t.borrow().get(&Value::str(name)) {
        f @ Value::Function(_) => Some(f),
        _ => None,
    }
}

fn has(table: &Value, name: &str) -> bool {
    field(table, name).is_some()
}
