//! The top of the stack: the Lua API, the interpreter that owns it, and starting up.
//!
//! Everything below this crate is a shell that can be driven. This is what drives it — reads the
//! config, installs `oslo.*`, opens the history store, and runs the read loop.
//!
//! # Why the Lua *API* is above the shell when the Lua *evaluator* is below it
//!
//! They are different things wearing the same word. `oslo-lua` is an interpreter: it knows how to
//! run Lua and nothing about shells, which is why it sits at the bottom and is publishable alone.
//! `lua::api` is how a config *reaches the shell* — `oslo.env.set`, `oslo.register_builtin`,
//! `oslo.ui.choose` — so it necessarily names everything underneath it and belongs at the top.
//!
//! Keeping both in one module is what used to pin the whole Lua layer beneath the editor and the
//! executor while `lua::api` needed to sit above them. Nothing can be on both sides of everything
//! else; splitting the two is what let the rest of the crates fall out.

/// The stack oslo runs its own work on, rather than whatever `ulimit -s` happens to be.
///
/// oslo's Lua is a tree-walker: a Lua function calling a Lua function is a chain of Rust frames,
/// so Lua's recursion depth is bounded by the Rust stack. Real Lua is not — it keeps Lua-to-Lua
/// calls on its own heap-allocated stack — so the only way to give a script a predictable limit is
/// to stop depending on the ambient one. A shell inherits its stack from whoever spawned it, which
/// under a service manager can be as little as 512 KiB.
///
/// The reservation is virtual: pages are committed as they are touched, so an `oslo` that never
/// runs Lua pays nothing for this. `oslo_lua` refuses at a depth chosen to fit well inside it, and
/// `lua_eval_tests` runs its depth cases on a thread of exactly this size so the limit is checked
/// against the stack oslo actually provides.
pub const INTERPRETER_STACK: usize = 16 * 1024 * 1024;

pub mod editor;
pub mod lua;
pub mod macros;
/// Installing somebody else's Lua, and loading it on demand. `plugin` only.
#[cfg(feature = "plugin")]
pub mod plugin;
pub mod runtimepath;
pub mod startup;

pub use lua::LuaEngine;

/// History expansion — `!!`, `!42`, `^a^b`.
///
/// **Only reachable from the interactive prompt**, which is what this crate is. It rewrites a line
/// before it is parsed, so a `-c` or a script able to reach it would let data turn into a different
/// command; keeping it above everything the shell can be driven through is what makes that
/// impossible rather than merely unlikely.
mod history_expand;
#[cfg(test)]
mod serial;

use history_expand::Expansion;
use oslo_base::{Result, ShellError};

/// `break`, `continue` and `return` outside any loop or function are a no-op, not an error.
///
/// They unwind as errors so nested command lists can pass them up; if nothing catches one it has
/// reached the top level, where bash silently ignores it rather than printing a diagnostic.
pub fn absorb_loop_control(result: Result<i32>) -> Result<i32> {
    match result {
        Err(ShellError::Break(_)) | Err(ShellError::Continue(_)) => Ok(0),
        Err(ShellError::Return(code)) => Ok(code),
        other => other,
    }
}

/// Resolve `!`/`^` history references in a line typed at the prompt.
///
/// `None` means the line must not run: a reference that cannot be resolved is a mistake, and bash
/// answers it by discarding the line, printing the reason, and leaving `$?` untouched — nothing
/// ran, so nothing should have changed. A rewritten line is echoed to stderr first, because the
/// user has to be able to see what `!!` turned into before it takes effect.
pub fn expand_history(line: &str, history: &[String]) -> Option<String> {
    match history_expand::expand(line, history) {
        Ok(Expansion::Unchanged) => Some(line.to_string()),
        Ok(Expansion::Expanded(expanded)) => {
            eprintln!("{}", expanded);
            // `^a^b` can leave nothing behind; an empty line is not a command.
            if expanded.trim().is_empty() {
                return None;
            }
            Some(expanded)
        }
        Err(err) => {
            eprintln!("oslo: {}", err);
            None
        }
    }
}
