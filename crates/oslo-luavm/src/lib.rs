//! The Lua VM, behind the same front door as the evaluator it replaces.
//!
//! `luna` is a stackless bytecode VM with a tracing collector, in pure Rust. Against the tree walker
//! it replaces that is coroutines, `goto`, real string patterns, byte-exact strings, collected
//! cycles and unbounded recursion — measured at 15–30× the speed — with no C anywhere, so a static
//! musl build still needs nothing installed.
//!
//! # The one thing that shapes everything above this
//!
//! luna's values carry a garbage-collector lifetime: `Value<'gc>` exists only inside
//! `lua.enter(|ctx| …)`, and holding one across calls means stashing it in the VM's registry. That
//! is the opposite of [`oslo_base::value::Value`], which is an `Rc` anyone can keep. So the boundary
//! is real and it lives here: oslo's own value type stays the interchange currency for the ~40 files
//! that have nothing to do with Lua — the structured pipeline, settings, hooks — and is converted at
//! the edge, once, rather than infecting them with a lifetime.
//!
//! [`Engine`] is what the shell holds. It owns the arena, so every method that touches Lua borrows
//! it mutably, and a native cannot re-enter — see [`host`].

pub mod convert;
pub mod globals;
pub mod host;

pub use globals::Globals;
pub use host::{Host, Native, NativeFn};

use luna::{Closure, Executor, Lua, Value, Variadic};
use oslo_base::value::{LuaError, LuaResult, Value as Own};
use std::cell::RefCell;

thread_local! {
    /// The status an `oslo.proc.exit(n)` asked for, kept aside while its error unwinds.
    ///
    /// **Because the exit has to survive being turned into a string.** An error crosses into the VM
    /// as the *value* `pcall` hands to a script, which is what makes `message:match(":(%d+):")`
    /// work — and a string cannot carry the status. So the status waits here and the engine picks
    /// it up when the call it was raised in comes back.
    static PENDING_EXIT: RefCell<Option<i32>> = const { RefCell::new(None) };
}

/// Record that an exit was requested, so the engine can report it rather than the message.
pub(crate) fn note_exit(status: i32) {
    PENDING_EXIT.with(|slot| *slot.borrow_mut() = Some(status));
}

/// Take the pending exit, if one was requested during the last call.
fn taken_exit() -> Option<i32> {
    PENDING_EXIT.with(|slot| slot.borrow_mut().take())
}

/// An exit asked for during a call wins, whatever the call itself answered.
///
/// **`pcall` must not be able to swallow an exit.** The request unwinds as an ordinary string error
/// so a script can read it, which means `pcall(function() os.exit(7) end)` caught it and carried on
/// — and the status stayed in the thread-local, waiting. The next unrelated failure then picked it
/// up in [`Engine::failure`], so `error("something else")` after a caught exit reported *exit 7*,
/// silently, with the real error nowhere: a genuine failure turned into a fabricated status.
///
/// Consulted on the way out of every entry point, so the slot is never left holding one. Lua's own
/// `os.exit` cannot be caught either.
fn settled(outcome: LuaResult<Vec<Own>>) -> LuaResult<Vec<Own>> {
    match taken_exit() {
        Some(status) => Err(LuaError::exit_request(status)),
        None => outcome,
    }
}

/// What every entry point answers when the VM is busy and no callback is in progress.
///
/// Reachable only if something outside a callback held the arena — which nothing does — so it
/// exists to be an error rather than a panic if that ever stops being true.
fn busy() -> LuaError {
    LuaError::new("the Lua engine is busy and no call is in progress")
}

/// Say "syntax error" for a chunk that would not compile, which is what Lua calls it.
///
/// The VM reports its own two stages — `parse error` and `compile error` — and both mean the same
/// thing to somebody who has just mistyped a line: the source is not Lua. That is the word the
/// language uses, the word the shell's own diagnostics use for the other language it speaks, and
/// the word somebody searches for. The VM's wording is kept after it, because it is the half that
/// says *where*.
fn syntax_wording(message: &str) -> String {
    for stage in ["parse error: ", "compile error: "] {
        let Some(at) = message.find(stage) else {
            continue;
        };
        // The VM raises a failed compile through its runtime channel, so the rendered message
        // arrives as `runtime error: parse error: …`. A chunk that never ran did not fail at run
        // time, and saying both is worse than saying the right one.
        let before = message[..at].trim_end_matches("runtime error: ");
        return format!("{before}syntax error: {}", &message[at + stage.len()..]);
    }
    message.to_string()
}

/// Take the VM's own category word off the front of a failure.
///
/// **Lua names no stage in front of a position.** `lua: stdin:1: attempt to call a nil value` is
/// the whole of what the reference interpreter prints; the VM instead renders `runtime error: …`
/// for a fault and `lua error: …` for a value raised by `error()`. Kept, they stacked up: one
/// mistyped line at the prompt answered
///
/// ```text
/// oslo: Lua error: (oslo lua): runtime error: (oslo lua):1: could not call a nil value
/// ```
///
/// — three labels and the chunk twice, for one nil. The stage is also the half a reader can do
/// nothing with: the position and the message are what say where to look.
fn plain_wording(message: &str) -> String {
    for category in ["runtime error: ", "lua error: "] {
        if let Some(rest) = message.strip_prefix(category) {
            return rest.to_string();
        }
    }
    message.to_string()
}

/// A Lua interpreter, and the shell's whole view of one.
pub struct Engine {
    lua: RefCell<Lua>,
    /// What the running source is called, for the position on an error.
    chunk: RefCell<String>,
    /// What the next chunk sees as `...`.
    varargs: RefCell<Vec<Own>>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            lua: RefCell::new(Lua::full()),
            chunk: RefCell::new("?".to_string()),
            varargs: RefCell::new(Vec::new()),
        }
    }

    /// Name the source that is about to run, for error positions.
    pub fn set_chunk(&self, name: &str) {
        *self.chunk.borrow_mut() = name.to_string();
    }

    /// Send globals Lua does not have to the shell's variables. See [`globals`].
    pub fn set_globals_host(&self, host: std::rc::Rc<dyn Globals>) {
        let mut lua = self.lua.borrow_mut();
        lua.enter(|ctx| globals::attach(ctx, host));
    }

    /// What a chunk sees as `...`.
    pub fn set_varargs(&self, args: Vec<Own>) {
        *self.varargs.borrow_mut() = args;
    }

    /// Lift the members of the global `table` to globals in their own right, so `oslo.fs` is `fs`.
    ///
    /// Three rules, and each one exists to avoid taking something away:
    ///
    /// * **A name already in `_G` is never replaced.** `math` is Lua's, and a session where
    ///   `math.floor` had quietly become something else would be worse than one where `math.eval`
    ///   simply did not exist.
    /// * **Two tables under one name are merged**, missing keys only. This is what makes `math`
    ///   work: Lua's `floor`, `pi` and `sqrt` stay exactly where they were and oslo's `eval`,
    ///   `convert` and `session` join them. The two share no key, which is checked by the test
    ///   rather than assumed.
    /// * **Only tables and functions are lifted.** A bare `version` global holding a string is
    ///   noise; a namespace or something callable is what the shorthand is for.
    ///
    /// Set **raw**, deliberately: `_G`'s `__newindex` sends values beside the table rather than
    /// into it (see [`globals`]), and a name that is not really in `_G` is invisible to
    /// `pairs(_G)`, to `rawget`, and to the completion that reads them.
    pub fn flatten_namespace(&self, table: &str) {
        let Ok(mut lua) = self.lua.try_borrow_mut() else {
            return;
        };
        lua.enter(|ctx| {
            let globals = ctx.globals();
            let key = Value::String(luna::String::from_slice(&ctx, table.as_bytes()));
            let Value::Table(source) = globals.get_value(ctx, key) else {
                return;
            };
            for (name, value) in source.iter(ctx).collect::<Vec<_>>() {
                let Value::String(_) = name else { continue };
                if !matches!(value, Value::Table(_) | Value::Function(_)) {
                    continue;
                }
                match globals.get_value(ctx, name) {
                    // Free: take it.
                    Value::Nil => {
                        let _ = globals.set(ctx, name, value);
                    }
                    // Taken by a table, and this is one too: fill in what it does not have.
                    Value::Table(existing) => {
                        let Value::Table(adding) = value else {
                            continue;
                        };
                        for (key, value) in adding.iter(ctx).collect::<Vec<_>>() {
                            if matches!(existing.get_value(ctx, key), Value::Nil) {
                                let _ = existing.set(ctx, key, value);
                            }
                        }
                    }
                    // Taken by something that is not a table. Left alone.
                    _ => {}
                }
            }
        });
    }

    /// The string keys of the table reached by `path`, each with whether it is callable.
    ///
    /// `[]` answers the globals. This is what completion needs and it is deliberately *not*
    /// [`Host::global`]: that converts the value it finds into the shell's own type, which for
    /// `_G` means deep-copying every table in the session — and `_G._G` means doing it forever.
    /// Here nothing leaves the VM but names.
    ///
    /// **Through the metatable as well**, because oslo's globals are reached that way: `_G` has an
    /// `__index` and the names behind it are exactly the ones a bare `fs` or `math` resolves to. A
    /// list that skipped them would omit the names this exists to offer. Only one level of it is
    /// followed and a table is never visited twice, so a cyclic `__index` cannot spin.
    pub fn names_at(&self, path: &[String]) -> Vec<(String, bool)> {
        let Ok(mut lua) = self.lua.try_borrow_mut() else {
            return Vec::new();
        };
        lua.enter(|ctx| {
            let mut table = ctx.globals();
            for step in path {
                let key = Value::String(luna::String::from_slice(&ctx, step.as_bytes()));
                match table.get_value(ctx, key) {
                    Value::Table(next) => table = next,
                    _ => return Vec::new(),
                }
            }
            let mut out = Vec::new();
            let mut current = Some(table);
            // A depth cap rather than a visited-set: an `__index` chain that loops is bounded by
            // this, and four links is already deeper than any real one.
            let mut depth = 0;
            while let Some(table) = current.take() {
                if depth > 4 {
                    break;
                }
                depth += 1;
                for (key, value) in table.iter(ctx) {
                    let Value::String(name) = key else { continue };
                    let Ok(name) = std::str::from_utf8(name.as_bytes()) else {
                        continue;
                    };
                    out.push((name.to_string(), matches!(value, Value::Function(_))));
                }
                current = table
                    .metatable()
                    .map(|meta| {
                        let key = Value::String(luna::String::from_slice(&ctx, b"__index"));
                        meta.get_value(ctx, key)
                    })
                    .and_then(|index| match index {
                        Value::Table(next) => Some(next),
                        _ => None,
                    });
            }
            out
        })
    }

    /// How many bytes the VM's heap is holding, or `None` while it is running.
    ///
    /// Only a *change* in this is worth reading. The absolute figure counts every string, table and
    /// closure the session has accumulated, which says more about how long the shell has been up
    /// than about whatever is being measured.
    pub fn memory_used(&self) -> Option<usize> {
        self.lua.try_borrow().map(|lua| lua.total_memory()).ok()
    }

    /// Stop whatever is running once the VM's heap passes `limit` bytes. `None` removes it.
    ///
    /// **Coarse on purpose, and only useful around something with an end.** The check happens
    /// between slices of the executor, so a single slice can overshoot, and the limit is on the
    /// whole VM rather than on one chunk — there is one heap and everything shares it. What it is
    /// for is bounding a chunk whose allocation could run away, by setting the ceiling to what is
    /// in use plus a margin and taking it off again afterwards. See `plugin::load`.
    ///
    /// Answers whether it was applied; a VM that is already running cannot be reconfigured.
    pub fn set_memory_limit(&self, limit: Option<usize>) -> bool {
        match self.lua.try_borrow_mut() {
            Ok(mut lua) => {
                lua.set_memory_limit(limit);
                true
            }
            Err(_) => false,
        }
    }

    /// Compile and run `source`, answering what it returned.
    ///
    /// Re-entrant, like [`call_function`](Self::call_function): a chunk compiled while the VM is
    /// already running goes out through the callback in progress.
    pub fn eval(&self, source: &str, chunk: &str) -> LuaResult<Vec<Own>> {
        self.set_chunk(chunk);
        let varargs = self.varargs.borrow().clone();
        let Ok(mut lua) = self.lua.try_borrow_mut() else {
            return host::reentrant_eval(source, chunk).unwrap_or_else(|| Err(busy()));
        };
        let executor = lua
            .try_enter(|ctx| {
                let closure = Closure::load(ctx, Some(chunk), source.as_bytes())?;
                let given: Vec<Value> = varargs.iter().map(|v| convert::into_lua(ctx, v)).collect();
                Ok(ctx.stash(Executor::start(ctx, closure.into(), Variadic(given))))
            })
            .map_err(|e| self.failure(e))?;
        settled(Self::finish(&mut lua, &executor).map_err(|e| self.failure(e)))
    }

    /// Compile `source` without running it, answering the chunk as a callable value.
    ///
    /// **So an expression can be compiled once and run per row.** `where free < 1e9` over a few
    /// hundred rows must not recompile the same six characters each time; the caller keeps what
    /// this returns and hands it to [`call_function`](Self::call_function) in the loop.
    /// Re-entrant, for the same reason [`eval`](Self::eval) is: `where` and `each` compile their
    /// expression through here, and a structured pipeline can be started from inside a native.
    pub fn load(&self, source: &str, chunk: &str) -> LuaResult<Own> {
        let Ok(mut lua) = self.lua.try_borrow_mut() else {
            return host::reentrant_load(source, chunk).unwrap_or_else(|| Err(busy()));
        };
        lua.try_enter(|ctx| {
            let closure = Closure::load(ctx, Some(chunk), source.as_bytes())?;
            Ok(convert::from_lua(
                ctx,
                Value::Function(luna::Function::Closure(closure)),
            ))
        })
        .map_err(|e| self.failure(e))
    }

    /// Call a Lua function the shell is holding.
    ///
    /// **Re-entrant.** When the VM is already running — a native that started a shell command, and
    /// the command reached a tool a config wrote in Lua — the arena cannot be borrowed again, so
    /// the call goes out through the callback that is running instead. See [`host::reentrant`].
    pub fn call_function(&self, function: &Own, args: Vec<Own>) -> LuaResult<Vec<Own>> {
        let Ok(mut lua) = self.lua.try_borrow_mut() else {
            return host::reentrant(function, args).unwrap_or_else(|| Err(busy()));
        };
        let executor = lua
            .try_enter(|ctx| {
                let Value::Function(f) = convert::into_lua(ctx, function) else {
                    return Err(convert::raise(
                        ctx,
                        &LuaError::new("attempt to call a value that is not a function"),
                    ));
                };
                let given: Vec<Value> = args.iter().map(|v| convert::into_lua(ctx, v)).collect();
                Ok(ctx.stash(Executor::start(ctx, f, Variadic(given))))
            })
            .map_err(|e| self.failure(e))?;
        settled(Self::finish(&mut lua, &executor).map_err(|e| self.failure(e)))
    }

    /// Drive an executor to the end and bring its results back out of the arena.
    ///
    /// The conversion happens *inside* `try_enter` because the results are `Value<'gc>` and cannot
    /// leave it — which is the boundary this crate exists to hold.
    fn finish(
        lua: &mut Lua,
        executor: &luna::StashedExecutor,
    ) -> Result<Vec<Own>, luna::ExternError> {
        lua.finish(executor)
            .map_err(|e| luna::ExternError::from(luna::RuntimeError::new(e)))?;
        lua.try_enter(|ctx| {
            let Variadic(values): Variadic<Vec<Value>> =
                ctx.fetch(executor).take_result(ctx).map_err(|e| {
                    luna::Error::Runtime(luna::RuntimeError::new(std::io::Error::other(
                        e.to_string(),
                    )))
                })??;
            Ok(values
                .into_iter()
                .map(|v| convert::from_lua(ctx, v))
                .collect())
        })
    }

    /// A VM failure as the shell's error, with an exit request recovered if that is what it was.
    fn failure(&self, error: luna::ExternError) -> LuaError {
        if let Some(status) = taken_exit() {
            return LuaError::exit_request(status);
        }
        let message = plain_wording(&syntax_wording(&error.to_string()));
        let chunk = self.chunk.borrow().clone();
        // The VM has already put `chunk:line:` in front where it knows one, so the message is
        // taken whole rather than positioned again — which is what this function always meant to
        // do, and what naming the chunk a second time undid.
        if message.starts_with(&format!("{chunk}:")) {
            return LuaError::without_position(message);
        }
        LuaError::new(message).in_chunk(chunk)
    }
}

impl Host for Engine {
    fn flatten_namespace(&self, table: &str) {
        Engine::flatten_namespace(self, table);
    }

    fn global(&self, name: &str) -> Own {
        let Ok(mut lua) = self.lua.try_borrow_mut() else {
            return host::with_running(|host| host.global(name)).unwrap_or(Own::Nil);
        };
        lua.enter(|ctx| {
            let key = Value::String(luna::String::from_slice(&ctx, name.as_bytes()));
            convert::from_lua(ctx, ctx.globals().get_value(ctx, key))
        })
    }

    fn set_global(&self, name: &str, value: Own) {
        let Ok(mut lua) = self.lua.try_borrow_mut() else {
            host::with_running(|host| host.set_global(name, value));
            return;
        };
        lua.enter(|ctx| {
            let key = Value::String(luna::String::from_slice(&ctx, name.as_bytes()));
            let _ = ctx.globals().set(ctx, key, convert::into_lua(ctx, &value));
        });
    }

    fn set_field(&self, path: &[&str], value: Own) -> bool {
        let Ok(mut lua) = self.lua.try_borrow_mut() else {
            return host::with_running(|host| host.set_field(path, value)).unwrap_or(false);
        };
        lua.enter(|ctx| host::set_field_in(ctx, path, &value))
    }

    fn field(&self, path: &[&str]) -> Own {
        let Ok(mut lua) = self.lua.try_borrow_mut() else {
            return host::with_running(|host| host.field(path)).unwrap_or(Own::Nil);
        };
        lua.enter(|ctx| host::field_in(ctx, path))
    }

    fn chunk(&self) -> String {
        self.chunk.borrow().clone()
    }

    fn call(&self, function: &Own, args: Vec<Own>) -> LuaResult<Vec<Own>> {
        self.call_function(function, args)
    }

    fn eval(&self, source: &str, chunk: &str) -> LuaResult<Vec<Own>> {
        Engine::eval(self, source, chunk)
    }

    fn load(&self, source: &str, chunk: &str) -> LuaResult<Own> {
        Engine::load(self, source, chunk)
    }
}

/// Whether `source` is a whole chunk, or was cut off part way through one.
///
/// This is what decides between running the line and asking for another: `if true then` is not an
/// error, it is a beginning. The VM distinguishes the two itself — a chunk that ends mid-construct
/// fails with *end of token stream*, and anything else is a real mistake that more input cannot
/// repair, because `x = = 2` never becomes valid however much follows it.
///
/// A bare VM rather than the session's, so an unfinished line cannot run a metamethod or see a
/// half-built global. It costs an arena per keystroke that ends a line, which is nothing against a
/// terminal round trip.
///
/// **Asked of the statement only, deliberately.** The prompt now tries a line as an expression
/// before it tries it as a chunk, so `1 +` is an unfinished *expression* and a REPL that knew it
/// would ask for the rest of the sum. Asking the parser that question is what cannot be done here:
/// luna reports `EndOfStream` both when it ran out of tokens *and* when a complete block was
/// followed by tokens it did not want (`parse_chunk`, where a leftover `end` is "expected the end
/// of the stream"). `x = = 2` lands in the second case as an expression, so believing it would
/// leave the prompt waiting for a line that can never finish it — and a prompt that will not come
/// back is a worse answer than a syntax error. So `1 +` is reported rather than continued.
pub fn is_complete(source: &str) -> bool {
    use luna::compiler::{ParseError, ParseErrorKind};

    let mut lua = Lua::core();
    lua.enter(|ctx| match Closure::load(ctx, None, source.as_bytes()) {
        Ok(_) => true,
        // The variant rather than the message: a rendered string is a thing that changes when
        // someone improves the wording, and this decides whether the shell hangs waiting.
        Err(luna::CompilerError::Parsing(ParseError {
            kind: ParseErrorKind::EndOfStream { .. },
            ..
        })) => false,
        Err(_) => true,
    })
}

/// The session's engine, reachable from anywhere on its thread.
///
/// **Because the things that call Lua are not the thing that owns Lua.** A completer, a timer, a
/// hook and the `where` filter each need to call a function a config registered, and none of them
/// is handed the engine — threading one through every call site would put a Lua parameter on code
/// that has nothing else to do with Lua. It is a thread-local rather than a global because the
/// engine is `Rc` and single-threaded by construction.
pub mod current {
    use super::Engine;
    use std::cell::RefCell;
    use std::rc::Rc;

    thread_local! {
        static CURRENT: RefCell<Option<Rc<Engine>>> = const { RefCell::new(None) };
    }

    /// Make `engine` the one this thread reaches, or clear it.
    pub fn publish(engine: Option<Rc<Engine>>) {
        CURRENT.with(|slot| *slot.borrow_mut() = engine);
    }

    /// The session's engine, if one has been published.
    pub fn handle() -> Option<Rc<Engine>> {
        CURRENT.with(|slot| slot.borrow().clone())
    }
}

/// Run `source` and answer the exit status, reporting an error the way the shell would.
pub fn run(source: &str, chunk_name: &str) -> i32 {
    let engine = Engine::new();
    match engine.eval(source, chunk_name) {
        Ok(_) => 0,
        Err(error) => match error.exit {
            Some(status) => status,
            None => {
                eprintln!("oslo: lua: {error}");
                1
            }
        },
    }
}

#[cfg(test)]
mod deep_tests;
#[cfg(test)]
mod tests;
