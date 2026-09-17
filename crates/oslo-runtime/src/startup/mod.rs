//! What the shell does around the user's program: startup files, prompts, history, Lua.
//!
//! These sit at the *top* of the stack, above everything they drive. Every one of them reads the
//! real user's `$HOME`, sources arbitrary files, or holds process-global state — so nothing below
//! may reach them, and nothing here is on a path a library caller can take by accident. That is
//! the same rule `history_expand` follows by living in the binary alone: it rewrites a line before
//! it is parsed, so a `-c` or a script reaching it would let data turn into a different command.
//!
//! Split by the question each file answers:
//!
//! * [`rc`] — which files a new shell reads before the first command, and what the prompt says.
//! * [`history`] — where the history lives, how big it is, and the `history` builtin.
//! * [`lua_init`] — the optional `init.lua` layer, and what happens when it is broken.
//! * [`repl`] — the interactive loop that uses all three.
//! * `tracking` — what that loop hands [`oslo_base::track`] instead of discarding.

mod arrival;
pub mod config;
#[cfg(feature = "direnv")]
mod environments;

/// Load the directory environment for `dir` in a tool that is not the REPL — `oslo make`, and
/// anything else that runs a project's own code in a process of its own.
///
/// **A recipe runs in the environment its directory declares.** Without this `oslo make` ran in
/// whatever the calling shell happened to be holding, so a `.make.lua` could not rely on anything
/// its own `.env.lua` computes — and a value the interactive shell had cached from an earlier,
/// possibly different, evaluation is what the recipe actually got.
///
/// The allow list still governs: an `.env.lua` nobody has approved is reported and not read, on
/// exactly the same terms as at a prompt.
#[cfg(feature = "direnv")]
pub fn load_directory_environment(
    env: &std::sync::Arc<std::sync::Mutex<oslo_shell::Environment>>,
    lua: &crate::lua::LuaEngine,
    dir: &std::path::Path,
) {
    environments::start();
    environments::arrive(env, lua, dir);
}
mod follow;
pub mod history;
mod integration;
pub mod language;
pub mod lua_init;
pub mod mode;
pub mod native;
mod nested;
mod plugins;
pub mod prompt;
pub mod rc;
mod read;
pub mod recall;
pub mod repl;
#[cfg(feature = "direnv")]
mod report;
mod servicing;
mod stored;
mod terminal;
mod timers;
pub mod timing;
mod tracking;
mod transcript;
