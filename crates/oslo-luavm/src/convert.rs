//! Converting between the shell's value and the VM's, which is the whole boundary.
//!
//! oslo's [`oslo_base::value::Value`] is owned and lives anywhere; luna's `Value<'gc>` is a handle
//! into a collected heap and lives only inside `Lua::enter`. Everything the shell hands to Lua goes
//! one way through here, and everything Lua answers comes back the other, so the lifetime stops at
//! this file rather than spreading through forty crates that have no VM in them.
//!
//! # Functions cross too, and that is what makes the port small
//!
//! [`oslo_base::value::Function::Held`] is an `Rc<dyn Any>` the shell never looks
//! inside. Two different things travel in it, and this file is the only place that knows which:
//!
//! - a [`Native`] — a Rust closure going *in*, wrapped as a luna callback;
//! - a [`StashedFunction`] — a Lua function coming *out*, rooted so the collector keeps it.
//!
//! That is why two hundred registered callables did not have to be rewritten against a VM type.
//! They build shell values and answer shell values; the engine boundary is here.

use crate::host::{CallbackHost, Native};
use luna::{
    Callback, CallbackReturn, Context, Function as LunaFn, StashedFunction, String as LunaStr,
    Table, Value, Variadic,
};
use oslo_base::value::{Function, LuaError, Number, Value as Own};
use std::cell::RefCell;
use std::rc::Rc;

/// What has already crossed, so that a thing that was one thing stays one thing.
///
/// **Identity is a property values have, and copying loses it.** `oslo.from_json` and
/// `oslo.json.decode` are the same `Rc` on the shell's side; converting each occurrence separately
/// makes two callbacks, and `oslo.from_json == oslo.json.decode` — which oslo documents and tests —
/// answers false. The same map is what makes a cyclic table terminate: `t.self = t` would otherwise
/// recurse until the stack ran out.
type Crossed<'gc> = std::collections::HashMap<usize, Value<'gc>>;

/// The shell's value, as something the VM can hold.
pub fn into_lua<'gc>(ctx: Context<'gc>, value: &Own) -> Value<'gc> {
    into_lua_seen(ctx, value, &mut Crossed::new())
}

fn into_lua_seen<'gc>(ctx: Context<'gc>, value: &Own, seen: &mut Crossed<'gc>) -> Value<'gc> {
    match value {
        Own::Nil => Value::Nil,
        Own::Bool(b) => Value::Boolean(*b),
        // Lua's own split, kept: `3` and `3.0` compare equal but format differently and divide
        // differently, which is why `oslo_base` keeps two variants rather than one `f64`.
        Own::Number(Number::Int(i)) => Value::Integer(*i),
        Own::Number(Number::Float(f)) => Value::Number(*f),
        // Copied, not borrowed: `IntoValue for &str` wants `&'static`, and the shell's strings
        // are `Rc<str>` owned by a value that outlives nothing in particular.
        Own::Str(s) => Value::String(LunaStr::from_slice(&ctx, s.as_bytes())),
        // One Lua string type, arrived at from two: the VM's strings have always been bytes, and
        // this is the half of the shell's that is not text. See [`Own::Bytes`].
        Own::Bytes(b) => Value::String(LunaStr::from_slice(&ctx, b)),
        Own::Table(table) => {
            let id = Rc::as_ptr(table) as *const () as usize;
            if let Some(already) = seen.get(&id) {
                return *already;
            }
            // Recorded *before* the entries are walked, so a table that contains itself finds the
            // one being built rather than starting another.
            let out = Table::new(&ctx);
            seen.insert(id, Value::Table(out));
            let entries = table.borrow().pairs();
            for (key, value) in entries {
                // A `set` only fails on a nil or NaN key. `pairs` yields neither: a nil key
                // cannot be stored, and a NaN one never compares equal to itself so it could
                // not have been either.
                let _ = out.set(
                    ctx,
                    into_lua_seen(ctx, &key, seen),
                    into_lua_seen(ctx, &value, seen),
                );
            }
            // **The metatable crosses too, or three headline surfaces are silently dead.** `sh` is
            // an *empty* table whose whole API is a `__index` that manufactures a command wrapper;
            // `oslo.prompt` and `oslo.theme.styles` are empty tables whose `__newindex` is what
            // files a prompt handler or defines a style. Copying only the entries delivers `{}` to
            // the VM, and `sh.ls(…)` becomes "attempt to call a nil value" while
            // `oslo.prompt.left = f` succeeds at doing nothing.
            let meta = table.borrow().metatable.clone();
            if let Some(meta) = meta
                && let Value::Table(meta) = into_lua_seen(ctx, &Own::Table(meta), seen)
            {
                out.set_metatable(ctx, Some(meta));
            }
            Value::Table(out)
        }
        Own::Function(f) => {
            let id = Rc::as_ptr(f) as *const () as usize;
            if let Some(already) = seen.get(&id) {
                return *already;
            }
            let out = function_into_lua(ctx, f);
            seen.insert(id, out);
            out
        }
    }
}

/// A callable crossing into the VM: a Rust one gets wrapped, a Lua one gets handed back.
fn function_into_lua<'gc>(ctx: Context<'gc>, f: &Rc<Function>) -> Value<'gc> {
    let Function::Held(held) = &**f else {
        // `Native { name }` carries no body — it exists so a value can *say* it is a function
        // without an engine in scope. Nothing registers one, so nothing can call one.
        return Value::Nil;
    };
    if let Some(stashed) = held.downcast_ref::<StashedFunction>() {
        return Value::Function(ctx.fetch(stashed));
    }
    match held.clone().downcast::<Native>() {
        Ok(native) => Value::Function(LunaFn::Callback(wrap(ctx, native))),
        Err(_) => Value::Nil,
    }
}

/// A Rust function as a luna callback.
///
/// The closure captures the `Rc` itself rather than an index into a registry, which is what
/// [`Callback::from_fn`] permits and what keeps the native's lifetime tied to the value holding it.
fn wrap<'gc>(ctx: Context<'gc>, native: Rc<Native>) -> Callback<'gc> {
    Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
        let args: Vec<Own> = stack.drain(..).map(|v| from_lua(ctx, v)).collect();
        let host = CallbackHost::new(ctx);
        // Published for the duration of the call, so shell code several Rust frames down can call
        // back into Lua — a registered tool, a registered builtin, a hook fired mid-command.
        match crate::host::while_running(&host, || (native.call)(&host, args)) {
            Ok(values) => {
                // `Variadic`, or a `Vec` would arrive as one table: returning two values and
                // returning a list of two are different things, and every multi-return binding
                // depends on the difference.
                let out: Vec<Value<'gc>> = values.iter().map(|v| into_lua(ctx, v)).collect();
                stack.replace(ctx, Variadic(out));
                Ok(CallbackReturn::Return)
            }
            Err(error) => Err(raise(ctx, &error)),
        }
    })
}

/// A shell error as the value luna unwinds with.
///
/// Rendered rather than boxed: `pcall` hands the error *value* to the script, and the idiom for
/// reading one is `message:match(":(%d+):")`. A Rust error object would arrive as userdata the
/// script cannot match on, so what travels is the same `chunk:line: message` string the tree
/// walker produced.
pub fn raise<'gc>(ctx: Context<'gc>, error: &LuaError) -> luna::Error<'gc> {
    // An exit is not a failure, and the status cannot ride in a string. It waits beside the VM
    // while the error unwinds, and the engine reports it instead of the message.
    if let Some(status) = error.exit {
        crate::note_exit(status);
    }
    let text = error.value_string(error.chunk.as_deref().unwrap_or("?"));
    luna::Error::from_value(Value::String(LunaStr::from_slice(&ctx, text.as_bytes())))
}

/// The VM's value, as something the shell can keep.
///
/// **Key order is not preserved.** oslo's table remembers the order keys were inserted, which is
/// what makes `to json` and `to text` print a row's columns the way the document had them.
/// luna's hash part has no order to give back — it is `ahash`, seeded per process, so two runs
/// disagree. A row that goes into Lua and comes back out therefore loses its column order. That
/// is a real gap for the structured pipeline and is recorded in luna's `plans/oslo_requirement.md`;
/// the array part
/// (`1..n`) is unaffected, because it is a vector.
pub fn from_lua<'gc>(ctx: Context<'gc>, value: Value<'gc>) -> Own {
    from_lua_within(ctx, value, &mut std::collections::HashMap::new())
}

/// Tables already brought back, so a cycle stays a cycle rather than becoming infinite.
///
/// **A visited set rather than a depth cap.** `t.self = t` is legal Lua and a script can write one
/// in a line; a cap would answer a 64-deep tree with `nil` at the bottom, which is a *different*
/// value that no longer contains itself — and oslo's JSON encoder, whose job is to refuse a cyclic
/// table by name, would see nothing to refuse and encode the truncation instead.
type Brought<'gc> = std::collections::HashMap<Table<'gc>, Own>;

/// A table brought across but not yet filled in: the VM's, and the shell's waiting for its entries.
type Pending<'gc> = Vec<(Table<'gc>, Rc<RefCell<oslo_base::value::Table>>)>;

/// **Iterative, because the depth is a script's to choose and the stack is not.**
///
/// This used to recurse once per level, so a table nested about fifteen thousand deep — which
/// `for i = 1, 20000 do c.n = {}; c = c.n end` writes in a line — exhausted the interpreter's
/// 16 MiB stack and aborted the process. Not an error a shell can report: the session is simply
/// gone, and it happened on the way *in* to any Rust-implemented `oslo.*` function, whatever that
/// function did with the argument. `oslo.quote`, which only wants a string, died the same way as
/// `oslo.json.encode`.
///
/// The shape is otherwise unchanged, and deliberately so — see [`Brought`]. Each table's `Rc` is
/// still made and recorded *before* its entries are walked, which is what makes a cycle find
/// itself; the only difference is that the entries are walked from a queue rather than from the
/// call stack, so nothing here is bounded by frames. Nothing is truncated and no depth is refused.
fn from_lua_within<'gc>(ctx: Context<'gc>, value: Value<'gc>, seen: &mut Brought<'gc>) -> Own {
    let mut waiting: Pending<'gc> = Vec::new();
    let root = shallow(ctx, value, seen, &mut waiting);

    // Filling one table can discover more, which go on the end of the same queue. A table is
    // pushed only when it is first seen, so this ends after one visit each.
    while let Some((from, into)) = waiting.pop() {
        for (key, item) in from.iter(ctx) {
            let key = shallow(ctx, key, seen, &mut waiting);
            let item = shallow(ctx, item, seen, &mut waiting);
            into.borrow_mut().set(key, item);
        }
        if let Some(meta) = from.metatable()
            && let Own::Table(meta) = shallow(ctx, Value::Table(meta), seen, &mut waiting)
        {
            into.borrow_mut().metatable = Some(meta);
        }
    }
    root
}

/// One value across, queuing a table's contents rather than following them.
fn shallow<'gc>(
    ctx: Context<'gc>,
    value: Value<'gc>,
    seen: &mut Brought<'gc>,
    waiting: &mut Pending<'gc>,
) -> Own {
    match value {
        Value::Nil => Own::Nil,
        Value::Boolean(b) => Own::Bool(b),
        Value::Integer(i) => Own::Number(Number::Int(i)),
        Value::Number(f) => Own::Number(Number::Float(f)),
        // **`Own::bytes`, not `from_utf8_lossy`.** A Lua string is a byte string, and coming out
        // through a lossy conversion meant every byte the VM held that was not text became `U+FFFD`
        // on the way to the shell — `string.pack` was unusable and a file read through the VM came
        // back changed. `Own::bytes` keeps text as text and everything else as itself.
        Value::String(s) => Own::bytes(s.as_bytes()),
        // Rooted in the VM's registry, so the collector keeps it while the shell holds it. This is
        // how a hook, a completer or a prompt handler survives past the call that installed it.
        Value::Function(f) => Own::Function(Rc::new(Function::Held(Rc::new(ctx.stash(f))))),
        Value::Table(t) => {
            if let Some(already) = seen.get(&t) {
                return already.clone();
            }
            // The `Rc` is made and recorded before the entries are walked, so an entry that points
            // back at this table finds it rather than starting again.
            let out = Rc::new(RefCell::new(oslo_base::value::Table::new()));
            seen.insert(t, Own::Table(Rc::clone(&out)));
            waiting.push((t, Rc::clone(&out)));
            Own::Table(out)
        }
        // A thread, or userdata: nothing the shell's value type has a home for.
        _ => Own::Nil,
    }
}
