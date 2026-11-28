//! `oslo.secret` — the secrets store, from Lua.
//!
//! ```lua
//! local value = oslo.secret.get("gh-token")        -- the store this shell would use
//! oslo.secret.set("gh-token", value)
//! for _, name in ipairs(oslo.secret.list()) do print(name) end
//! oslo.secret.forget("gh-token")
//!
//! local work = oslo.secret.open("work")            -- another store by name
//! print(work:get("deploy"))
//!
//! local mine = oslo.secret.mine()                  -- a plugin's own encrypted store
//! mine:set("cursor", "42")
//!
//! local sealed = oslo.secret.seal("anything")      -- base64 of an age file
//! print(oslo.secret.unseal(sealed))
//! ```
//!
//! # Three doors, on purpose
//!
//! `get`/`set`/`forget`/`list` act on the store this invocation would use — `$OSLO_SECRET_STORE`,
//! the `default` line, or `user`. `open(name)` names another. `mine()` is a plugin's own, and takes
//! no name because the name is not the plugin's to write.
//!
//! # What a plugin may reach
//!
//! **`plugin.` is refused from Lua, always, to every caller**, whether it is named to `open` or
//! selected for the bare calls through `$OSLO_SECRET_STORE`. `mine()` is the only door to a plugin
//! store. The command line has no such rule: `oslo secret --store plugin.notes list` works.
//!
//! **While a plugin's file loads, a call or a handle is filtered to the names the user's config
//! granted that plugin**: `get`, `set` and `forget` refuse any other name, and `list` answers only
//! granted names. A handle made then keeps the filter for as long as it lives.
//!
//! **`seal` and `unseal` are refused to a plugin's load and to a handle made during one**, because
//! `unseal` opens any file sealed to the store's key whatever its name. `mine():seal` and
//! `mine():unseal` stay, on the plugin's own store. The config, the prompt and every hook keep
//! both: a hook a plugin registered is indistinguishable from one the config did.
//!
//! None of this is a sandbox: a plugin can run `oslo secret get` as a subprocess. See
//! `docs/features/secrets.md`.
//!
//! # Bytes, and base64
//!
//! A value is a Lua string, which is a byte string. `seal` and `unseal` are the exception: they
//! carry a whole age *file*, which is binary, so they speak base64 rather than putting a NUL and a
//! header through anything that might treat it as text.

mod defined;

use super::util::{ok, put, text};
use oslo_base::secrets::{self, Store};
use oslo_base::value::{Table, Value};

/// Build the `oslo.secret` table.
pub fn build() -> Value {
    let mut secret = Table::new();

    // oslo.secret.open(name) -> handle, or nil + message
    put(&mut secret, "open", |_, args| {
        let name = text(&args, 1, "oslo.secret.open")?;
        match reachable(&name).and_then(|()| Store::named(&name)) {
            // A store Lua defined keeps its own bytes; everything else is a directory of files.
            Ok(store) if defined::holds(&name) => ok(defined::handle(store, allowed())),
            Ok(store) => ok(handle(store, allowed())),
            Err(message) => Ok(vec![Value::Nil, Value::str(message)]),
        }
    });

    // oslo.secret.define(name, handlers) -> where this store's bytes live
    put(&mut secret, "define", |_, args| defined::define(&args));

    // oslo.secret.through({argv…}, base64) -> base64, or nil + message
    //
    // **The one thing a crypto hook cannot do for itself.** A handler is handed base64 and must
    // answer base64, while `age` and everything like it speak raw bytes on a pipe — and a Lua string
    // cannot hold those, so there is no way to build this out of `oslo.run`. Putting the payload in
    // argv instead would publish the plaintext to `ps` on the encrypt side.
    put(&mut secret, "through", |_, args| {
        let Some(Value::Table(list)) = args.first() else {
            return Ok(vec![
                Value::Nil,
                Value::str("oslo.secret.through: the first argument is a list of words"),
            ]);
        };
        let argv: Vec<String> = list
            .borrow()
            .sequence()
            .iter()
            .filter_map(|word| match word {
                Value::Str(text) => Some(text.to_string()),
                _ => None,
            })
            .collect();
        if argv.is_empty() {
            return Ok(vec![
                Value::Nil,
                Value::str("oslo.secret.through: no program to run"),
            ]);
        }
        let payload = text(&args, 2, "oslo.secret.through")?;
        let Some(bytes) = unbase64(&payload) else {
            return Ok(vec![
                Value::Nil,
                Value::str("oslo.secret.through: the payload is not base64"),
            ]);
        };
        Ok(match secrets::cipher::through(&argv, &bytes) {
            Ok(answered) => vec![Value::str(base64(&answered))],
            Err(message) => vec![Value::Nil, Value::str(message)],
        })
    });

    // oslo.secret.mine() -> the calling plugin's own store
    #[cfg(feature = "plugin")]
    put(&mut secret, "mine", |_, _| {
        let Some(who) = who() else {
            return Ok(vec![
                Value::Nil,
                Value::str("oslo.secret.mine: only a plugin has one, and only while it loads"),
            ]);
        };
        match Store::named(&format!("{}{}", secrets::PLUGIN, who.plugin)) {
            // Its own store, and no filter on it: the names in there are the plugin's own.
            Ok(store) => ok(handle(store, None)),
            Err(message) => Ok(vec![Value::Nil, Value::str(message)]),
        }
    });

    // oslo.secret.stores() -> every store this machine declares
    put(&mut secret, "stores", |_, _| {
        let declared = match secrets::conf::read() {
            Ok(conf) => conf,
            Err(message) => return Ok(vec![Value::Nil, Value::str(message)]),
        };
        let mut names: Vec<String> = declared
            .sections
            .iter()
            .map(|section| section.name.clone())
            .filter(|name| !name.starts_with(secrets::PLUGIN))
            .collect();
        if !names.iter().any(|name| name == secrets::USER) {
            names.insert(0, secrets::USER.to_string());
        }
        for name in defined::names() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        ok(list(&names))
    });

    for (verb, on_selected) in DIRECT {
        put(&mut secret, verb, move |_, args| {
            let store = match selected() {
                Ok(store) => store,
                Err(message) => return Ok(vec![Value::Nil, Value::str(message)]),
            };
            on_selected(&store, &args)
        });
    }

    Value::table(secret)
}

/// The store a bare `oslo.secret.X` acts on, held to the same rule as `open`.
fn selected() -> Result<Store, String> {
    let store = Store::selected(None)?;
    reachable(&store.name)?;
    Ok(store)
}

/// What a bare `oslo.secret.X` does, against the store this shell would use.
type Direct = fn(&Store, &[Value]) -> oslo_base::value::LuaResult<Vec<Value>>;
const DIRECT: &[(&str, Direct)] = &[
    ("get", |store, args| {
        let name = text(args, 1, "oslo.secret.get")?;
        Ok(read(store, &name, allowed()))
    }),
    ("set", |store, args| {
        let name = text(args, 1, "oslo.secret.set")?;
        let value = text(args, 2, "oslo.secret.set")?;
        Ok(wrote(store, &name, &value, allowed()))
    }),
    ("forget", |store, args| {
        let name = text(args, 1, "oslo.secret.forget")?;
        Ok(forgot(store, &name, allowed()))
    }),
    ("list", |store, _| {
        ok(list(&visible(store.names(), &allowed())))
    }),
    ("seal", |store, args| {
        let value = text(args, 1, "oslo.secret.seal")?;
        Ok(sealed(
            store,
            &value,
            allowed().is_some(),
            "oslo.secret.seal",
        ))
    }),
    ("unseal", |store, args| {
        let sealed = text(args, 1, "oslo.secret.unseal")?;
        Ok(opened(
            store,
            &sealed,
            allowed().is_some(),
            "oslo.secret.unseal",
        ))
    }),
];

/// `seal`: the store's crypto without its filing, refused to a plugin.
fn sealed(store: &Store, value: &str, plugin: bool, verb: &str) -> Vec<Value> {
    if plugin {
        return vec![Value::Nil, Value::str(not_to_a_plugin(verb))];
    }
    match store.seal(value.as_bytes()) {
        Ok(sealed) => vec![Value::str(base64(&sealed))],
        Err(message) => vec![Value::Nil, Value::str(message)],
    }
}

/// `unseal`: the other half, refused to a plugin the same way.
fn opened(store: &Store, sealed: &str, plugin: bool, verb: &str) -> Vec<Value> {
    if plugin {
        return vec![Value::Nil, Value::str(not_to_a_plugin(verb))];
    }
    let Some(bytes) = unbase64(sealed) else {
        return vec![Value::Nil, Value::str("not base64")];
    };
    match store.unseal(&bytes) {
        Ok(value) => vec![handed(&value)],
        Err(message) => vec![Value::Nil, Value::str(message)],
    }
}

fn not_to_a_plugin(verb: &str) -> String {
    format!(
        "{verb}: not while a plugin loads, nor on a handle it made — a plugin seals with \
         oslo.secret.mine()"
    )
}

/// A value leaving the store for Lua, remembered so a variable later given it is marked sensitive.
pub(crate) fn handed(value: &[u8]) -> Value {
    let text = String::from_utf8_lossy(value);
    oslo_shell::env::scope::sensitive::note(text.as_bytes());
    Value::str(text)
}

/// The plugin whose file is running, when there is one and this build has plugins at all.
///
/// A build without the `plugin` feature has no loader, so nothing is ever loading and every caller
/// is the config or the prompt.
#[cfg(feature = "plugin")]
fn who() -> Option<crate::plugin::loading::Loading> {
    crate::plugin::loading::current()
}

/// The names this caller may read, or `None` for all of them.
///
/// Taken at the moment a handle is made or a call is answered, which is what makes it attribution:
/// it is the manifest of whatever plugin is loading, and nothing at all otherwise.
#[cfg(feature = "plugin")]
fn allowed() -> Option<Vec<String>> {
    who().map(|it| it.secrets)
}

#[cfg(not(feature = "plugin"))]
fn allowed() -> Option<Vec<String>> {
    None
}

/// Whether Lua may name this store at all.
fn reachable(name: &str) -> Result<(), String> {
    match name.starts_with(secrets::PLUGIN) {
        true => Err(format!(
            "{name}: a plugin's store is reached with oslo.secret.mine(), not by name"
        )),
        false => Ok(()),
    }
}

/// The table `open` and `mine` answer with.
fn handle(store: Store, allowed: Option<Vec<String>>) -> Value {
    let mut table = Table::new();

    let (it, may) = (store.clone(), allowed.clone());
    put(&mut table, "get", move |_, args| {
        let name = text(&args, 2, "store:get")?;
        Ok(read(&it, &name, may.clone()))
    });

    let (it, may) = (store.clone(), allowed.clone());
    put(&mut table, "set", move |_, args| {
        let name = text(&args, 2, "store:set")?;
        let value = text(&args, 3, "store:set")?;
        Ok(wrote(&it, &name, &value, may.clone()))
    });

    let (it, may) = (store.clone(), allowed.clone());
    put(&mut table, "forget", move |_, args| {
        let name = text(&args, 2, "store:forget")?;
        Ok(forgot(&it, &name, may.clone()))
    });

    let (it, may) = (store.clone(), allowed.clone());
    put(&mut table, "list", move |_, _| {
        ok(list(&visible(it.names(), &may)))
    });

    let it = store.clone();
    put(&mut table, "where", move |_, _| {
        ok(Value::str(it.directory.to_string_lossy()))
    });

    let (it, plugin) = (store.clone(), allowed.is_some());
    put(&mut table, "seal", move |_, args| {
        let value = text(&args, 2, "store:seal")?;
        Ok(sealed(&it, &value, plugin, "store:seal"))
    });

    let (it, plugin) = (store, allowed.is_some());
    put(&mut table, "unseal", move |_, args| {
        let sealed = text(&args, 2, "store:unseal")?;
        Ok(opened(&it, &sealed, plugin, "store:unseal"))
    });

    Value::table(table)
}

fn read(store: &Store, name: &str, may: Option<Vec<String>>) -> Vec<Value> {
    if let Some(refused) = refusal(name, may) {
        return vec![Value::Nil, Value::str(refused)];
    }
    match store.get(name) {
        Ok(value) => vec![handed(&value)],
        Err(message) => vec![Value::Nil, Value::str(message)],
    }
}

fn forgot(store: &Store, name: &str, may: Option<Vec<String>>) -> Vec<Value> {
    if let Some(refused) = refusal(name, may) {
        return vec![Value::Nil, Value::str(refused)];
    }
    match store.forget(name) {
        Ok(()) => vec![Value::Bool(true)],
        Err(message) => vec![Value::Nil, Value::str(message)],
    }
}

/// The names a caller may see: all of them, or only the granted ones while a plugin loads.
pub(crate) fn visible(names: Vec<String>, may: &Option<Vec<String>>) -> Vec<String> {
    match may {
        Some(allowed) => names
            .into_iter()
            .filter(|name| allowed.contains(name))
            .collect(),
        None => names,
    }
}

fn wrote(store: &Store, name: &str, value: &str, may: Option<Vec<String>>) -> Vec<Value> {
    if let Some(refused) = refusal(name, may) {
        return vec![Value::Nil, Value::str(refused)];
    }
    match store.set(name, value.as_bytes()) {
        Ok(()) => vec![Value::Bool(true)],
        Err(message) => vec![Value::Nil, Value::str(message)],
    }
}

/// Why this caller may not touch `name`, if it may not.
pub(crate) fn refusal(name: &str, may: Option<Vec<String>>) -> Option<String> {
    let allowed = may?;
    if allowed.iter().any(|declared| declared == name) {
        return None;
    }
    Some(format!(
        "{name}: not declared in this plugin's `secrets = {{ … }}`"
    ))
}

fn list(names: &[String]) -> Value {
    let mut table = Table::new();
    for (at, name) in names.iter().enumerate() {
        table.set(Value::int(at as i64 + 1), Value::str(name.clone()));
    }
    Value::table(table)
}

/// **Base64 rather than the bytes**, and rather than armour: an age file is binary, and `armor`
/// costs 45 KB measured for a format nothing outside this boundary would read.
pub(crate) fn base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(crate) fn unbase64(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .ok()
}
