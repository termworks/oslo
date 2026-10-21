//! Everything the shell does between one command ending and the next prompt being drawn.
//!
//! Six phases, and what they have in common is that none of them is about the line you are about
//! to type: they reconcile the session with whatever changed while it was busy. The directory it
//! is standing in, the macros another window edited, the universal variables another window set,
//! what this shell publishes about itself, and the two hooks a prompt integration hangs off.
//!
//! **They are here rather than in the loop because they are one thing.** Each is cheap when nothing
//! has changed — a comparison, two `stat`s, one `stat`, a relaxed load — and the reason they can
//! afford to run before *every* prompt is that the common answer is "nothing to do". Read together
//! that is obvious; read spread through the loop it looks like six unrelated costs.

use super::super::{arrival, integration, stored, timing};
use crate::lua::LuaEngine;
use crate::lua::api::hooks;
use oslo_shell::Environment;
use std::sync::{Arc, Mutex};

/// Bring the session up to date, then let a config draw what it wants to.
///
/// `settled` is the directory the shell last reconciled against, and is updated in place.
pub(super) fn each_prompt(
    env: &Arc<Mutex<Environment>>,
    lua: &LuaEngine,
    macros_held: &Mutex<stored::Held>,
    settled: &mut String,
    last_status: i32,
) {
    // Startup held echo off so a late query reply was not printed; the editor owns
    // the terminal from here.
    oslo_ui::term::query::release_echo();

    // **Wherever the shell got here from, the prompt is the moment it has to be true.**
    //
    // The directory environment used to be reconciled in one place only: after a command line
    // whose start and end directories differed. That misses every other way of moving — a key
    // binding that jumps, a Lua hook, a `cd` in a sourced file, anything that does not straddle a
    // whole command — and what is left behind is not nothing. It is the previous project's
    // `$PATH`, its exported variables and its aliases, in a shell standing somewhere else. An
    // alias like `_b` then still builds the repository you walked out of, and the only cure was a
    // `cd` round trip to force the check to run.
    //
    // Comparing against the directory last *settled* rather than against where the last command
    // happened to start makes the route irrelevant. `arrive` itself is cheap when nothing has
    // changed, and this only calls it when the directory actually differs.
    timing::phase("direnv", || {
        let here = super::current_directory();
        if here != *settled {
            arrival::arrive(env, lua, std::path::Path::new(&here));
            *settled = here;
        }
    });

    // Another shell — or `oslo macros` in this one — may have added, removed or turned one off
    // since the last prompt. Two `stat`s decide whether there is anything to do, so the common
    // case, nobody changed anything, costs those and no parse.
    timing::phase("macros", || {
        if let Ok(mut held) = macros_held.lock() {
            stored::refresh(env, &mut held);
        }
    });

    // The same argument one file over: another window may have run `set -U` since the last prompt,
    // and this is where this session finds out. One `stat` decides, so a machine that has never
    // set one pays that and nothing else. See `oslo_shell::env::universal` for why a stat here
    // rather than an inotify watch in the loop.
    #[cfg(feature = "universal")]
    timing::phase("universal", || {
        if let Ok(mut env) = env.lock() {
            oslo_shell::env::universal::sync_into(&mut env);
        }
    });

    // What the shell says about itself: the terminal's size, and — only while serving — the
    // control socket's fallback copy of the environment. See `session::publish`.
    timing::phase("publish", || super::session::publish(env));

    // A prompt is about to be drawn. This is bash's `PROMPT_COMMAND` and zsh's `precmd`, and the
    // hook a prompt integration written in Lua needs — the shell-side one already exists as
    // `$PROMPT_COMMAND` below.
    timing::phase("pre-prompt", || {
        lua.fire_at(hooks::at::PRE_PROMPT, Vec::new())
    });

    // `$PROMPT_COMMAND` runs before every prompt. It is the other half of the DEBUG trap —
    // together they are bash's preexec/precmd pair, and every integration written for bash is
    // built on the two of them: starship redraws `PS1` here, hexe reports the command that just
    // finished. Fired before the line is read rather than after a command, so it also runs for the
    // first prompt of the session, which is where a prompt integration draws itself.
    timing::phase("prompt-command", || {
        integration::prompt_command(env, last_status)
    });
}
