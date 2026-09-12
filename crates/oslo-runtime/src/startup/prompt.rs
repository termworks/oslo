//! What the prompt says, in whichever language the line is being read as.
//!
//! Split from `repl` because it is a separate question from *reading* a line: the loop asks for a
//! string and draws it, and everything about where that string comes from — a Lua function, `PS1`,
//! or the built-in default — is decided here.

use super::mode::Mode;
use super::rc;
use crate::lua::LuaEngine;
use oslo_shell::Environment;
use std::sync::{Arc, Mutex};

/// The facts a prompt segment is drawn from, gathered once.
///
/// Everything here the shell already knows; a segment looking each of them up itself would run
/// `git` once per segment per keystroke.
pub fn segment_context(
    last_status: i32,
    mode: Mode,
    vimode: Option<&str>,
) -> crate::lua::context::Context {
    crate::lua::context::Context {
        status: last_status,
        duration_ms: super::repl::last_command_duration().map(|d| d.as_millis() as u64),
        cwd: super::repl::cwd(),
        branch: oslo_ui::prompt::git_branch(),
        user: whoami(),
        host: hostname(),
        language: mode.name().to_string(),
        vimode: vimode
            .map(str::to_string)
            .or_else(|| oslo_ui::vi::mode().map(|m| m.name().to_string())),
        cols: oslo_ui::dropdown::terminal_cols(),
        // The real count. Hardcoded `0` until now, which made a `jobs` segment in a prompt — the
        // reason the field exists — always draw nothing. Every prompt tool reads this; in bash it
        // is `jobs -p | wc -l` and in zsh `${#jobstates}`.
        jobs: oslo_shell::exec::job::with_jobs(|jobs| jobs.jobs().len()),
        continuation: false,
        // Nothing is running at a prompt; `title_context` fills this in for the other case.
        command: None,
    }
}

/// The same facts, for a title drawn while `command` is running.
pub fn title_context(last_status: i32, mode: Mode, command: &str) -> crate::lua::context::Context {
    crate::lua::context::Context {
        command: Some(command.to_string()),
        ..segment_context(last_status, mode, None)
    }
}

/// Who is logged in, `$USER` first because that is what `su` updates.
fn whoami() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| {
            nix::unistd::User::from_uid(nix::unistd::getuid())
                .ok()
                .flatten()
                .map(|u| u.name)
        })
        .unwrap_or_else(|| "?".to_string())
}

/// This machine's short name — everything before the first dot.
fn hostname() -> String {
    nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .map(|h| h.split('.').next().unwrap_or(&h).to_string())
        .unwrap_or_else(|| "?".to_string())
}

pub fn primary_prompt(
    env_struct: &Arc<Mutex<Environment>>,
    lua: &LuaEngine,
    last_status: i32,
    mode: Mode,
) -> String {
    // Published before the prompt is drawn, so a `PS1` or a Lua prompt function can say which
    // language it is prompting for.
    env_struct
        .lock()
        .unwrap()
        .set_var("OSLO_MODE", mode.name(), false);

    // A Lua prompt is an explicit choice by the user and outranks `PS1`, which in turn outranks
    // the built-in default.
    lua.render_with("prompt.left", &segment_context(last_status, mode, None))
        .or_else(|| lua.render_prompt())
        .unwrap_or_else(|| {
            // Both languages get the *same* prompt, with the language as one of its segments —
            // `you@host | N | lua >`. A separate `lua>` used to be the only signal, which meant
            // switching language threw away the branch, the mode and the directory as well.
            //
            // `PS1` still wins for shell lines, because that is what `PS1` is. It cannot win for
            // Lua ones: it describes a shell prompt, and drawing `oslo$` in front of something
            // that is not a shell command is exactly the confusion this segment exists to stop.
            if mode == Mode::Lua {
                oslo_ui::prompt::render_default_left_prompt(last_status, mode.name())
            } else {
                rc::ps1(
                    &mut env_struct.lock().unwrap_or_else(|held| held.into_inner()),
                    last_status,
                )
            }
        })
}

thread_local! {
    /// What building a prompt needs, kept so a plain `fn` can do it.
    ///
    /// **Because the closure the editor was given is gone by then.** A prompt kept alive while a
    /// command runs — see [`oslo_ui::prompt::hold`] — is built long after `read_line` returned and
    /// its borrows were dropped, so the pieces are cloned here instead. Both are `Rc`/`Arc` inside,
    /// so this is two pointer bumps per prompt and not a second interpreter.
    static ALIVE: std::cell::RefCell<Option<(Arc<Mutex<Environment>>, LuaEngine)>> =
        const { std::cell::RefCell::new(None) };
}

/// Remember what the next prompt is built from, and register how to build it.
///
/// Called once per prompt, because `last_status` and the language change and a prompt built from
/// last cycle's would be quietly wrong about both.
pub fn keep_alive(env: &Arc<Mutex<Environment>>, lua: &LuaEngine) {
    ALIVE.with(|slot| *slot.borrow_mut() = Some((Arc::clone(env), lua.clone())));
    oslo_ui::prompt::hold::renders_with(again);
}

/// Build the prompt as it stands, for [`oslo_ui::prompt::hold::pump`].
///
/// **Never through [`primary_prompt`], and never touching the `Environment`.** This runs while a
/// command is *executing*, and a builtin executes holding the shell state — `builtin_nav` is handed
/// `&mut Environment`, so the mutex is locked for as long as the browser is open. `primary_prompt`
/// locks it twice, to publish the language and to read `$PS1`, and either would be a deadlock
/// against a lock this same thread is holding.
///
/// So the prompt is built from what needs nothing held: the Lua the config installed, over facts
/// taken from the process rather than the shell. Lua reaching for the environment is safe on its
/// own account — the `oslo.*` verbs `try_lock` and answer rather than block.
///
/// **What that costs is `$PS1`.** A prompt written as a shell variable is not rebuilt while a
/// detached command runs; the one on screen stays. A prompt written in Lua, or handed to another
/// program, is — which is every prompt this was built for.
fn again() -> Option<(String, String)> {
    ALIVE.with(|slot| {
        let held = slot.borrow();
        let (_, lua) = held.as_ref()?;
        // The status of the command before this one, which is what the prompt was drawn with and
        // what it should keep saying — the command running now has not ended.
        let status = oslo_ui::prompt::hold::last_status();
        let facts = segment_context(status, Mode::Shell, None);
        // Falls back the way the editor's own render does. Bailing here instead meant a config
        // that styled only its *right* prompt never repainted at all — and the right prompt is
        // where a directory and a spinner usually are, so that was every case this exists for.
        let left = lua
            .render_with("prompt.left", &facts)
            .unwrap_or_else(|| oslo_ui::prompt::render_default_left_prompt(status, "sh"));
        let right = lua
            .render_with("prompt.right", &facts)
            .unwrap_or_else(|| oslo_ui::prompt::render_default_right_prompt(status, None));
        Some((left, right))
    })
}
