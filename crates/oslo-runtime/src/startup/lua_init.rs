//! The optional Lua configuration layer (PLAN R9.9).
//!
//! Two `let _ =` discards used to make a broken `init.lua` indistinguishable from no `init.lua`:
//! a typo on line 1 disabled every alias, prompt and binding in the file, and the shell started
//! as if the user had never written it. Config that fails must say so; it must not take the
//! shell down with it either, which is why nothing here is fatal.

use crate::lua::LuaEngine;
use oslo_base::error::ShellError;
use oslo_shell::Environment;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Where an interactive shell looks for its config, in order. The first that exists is used.
///
/// Three names for one file rather than three files that all load: a config split across places
/// is a config nobody can find the whole of, and "which one won" is the question that follows.
///
/// Configuration is Lua and nothing else. `~/.oslorc` used to be looked for here too; anyone
/// with an old one gets a Lua syntax error, which is loud and points at the line — the loudest
/// available way to say the format changed, and better than half of it silently working.
pub fn config_paths(env: &Environment) -> Vec<PathBuf> {
    let home = env
        .get_var("HOME")
        .map(str::to_string)
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_default();

    let xdg = env
        .get_var("XDG_CONFIG_HOME")
        .map(str::to_string)
        .or_else(|| std::env::var("XDG_CONFIG_HOME").ok())
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .or_else(|| (!home.is_empty()).then(|| PathBuf::from(&home).join(".config")));

    // **One name, one language.** `oslo/config` without the extension and `~/.oslorc` both used to
    // be looked for as well. Neither is worth the question they create — "which one is being read,
    // and what happens if I have two?" — and `.oslorc` in particular reads like a shell rc file to
    // anyone who has used one, which it has not been for some time. Configuration is Lua, the file
    // says so in its name, and there is exactly one place to put it.
    //
    // `init.lua`, because the directory is a Lua *package* and that is what a package's entry point
    // is called — the same name nvim uses, and the one `require "oslo"` and `require "aliases"`
    // already imply. `?/init.lua` is on `package.path` for exactly this reason.
    let mut paths = Vec::new();
    if let Some(xdg) = xdg {
        paths.push(xdg.join("oslo/init.lua"));
    }
    let _ = home;
    paths
}

/// The config file this shell will actually read, if there is one.
pub fn config_path(env: &Environment) -> Option<PathBuf> {
    config_paths(env).into_iter().find(|p| p.is_file())
}

/// The config files oslo evaluates, in order.
///
/// **`init.lua`, and only that.** The plugins on the [runtimepath] run too, but they are loaded by
/// [`crate::plugin::load_all`] rather than listed here — it runs them attributed to the plugin they
/// came from, which is what decides the secrets each may read. Listing them here as well is how they
/// once ran twice, and every binding a plugin made was registered twice with it.
///
/// The order is still neovim's: this runs first, then the path, then the `after` roots. It used to
/// be the other way round — `conf.d` first so a hand-written file always beat anything a package
/// dropped in — which reads well until two *plugins* disagree, where it decides nothing at all.
/// `after/plugin/` is the seam that does both.
///
/// [runtimepath]: crate::runtimepath
pub fn config_files(env: &Environment) -> Vec<PathBuf> {
    config_path(env).into_iter().collect()
}

/// Wire the `oslo.*` table into `lua`, reporting a failure instead of swallowing it.
///
/// Returns whether the bindings are usable. When they are not, `init.lua` is not run at all:
/// every line in it would fail on `oslo` being nil, and one clear message beats fifty.
pub fn install_bindings(lua: &LuaEngine, env: Arc<Mutex<Environment>>) -> bool {
    match lua.setup_bindings(env) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("oslo: lua: cannot install the oslo bindings: {}", e);
            false
        }
    }
}

/// Make background work deliverable in a run that never enters the REPL.
///
/// **`oslo.spawn` was silently useless in `oslo make` and in a plain script**, and this is the half
/// of the fix that is not in Lua. A worker queues its result and writes a byte to the self-pipe;
/// `arm` is what creates that pipe, and without it the first nudge goes nowhere. The servicer is
/// what a blocking join calls to actually hand the results over.
///
/// Skipped when something is already installed, because [`oslo_base::background::install`] keeps the
/// first servicer and the REPL's does more than this one — reaping jobs, refreshing universals and
/// rebuilding the prompt. A batch run has none of those to do.
pub fn install_batch_delivery() {
    if oslo_base::background::is_installed() {
        return;
    }
    oslo_base::background::arm();
    oslo_base::background::install(crate::lua::api::spawn::deliver_if_any);
}

/// Run the config if there is one, reporting a broken one as `oslo: <path>: <error>`.
///
/// The shell carries on with its defaults afterwards either way: a config that fails half way
/// leaves whatever it managed to set, and a broken one must not cost you your shell.
pub fn load_config(lua: &LuaEngine, path: &Path) {
    if !path.is_file() {
        return;
    }
    // `load_file` still takes a `&str`; a path that is not UTF-8 is reported rather than
    // `unwrap`ped, which is what used to panic the shell before it printed its first prompt.
    let Some(text) = path.to_str() else {
        oslo_base::messages::error(path.display().to_string(), "path is not valid UTF-8");
        return;
    };
    if let Err(e) = lua.load_file(text) {
        // Printed with the path marked up and remembered without the escape codes: a config that
        // failed at startup is the thing most often still being asked about twenty commands later.
        //
        // The report comes first when there is one: it names the same path and quotes the line Lua
        // raised on, which is more than the marked-up path can say. The marked-up form is the
        // fallback, and what anything reading stderr still sees.
        let shown = std::fs::read_to_string(path).unwrap_or_default();
        if !oslo_shell::env::complain_lua(&path.display().to_string(), &shown, &e.to_string()) {
            eprintln!(
                "oslo: {}: {}",
                oslo_ui::marks::path(&path.display().to_string()),
                e
            );
        }
        oslo_base::messages::say(
            oslo_base::messages::Level::Error,
            path.display().to_string(),
            e.to_string(),
        );
    }
}

/// The status `oslo.proc.exit(n)` asked for, if that is what ended the script.
///
/// The request travels as an error because unwinding is the only way out of a call several Lua
/// frames deep, and the status rides on the error itself rather than being recovered from a
/// message. That is what makes `oslo.proc.exit` work from inside a function, a callback, or a
/// registered builtin, rather than only at the top level of a script.
fn requested_exit(err: &ShellError) -> Option<i32> {
    match err {
        ShellError::Lua(lua_err) => lua_err.exit,
        _ => None,
    }
}

/// Blank out a leading `#!` line so Lua can parse the source.
///
/// Lua's *file* loader skips a shebang; loading the same bytes as a string does not, so
/// `#!/usr/bin/env lua` reaches the parser and dies with "unexpected symbol near '#'". The line is
/// replaced rather than removed so every later line keeps its number and an error still points
/// at the right place.
fn without_shebang(source: &str) -> String {
    match source.strip_prefix("#!") {
        None => source.to_string(),
        Some(rest) => match rest.find('\n') {
            Some(end) => format!("\n{}", &rest[end + 1..]),
            None => String::new(),
        },
    }
}

/// Run Lua source as the shell's program, and exit with its status.
///
/// "Its status" is `$?` as the script leaves it, so `oslo.proc.exec("false")` at the end of the file
/// exits 1; the process used to exit 0 no matter what the script ran, which made this unusable
/// from anything that checks an exit code.
///
/// `name` is what a diagnostic calls the source — a path, or `-c`/`stdin` when there is no file.
pub fn run_lua_source(source: &str, name: &str, args: &[String]) -> i32 {
    #[cfg(feature = "watch")]
    let _watch_cleanup = WatchCleanup;
    let env = Arc::new(Mutex::new(Environment::new()));
    let lua = match LuaEngine::new() {
        Ok(lua) => lua,
        Err(e) => {
            eprintln!("oslo: lua: {}", e);
            return 1;
        }
    };
    if !install_bindings(&lua, Arc::clone(&env)) {
        return 1;
    }
    // A script has no REPL to deliver from, so `oslo.spawn` needs a servicer of its own.
    install_batch_delivery();
    if let Err(e) = lua.set_script_args(name, args) {
        eprintln!("oslo: lua: {}", e);
        return 1;
    }
    if let Err(e) = lua.eval_as(&without_shebang(source), name) {
        // `oslo.proc.exit(n)` unwinds as a shell exit rather than a Lua failure. Without this it
        // reached here as an ordinary error and printed a Lua error, so the one API for choosing
        // an exit status produced a diagnostic and exit 1 instead.
        if let Some(code) = requested_exit(&e) {
            return code;
        }
        // The error names its own file, and with a *line* where the VM knew one — `e.lua:3: …`
        // rather than the `e.lua: ` this used to put in front of it. Naming the script here as
        // well printed it twice for every failure.
        eprintln!("oslo: {e}");
        return 1;
    }
    env.lock().map(|guard| guard.last_status).unwrap_or(1)
}

#[cfg(feature = "watch")]
struct WatchCleanup;

#[cfg(feature = "watch")]
impl Drop for WatchCleanup {
    fn drop(&mut self) {
        crate::lua::api::watch_service::stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::without_shebang;

    /// A shebanged Lua script has to parse, and its line numbers have to survive: Lua's file
    /// loader skips `#!` but its string loader does not, and switching to the latter is what
    /// made `oslo script` able to detect the language at all.
    #[test]
    fn a_shebang_is_blanked_out_without_shifting_the_lines() {
        let out = without_shebang("#!/usr/bin/env lua\nprint(1)\n");
        assert_eq!(out, "\nprint(1)\n");
        assert_eq!(out.lines().count(), 2, "line numbers must not shift");
    }

    #[test]
    fn source_without_a_shebang_is_untouched() {
        assert_eq!(without_shebang("print(1)\n"), "print(1)\n");
        // A `#` that is not a shebang is Lua's length operator and must survive.
        assert_eq!(without_shebang("print(#t)\n"), "print(#t)\n");
    }

    #[test]
    fn a_shebang_with_no_newline_leaves_nothing_to_run() {
        assert_eq!(without_shebang("#!/usr/bin/lua"), "");
    }
}

/// Teach `source` that a file may be Lua.
///
/// **The rule was applied at one entry point and not the other.** `oslo script.lua` detects its
/// language; `source script.lua` sent it to the shell parser and reported a syntax error, in a shell
/// whose stated rule is that Lua never needs an opt-in flag. Installed once, from `main`, before any
/// shell runs — the slot is a plain function pointer in a `OnceLock`, so every thread sees it.
pub fn install_source_language() {
    oslo_shell::sourced::install(source_if_lua);
}

/// Run a sourced file when it is Lua. `None` hands it back to the shell parser.
///
/// **The interpreter this thread already has, when it has one.** In the REPL that is the one the
/// config ran in, so a sourced file sees what `init.lua` defined; building a second would give it
/// empty globals and quietly break `source` inside a config.
fn source_if_lua(path: &str, text: &str) -> Option<i32> {
    if crate::startup::language::detect(Some(path), text) != crate::startup::language::Language::Lua
    {
        return None;
    }
    if oslo_luavm::current::handle().is_none() {
        sourcing_engine()?;
    }
    let engine = oslo_luavm::current::handle()?;
    let body = without_shebang(text);
    match engine.eval(&body, path) {
        Ok(_) => Some(0),
        Err(e) => {
            // Lua names the line it raised on, and the text it raised in is right here — so a
            // sourced file gets the same report a shell script gets, with its own line quoted.
            if !oslo_shell::env::complain_lua(path, &body, &e.to_string()) {
                eprintln!("oslo: {e}");
            }
            Some(1)
        }
    }
}

thread_local! {
    /// Kept alive for the life of the thread, because the parked handle is what everything else
    /// reaches for and an engine dropped here would take it with it.
    static SOURCING: std::cell::RefCell<Option<LuaEngine>> =
        const { std::cell::RefCell::new(None) };
}

/// Build the interpreter a script sources into, once.
///
/// A fresh `Environment` rather than the shell's: Lua reached from inside a builtin cannot hold the
/// live one — see `oslo_shell::env::view`, which is this codebase's standing answer to that. What a
/// sourced file registers lands in the thread-local tool table, which is what makes it worth doing.
fn sourcing_engine() -> Option<()> {
    SOURCING.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let engine = LuaEngine::new().ok()?;
            // Publishes the interpreter as this thread's, which is what the caller reads back.
            if !install_bindings(&engine, Arc::new(Mutex::new(Environment::new()))) {
                return None;
            }
            install_batch_delivery();
            *slot = Some(engine);
        }
        Some(())
    })
}
