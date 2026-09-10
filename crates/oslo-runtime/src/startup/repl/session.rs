//! The three things a session does around the loop rather than inside it.
//!
//! Split from `repl.rs` because that file is the loop, and these are its edges: what every
//! prompt publishes, what the last one fires, and what is left tidy on the way out. None of them
//! reads a command or decides anything about one.

use super::history;
use crate::lua::LuaEngine;
use crate::lua::api::hooks;
use oslo_shell::Environment;
use std::sync::{Arc, Mutex};

/// Put the terminal's size in `$COLUMNS` and `$LINES`, **exported**, before every prompt.
///
/// # Why a shell has to do this
///
/// A program the shell runs cannot ask how wide the terminal is unless it *has* the terminal —
/// and a prompt renderer is usually reading a pipe, because the shell is capturing what it says.
/// `TIOCGWINSZ` on a pipe fails, so every such tool falls back to `$COLUMNS`, and to 80 when that
/// is missing too. bash maintains both under `checkwinsize`; zsh keeps them as special variables.
///
/// oslo did neither, and the effect was not subtle: `hexe shp prompt` laid every prompt out as
/// though the terminal were 80 columns wide however wide it really was, so segments were dropped
/// as "not fitting" on a 230-column screen. The same is true of starship, of `$(tput cols)` inside
/// a substitution, and of anything else asked to render into a pipe.
///
/// Refreshed per prompt rather than from a `SIGWINCH` handler, which is what `checkwinsize` does
/// and is enough: nothing between two prompts can observe the value except a command, and a
/// command that ran before the resize could not have seen it anyway. It also keeps the signal
/// handler free of anything that allocates.
///
/// **Exported**, unlike bash's, because the whole point is that a *child* reads it — a shell
/// variable a child cannot see would fix nothing here.
pub(super) fn publish_terminal_size(env: &Arc<Mutex<Environment>>) {
    let cols = oslo_ui::dropdown::terminal_cols();
    let rows = oslo_ui::dropdown::width::terminal_rows();
    let mut guard = env.lock().unwrap_or_else(|held| held.into_inner());
    guard.set_var("COLUMNS", &cols.to_string(), true);
    guard.set_var("LINES", &rows.to_string(), true);
}

/// Everything the shell says about itself before a prompt is drawn.
///
/// Two facts that are true of the *shell* rather than of the line about to be typed: how big the
/// terminal is, and — for a shell that is serving — what its environment holds. Both are cheap and
/// both are wanted at the same moment, so they share a phase rather than each having one.
///
/// The second returns immediately unless a control socket is bound, which is almost every shell.
/// See `lua::api::live::publish` for why a copy is taken at all.
pub(super) fn publish(env: &Arc<Mutex<Environment>>) {
    publish_terminal_size(env);
    crate::lua::api::live::publish(env);
}

/// `on-exit`, from both ways a REPL ends — `exit` and end of input.
///
/// **Before the EXIT trap, not after.** The trap may itself call `exit`, and a hook that ran after
/// it would be skipped exactly when the session ended in the way most worth reporting. Both call
/// sites are needed because POSIX makes no distinction between the two endings and neither does
/// anything else here.
pub(super) fn fire_exit(lua: &LuaEngine, status: i32) {
    // Anything still held runs before the session's last hook does. A `cd` in the command that
    // ended the shell would otherwise have queued a `post-change-dir` that nothing ever drained.
    crate::lua::engine::run_deferred_hooks();
    lua.fire_at(
        hooks::at::ON_EXIT,
        vec![LuaEngine::hook_fields(&[(
            "status",
            oslo_base::value::Value::int(status as i64),
        )])],
    );
    // **After the hook, so a handler can still be asked about this shell**, and before the process
    // goes: a socket file that outlives its listener is a path a peer connects to and is refused
    // by. Binding tests for that and clears it, so this is tidiness rather than correctness — which
    // is also why nothing here worries about the shell being killed outright.
    crate::lua::api::live::stop_serving();
}

/// Leave the history in the state a shell that is not running should leave it.
///
/// The trim puts the history table back inside `$HISTSIZE`. It has to happen here as well as on the
/// amortised counter, or a session shorter than the counter's period never enforces the bound at
/// all. Best effort, and it does not block: another terminal holding the file means it does not
/// happen this time.
///
/// Neither store has a checkpoint any more, because neither has a log. Both are one file that is
/// consistent at every commit, and the tracker's own bound is the daily sweep in `track::prune`
/// rather than anything the way out of the loop can do. See `history_db`'s note and that module's.
pub(super) fn settle_stores(settings: &history::Settings) {
    // **Before the trim**, and before anything else here reads the store: the last command's
    // boundary and outcome are on the writer thread, and a trim that ran ahead of them would bound
    // a history that is still one command short of itself. Every ordinary way out of the loop comes
    // through here, which is what makes the deferral safe — see `track::writer`.
    oslo_base::track::writer::settle();
    if let Some(db) = oslo_base::track::store() {
        db.trim(settings.max_size.max(1));
    }
    // The predictor's snapshot, written once on the way out rather than after every command.
    //
    // It costs well under a millisecond and is about 31 KB, but it is still a file write, and a
    // shell that did it per command would be doing it for nothing — the model is only *read* at
    // the next start. Best effort, like the trim: another shell writing at the same instant means
    // one of the two saves wins, and the loser costs a session's learning rather than anything a
    // user would notice.
    //
    // Nothing is written for a session that keeps no history, which is the same gate the model was
    // read behind in `Tracker::start`. A model in this process at all means the gate let it in.
    #[cfg(feature = "vista")]
    {
        if !oslo_base::predict::ready() {
            return;
        }
        if let Some(path) = oslo_base::predict::default_path(
            std::env::var("XDG_DATA_HOME").ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
        ) {
            oslo_base::predict::save_shared(&path);
        }
    }
}
