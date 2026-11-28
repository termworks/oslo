//! Owning the Lua interpreter, and the bridge back from the shell into it.

use crate::lua::context::Context;
use oslo_base::error::{Result, ShellError};
use oslo_base::value::{LuaError, LuaResult, Table, Value};
use oslo_luavm::{Engine, Host};
use oslo_shell::env::Environment;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// Values the host holds on a script's behalf.
///
/// The prompt function and every `oslo.register_builtin` callback live here rather than in a Lua
/// global, so a script cannot overwrite one by choosing an unlucky variable name — and so that
/// `pairs(_G)` does not walk the shell's internals.
pub(crate) type Registry = Rc<RefCell<HashMap<String, Value>>>;

/// Registry key under which `oslo.ui.prompt` stores its function.
pub(crate) const PROMPT_KEY: &str = "prompt";

/// Prefix for the registry keys holding builtin callbacks.
pub(crate) const BUILTIN_KEY_PREFIX: &str = "builtin:";

thread_local! {
    /// The interpreter and registry a builtin registered on this thread should call back into.
    ///
    /// Per-thread because the interpreter is: a builtin invoked from a thread that never ran
    /// [`LuaEngine::setup_bindings`] finds nothing here and says so, rather than reaching into
    /// another thread's state.
    static ACTIVE: RefCell<Option<(Rc<Engine>, Registry)>> = const { RefCell::new(None) };

    /// The shell state the `oslo.*` API reaches, kept so that code far from it can ask whether it
    /// is currently *held*.
    ///
    /// A hook fires from places that have no `Environment` to offer and no way to get one — the
    /// line editor, `attempt_directory`, the job reaper. Whether a handler could act depends
    /// entirely on whether this mutex is free at that moment, and this is what lets the question be
    /// asked there. See [`state_is_held`] and `queue`.
    static SHELL_STATE: RefCell<Option<Arc<Mutex<Environment>>>> = const { RefCell::new(None) };
}

/// Whether the shell is in the middle of something that has its state locked.
///
/// **The question a hook fire site has to ask.** `attempt_directory` runs with the `Environment`
/// locked and the line editor does not, so the same `fire_at_here` call is safe to run inline from
/// one and useless from the other. `false` when there is no interpreter on this thread, which is
/// the non-interactive case where nothing can be attached anyway.
///
/// One uncontended `try_lock`. The shell is single-threaded through this path, so the answer cannot
/// go stale between here and the caller acting on it.
pub(crate) fn state_is_held() -> bool {
    SHELL_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|env| env.try_lock().is_err())
    })
}

/// The interpreter on this thread, if there is one, as a handle that can outlive the borrow.
pub fn interpreter_handle() -> Option<Rc<Engine>> {
    // Published into `oslo_luavm::current` as well, for callers below this crate. See there.
    ACTIVE.with(|slot| slot.borrow().as_ref().map(|(interp, _)| Rc::clone(interp)))
}

/// A value the host is holding under `key`, from the registry on this thread.
///
/// The reach-back `oslo.feature.when` needs: the predicate is registered while the config runs and
/// called from the read loop, which has no route back to the table it was stored in. `None` when
/// there is no interpreter here — a script, a non-interactive shell — which is also every case
/// where no config could have registered one.
pub fn host_value(key: &str) -> Option<Value> {
    let (_, registry) = ACTIVE.with(|slot| slot.borrow().clone())?;
    // Cloned out rather than handed back by reference: the caller runs the value it gets, and a
    // predicate that registered another one would meet its own outstanding borrow.
    registry.borrow().get(key).cloned()
}

/// Call a Lua value with whatever interpreter is on this thread.
///
/// The same reach-back `ask_hook_here` uses, for callers that hold a function rather than a hook
/// name — a key binding written in the config, for instance, which the editor holds directly.
pub fn call_here(f: &Value, args: Vec<Value>) -> LuaResult<Vec<Value>> {
    let Some((interp, _)) = ACTIVE.with(|slot| slot.borrow().clone()) else {
        return Err(LuaError::new(
            "no Lua interpreter on this thread".to_string(),
        ));
    };
    interp.call_function(f, args)
}

/// Taking and releasing the shell state, which is also where deferred hooks are drained.
mod borrow;
/// The hook reach-backs, which every fire site outside this file uses.
mod hooks;
#[path = "engine/installs.rs"]
mod installs;
/// Running a plugin's entry file on this session's interpreter.
#[cfg(feature = "plugin")]
mod plugin;
/// Hooks held until the shell can act on them. See that module for why.
mod queue;
pub(crate) use borrow::borrow_env;
pub use hooks::{
    answer_hook_here, answer_hook_with, ask_hook_here, fire_at_here, key_hook_here,
    key_hook_watched,
};
#[cfg(feature = "plugin")]
pub(crate) use plugin::load_plugin_file;
pub use queue::drain as run_deferred_hooks;

/// The interpreter and registry active on this thread, if a config was loaded.
///
/// `call_lua_builtin` used to live here and reach these directly; the builtin's call path moved to
/// `api/builtin/call.rs` when it grew the shell record, and this is the seam it needs.
pub(crate) fn active() -> Option<(Rc<Engine>, Registry)> {
    ACTIVE.with(|slot| slot.borrow().clone())
}

/// The shell's variables, as the evaluator's global namespace.
///
/// This is what makes `export one=two` in shell readable as `one` in Lua on the next line, and
/// `name = "world"` in Lua readable as `$name` in shell. One namespace, two spellings.
///
/// A busy lock is answered as "not set" rather than by blocking. The only way to reach here with
/// the mutex held is from inside a Lua-registered builtin, which is already running *because* the
/// evaluator holds it — blocking would be a deadlock with no output at all, where a nil is a
/// wrong answer the reader can see.
struct ShellGlobals {
    env: Arc<Mutex<Environment>>,
}

impl oslo_luavm::Globals for ShellGlobals {
    fn get(&self, name: &str) -> Option<String> {
        self.env.try_lock().ok()?.get_param(name)
    }

    fn set(&self, name: &str, value: &str) {
        if let Ok(mut env) = self.env.try_lock()
            && env.set_var(name, value, true)
        {
            env.mark_if_handed_out(name, value);
        }
    }

    fn unset(&self, name: &str) {
        if let Ok(mut env) = self.env.try_lock() {
            env.unset_var(name);
        }
    }
}

#[derive(Clone)]
pub struct LuaEngine {
    /// `Rc` because the shell reaches back in through [`ACTIVE`] while a call is still running.
    interp: Rc<Engine>,
    registry: Registry,
    /// Arguments passed to the chunk as `...`; see [`LuaEngine::set_script_args`].
    script_args: RefCell<Vec<Value>>,
}

impl Default for LuaEngine {
    fn default() -> Self {
        Self::new().expect("Failed to initialize Lua engine")
    }
}

impl LuaEngine {
    pub fn new() -> Result<Self> {
        // **The moment the shell gains a way to reach a hook.** Everything below this layer fires
        // through `crate::hooks`, which knows the moments and nothing else; this is where the four
        // functions that can actually run one are handed over. Before it — a script, `sh -c`, a
        // test that never builds an engine — every hook is a relaxed load and a return.
        oslo_base::hooks::install(oslo_base::hooks::Dispatch {
            watched: crate::lua::api::hooks::watched,
            fire: hooks::fire_at_here,
            answer: hooks::answer_hook_with,
            ask: hooks::ask_hook_here,
            emit: hooks::emit_user_here,
        });
        Ok(Self {
            interp: Rc::new(Engine::new()),
            registry: Rc::new(RefCell::new(HashMap::new())),
            script_args: RefCell::new(Vec::new()),
        })
    }

    pub fn setup_bindings(&self, env: Arc<Mutex<Environment>>) -> Result<()> {
        // Published before anything can be registered, so a builtin registered by the very
        // script that is being loaded can already find its way back here.
        ACTIVE.with(|slot| {
            *slot.borrow_mut() = Some((Rc::clone(&self.interp), Rc::clone(&self.registry)))
        });
        // The same interpreter, published where code *below* this crate can reach it. A `where`
        // filter in a structured pipeline wants the config's interpreter so a predicate can call a
        // function the config defined, and the pipeline is part of the shell — it cannot ask
        // upward. See `oslo_luavm::current`.
        oslo_luavm::current::publish(Some(Rc::clone(&self.interp)));
        // The same publication, for the question "can a hook act right now". Kept beside the
        // interpreter because the two are set up and torn down together.
        SHELL_STATE.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&env)));
        self.interp.set_globals_host(Rc::new(ShellGlobals {
            env: Arc::clone(&env),
        }));
        crate::lua::api::install(self.interp.as_ref(), &self.registry, env);
        Ok(())
    }

    pub fn eval_script(&self, script: &str) -> Result<()> {
        self.run(script, "(oslo lua)")
    }

    /// Whether `source` compiles, without running any of it.
    ///
    /// **Compiled and thrown away, never run.** The prompt uses this to tell an expression from a
    /// statement, and the only safe way to ask that question is to parse: running `f()` to find
    /// out whether it was an expression would have called `f`.
    pub fn compiles(&self, source: &str) -> bool {
        self.interp.load(source, "(oslo lua)").is_ok()
    }

    pub fn load_file(&self, path: &str) -> Result<()> {
        let content = std::fs::read_to_string(path)?;
        self.eval_as(&content, path)
    }

    /// Run Lua source under `name`.
    ///
    /// Naming the chunk is what makes an error out of `init.lua` point at the user's script,
    /// and it is also what `pcall` hands the script: `error("x")` on line 12 is caught as
    /// `init.lua:12: x`, which is the form Lua code parses with `message:match(":(%d+):")`.
    pub fn eval_as(&self, source: &str, name: &str) -> Result<()> {
        self.run(source, name)
    }

    /// Publish a script's arguments the way every Lua interpreter does.
    ///
    /// `arg[0]` is the script, `arg[1..n]` its arguments — and the same list becomes the chunk's
    /// `...`. Without this a Lua program could not read its own argv at all: `oslo build.lua
    /// release` gave the script `arg == nil`, which makes Lua a configuration language rather
    /// than a scripting one.
    ///
    /// `arg[-1]` is the interpreter, matching `lua`'s own convention, so a script that
    /// re-executes itself can find the shell it is running under.
    pub fn set_script_args(&self, script: &str, args: &[String]) -> Result<()> {
        let mut table = Table::new();
        table.set(
            Value::int(-1),
            Value::str(
                std::env::current_exe()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "oslo".to_string()),
            ),
        );
        table.set(Value::int(0), Value::str(script));
        for (i, value) in args.iter().enumerate() {
            table.set(Value::int(i as i64 + 1), Value::str(value));
        }
        self.interp.set_global("arg", Value::table(table));
        self.script_args
            .replace(args.iter().map(Value::str).collect());
        Ok(())
    }

    /// Compile hot functions to machine code, answering whether the JIT is on. See
    /// [`oslo_luavm::Engine::set_jit`].
    pub fn set_jit(&self, on: bool) -> bool {
        self.interp.set_jit(on)
    }

    fn run(&self, source: &str, name: &str) -> Result<()> {
        // Both failures are named here: a chunk that does not parse and one that fails part-way
        // through are equally useless as `Lua error: attempt to call a nil value` with no file.
        self.interp.set_varargs(self.script_args.borrow().clone());
        let outcome = self
            .interp
            .eval(source, name)
            .map_err(|e| ShellError::Lua(e.in_chunk(name)));
        // A script is the one place with no loop to drain into. `oslo.run{"cd", d}` at the end of
        // a chunk would otherwise leave its `post-change-dir` queued for a REPL that never comes.
        // After the failure case too: a chunk that raised still ran the commands before it.
        queue::drain();
        outcome.map(|_| ())
    }

    /// Tell a hook something happened, by its index in [`crate::lua::api::hooks::HOOKS`].
    ///
    /// By index rather than by name because a name is a spelling and there are several of each:
    /// `preexec` and `pre-cmd` are one list, and firing "the one the caller typed" would run half
    /// of it. The index names the *moment*, which is the thing a fire site actually has.
    ///
    /// The watched bit is checked first, so a moment nobody attached to costs one relaxed load —
    /// which is what lets this be called from a mode change or a prompt without thinking about it.
    ///
    /// Goes through the same inline-or-deferred decision as every other fire site rather than
    /// calling the handlers directly. These callers are in the REPL loop, where the state is free
    /// and nothing is deferred — but a second dispatch path is how the two would drift apart.
    pub fn fire_at(&self, index: usize, args: Vec<Value>) {
        hooks::fire_values_here(index, args);
    }

    /// A table for a hook, from plain pairs — the shape most of them hand over.
    pub fn hook_fields(fields: &[(&str, Value)]) -> Value {
        let mut t = oslo_base::value::Table::new();
        for (name, value) in fields {
            t.set(Value::str(*name), value.clone());
        }
        Value::table(t)
    }

    /// What `preexec` and `precmd` are handed: the command about to run.
    ///
    /// A table rather than the bare string these used to get. The string was never enough — a hook
    /// that wants to log a command wants to know where it ran, and `oslo.sys.cwd()` asked from
    /// inside the handler answers the same thing only until the command is a `cd`.
    pub fn command_started(text: &str, cwd: &str, mode: &str) -> Value {
        let mut t = oslo_base::value::Table::new();
        t.set_str("text", Value::str(text));
        t.set_str("cwd", Value::str(cwd));
        t.set_str("mode", Value::str(mode));
        // **Parsed only when somebody is listening.** The line is parsed again a moment later by
        // the shell either way, so this is one extra parse per command — worth nothing to a config
        // that asks for it and worth avoiding entirely for the far more common one that does not.
        // Absent rather than empty when the line does not parse; see `lua::parsed`.
        if mode == "sh"
            && oslo_base::hooks::watched(oslo_base::hooks::at::PRE_CMD)
            && let Some(commands) = crate::lua::parsed::commands_of(text)
        {
            t.set_str("commands", commands);
        }
        Value::table(t)
    }

    /// What `postexec` and `postcmd` are handed: the command that just finished.
    ///
    /// `cwd` is where it *ended*, which is what makes the pair useful across a `cd`. `duration_ms`
    /// is the same measurement the slow-command notice and the history's timing column use, so a
    /// hook and the shell cannot disagree about how long something took.
    pub fn command_finished(
        text: &str,
        cwd: &str,
        mode: &str,
        status: i32,
        duration: std::time::Duration,
    ) -> Value {
        let mut t = oslo_base::value::Table::new();
        t.set_str("text", Value::str(text));
        t.set_str("cwd", Value::str(cwd));
        t.set_str("mode", Value::str(mode));
        t.set_str("status", Value::int(status as i64));
        t.set_str("ok", Value::Bool(status == 0));
        t.set(
            Value::str("duration_ms"),
            Value::int(duration.as_millis() as i64),
        );
        Value::table(t)
    }

    /// Render one of the prompts, or `None` when the config set none.
    ///
    /// Three shapes, in the order a config is likely to reach for them: a **string** used as
    /// written, so a prompt that never changes need not be a closure; a **function** called for its
    /// return value; and a **list of segments**, which is the one that can be measured, styled and
    /// degraded piece by piece — see the `segment` module for the shape of one.
    pub fn render(&self, key: &str) -> Option<String> {
        self.render_with(key, &Context::default())
    }

    /// As [`render`](Self::render), with the facts a segment's `render(ctx)` is given.
    pub fn render_with(&self, key: &str, ctx: &Context) -> Option<String> {
        let value = self.registry.borrow().get(key).cloned()?;
        if let Value::Str(text) = &value {
            return Some(text.to_string());
        }
        if crate::lua::api::segment::is_segment_list(&value) {
            return self.render_segments(key, &value, ctx);
        }
        // A prompt produced by another program — starship, hexe, anything that prints one.
        if let Some(spec) = crate::lua::api::external::spec_of(&value) {
            return crate::lua::api::external::render(&spec, ctx);
        }
        // **The facts go in as the argument.** A segment list and an external prompt were handed
        // the context above; a plain function was called with nothing, so `oslo.prompt.left =
        // function(p) return p.cwd end` — the shape the documentation shows and the obvious one to
        // write — saw `p` as nil and raised. Every prompt key is a function of the same facts, so
        // there is no reason for the three shapes to disagree.
        //
        // Zero-argument functions are unaffected: Lua discards arguments a function does not name.
        match self.interp.call_function(&value, vec![ctx.to_lua()]) {
            Ok(values) => match values.first() {
                Some(Value::Str(s)) => Some(s.to_string()),
                Some(Value::Number(n)) => Some(n.to_string()),
                _ => None,
            },
            Err(e) => {
                // Reported rather than swallowed: a prompt function that raises leaves the shell
                // silently drawing its default, which looks exactly like the config not loading.
                oslo_base::messages::error(key, e.to_string());
                None
            }
        }
    }

    /// Read `oslo.theme` as it stands, and what was wrong with it.
    pub fn read_theme(&self) -> (oslo_ui::theme::Theme, Vec<String>) {
        let oslo = self.interp.global("oslo");
        let theme = match &oslo {
            Value::Table(table) => table.borrow().get_str("theme"),
            _ => Value::Nil,
        };
        oslo_ui::theme::read_lua_theme(&theme)
    }

    /// Read `oslo.completion`, `oslo.suggest` and `oslo.history` as they stand.
    /// The `oslo` table as it stands, for a reader that wants more than the settings.
    pub fn oslo_table(&self) -> Value {
        self.interp.global("oslo")
    }

    pub fn read_settings(&self) -> (oslo_ui::settings::Settings, Vec<String>) {
        oslo_ui::settings::read_lua_settings(&self.interp.global("oslo"))
    }

    /// Render a list of segments into one string, dropping the least important until it fits.
    fn render_segments(&self, key: &str, list: &Value, ctx: &Context) -> Option<String> {
        use crate::lua::api::segment;
        let Value::Table(table) = list else {
            return None;
        };
        let count = table.borrow().length();
        let ctx_value = ctx.to_lua();
        let mut pieces = Vec::new();
        // The fastest anything asked for, so several animated segments share one timer.
        let mut soonest: Option<u64> = None;
        for i in 1..=count {
            let seg = table.borrow().get(&Value::int(i));
            let (name, priority) = segment::describe(&seg);
            let every = segment::every_ms(&seg);
            if let Some(ms) = every {
                soonest = Some(soonest.map_or(ms, |had: u64| had.min(ms)));
            }
            // **Only the ones with something new to say.** Everything else is what it drew last
            // time — which is what makes a prompt that moves affordable. See `segment::cache`.
            if let Some(kept) = segment::cache::reuse(key, &name, every) {
                pieces.push(kept);
                continue;
            }
            let Some(render) = segment::render_fn(&seg) else {
                continue;
            };
            let produced = match self.interp.call_function(&render, vec![ctx_value.clone()]) {
                Ok(values) => values.first().cloned().unwrap_or(Value::Nil),
                Err(e) => {
                    // Named, because with several segments "the prompt failed" does not say which.
                    // A segment fails on every draw, which is what `messages` counts rather than
                    // keeps five hundred times.
                    oslo_base::messages::error(format!("prompt segment '{name}'"), e.to_string());
                    continue;
                }
            };
            let text = segment::spans_to_text(&produced, &|body, style| {
                crate::lua::api::prompt::style_named(style).paint(body, oslo_ui::theme::depth())
            });
            if text.is_empty() {
                continue;
            }
            let width = oslo_ui::prompt::printed_width(&text);
            let rendered = segment::Rendered {
                name,
                priority,
                text,
                width,
            };
            segment::cache::keep(key, &rendered.name, &rendered);
            pieces.push(rendered);
        }
        // Asked for *after* rendering, so a segment that stops animating stops being woken for:
        // the deadline is only ever re-armed by a pass that found somebody still asking.
        if let Some(ms) = soonest {
            oslo_ui::prompt::animate_in(std::time::Duration::from_millis(ms));
        }
        // Half the terminal at most: a prompt wider than that leaves no room to type, which is the
        // thing the prompt exists to serve.
        let budget = ctx.cols.max(20) / 2;
        let kept = segment::fit(pieces, budget);
        Some(kept.into_iter().map(|p| p.text).collect())
    }

    /// The prompt function currently installed, if any.
    ///
    /// For a caller that has to put it back — a `.env.lua` may set the prompt for its directory,
    /// and leaving that directory has to restore whatever was there before. Returned as the opaque
    /// value it is; nothing outside Lua can do anything with it except hand it back.
    pub fn prompt_handler(&self) -> Option<Value> {
        self.registry.borrow().get(PROMPT_KEY).cloned()
    }

    /// Put a prompt function back, or remove it when there was none.
    pub fn restore_prompt(&self, handler: Option<Value>) {
        match handler {
            Some(handler) => {
                self.registry
                    .borrow_mut()
                    .insert(PROMPT_KEY.to_string(), handler);
            }
            None => {
                self.registry.borrow_mut().remove(PROMPT_KEY);
            }
        }
    }

    pub fn render_prompt(&self) -> Option<String> {
        let prompt = self.registry.borrow().get(PROMPT_KEY).cloned()?;
        match self.interp.call_function(&prompt, Vec::new()) {
            // Anything but a string is not a prompt. Rendering `nil` or a table's address into
            // the line the user types on is worse than falling back to the shell's default.
            Ok(values) => match values.first() {
                Some(Value::Str(s)) => Some(s.to_string()),
                _ => None,
            },
            Err(_) => None,
        }
    }
}

#[cfg(test)]
#[path = "engine/tests.rs"]
mod tests;
