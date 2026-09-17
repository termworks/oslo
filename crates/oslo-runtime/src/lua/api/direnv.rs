//! `oslo.direnv` — the library a `.env.lua` is written against.
//!
//! A namespace rather than more functions on `oslo`, because these are for one job. `oslo` itself
//! had grown twenty-four flat entries with `nix_develop` sitting between `login` and `path_add`,
//! which tells a reader nothing about which of them belong together. `fs`, `json`, `path`, `proc`
//! and `re` were already tables; this is the same idea applied to the directory environment.
//!
//! # Thin, on purpose
//!
//! Every function here is a call into [`oslo_shell::direnv`], and the work is there rather than in
//! this file. `nix_develop` is the one that makes the split worth keeping: it takes a dev shell's
//! environment from `nix print-dev-env --json` while withholding the handful of variables that
//! would wreck the shell you are standing in, and getting that list wrong is silent and severe. See
//! `oslo_shell::nix_shell` for what those are and why.

use super::util::{put, text};
use oslo_base::value::{LuaError, Table, Value};
use oslo_shell::direnv::paths;
use oslo_shell::env::Environment;
#[cfg(feature = "nix")]
use oslo_shell::nix_shell as devshell;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

/// Build the `oslo.direnv` table.
pub fn build(env: &Arc<Mutex<Environment>>) -> Value {
    let mut it = Table::new();
    // `oslo.direnv.nix_develop` is the seam: a directory file asking for a dev shell. It needs
    // both halves, and a build with only one of them simply does not offer the name.
    #[cfg(feature = "nix")]
    nix_develop(&mut it, env);
    path_add(&mut it, env);
    dir(&mut it);
    watching(&mut it);
    #[cfg(feature = "watch")]
    watch_commands(&mut it);
    unloading(&mut it);
    Value::table(it)
}

/// `oslo.direnv.dir()` — the directory whose file is running.
///
/// # Why a file needs to be told where it is
///
/// It is now also the working directory, so most files never have to ask. This is for the ones that
/// do: a path being *stored* rather than used — a cache location, a marker written into
/// `oslo.state`, a value handed to a program that will run somewhere else later — has to be
/// absolute, and `"./thing"` stops meaning anything the moment the shell moves on.
///
/// `nil` outside a directory environment, which is the honest answer in `init.lua` and in
/// `oslo make`: no file is loading, so there is no directory speaking for one, and a caller should
/// fall back to `oslo.fs.cwd()`.
fn dir(it: &mut Table) {
    put(it, "dir", |_, _| {
        Ok(vec![match super::mark::loading_directory() {
            Some(path) => Value::str(path),
            None => Value::Nil,
        }])
    });
}

fn path_add(it: &mut Table, env: &Arc<Mutex<Environment>>) {
    // oslo.direnv.path_add(dir, [var]) — put `dir` on the front of `$PATH`, or of the named variable.
    //
    // The single most common thing a `.env.lua` does, and spelling it by hand is both wordy and
    // easy to get subtly wrong: forgetting the separator, or appending rather than prepending so
    // the project's own tool loses to the system one. Relative paths resolve against the current
    // directory, because a directory environment saying `./bin` means *its* bin.
    //
    // Idempotent, so a reload does not grow the variable each time. This is `PATH_add` from an
    // `.envrc` under another name — one implementation, so the two cannot drift.
    let env = Arc::clone(env);
    put(it, "path_add", move |_, args| {
        let dir = text(&args, 1, "oslo.direnv.path_add")?.to_string();
        let name = match args.get(1) {
            Some(Value::Str(name)) => name.to_string(),
            _ => "PATH".to_string(),
        };
        let mut guard = crate::lua::engine::borrow_env(&env)?;
        paths::prepend_into(&mut guard, &name, &[dir])
            .map_err(|e| LuaError::new(format!("oslo.direnv.path_add: {e}")))?;
        let joined = guard.get_var(&name).unwrap_or_default().to_string();
        Ok(vec![Value::str(joined)])
    });
}

#[cfg(feature = "nix")]
fn nix_develop(it: &mut Table, env: &Arc<Mutex<Environment>>) {
    let env = Arc::clone(env);
    put(it, "nix_develop", move |_, args| {
        // `oslo.direnv.nix_develop()` means this directory's flake; a string names another installable,
        // exactly as `use flake ..#other` does. A table asks for options.
        let (forwarded, want) = match args.first() {
            Some(Value::Str(_)) => (
                vec![text(&args, 1, "oslo.direnv.nix_develop")?.to_string()],
                devshell::Want::default(),
            ),
            // `nix_develop{ hook = true, functions = true }`, and optionally the installable.
            Some(Value::Table(options)) => {
                let options = options.borrow();
                let named = match options.get_str("flake") {
                    Value::Str(name) => vec![name.to_string()],
                    _ => Vec::new(),
                };
                let on = |key: &str| matches!(options.get(&Value::str(key)), Value::Bool(true));
                (
                    named,
                    devshell::Want {
                        hook: on("hook"),
                        functions: on("functions"),
                        // `builtins.getEnv` is empty in a pure evaluation, so a flake that reads
                        // the environment this file just set needs to be told it may.
                        impure: on("impure"),
                    },
                )
            }
            // Nothing named: `print-dev-env` resolves this directory's flake by itself, which is
            // what a bare `use flake` relies on too.
            _ => (Vec::new(), devshell::Want::default()),
        };
        let count = {
            let mut guard = crate::lua::engine::borrow_env(&env)?;
            devshell::apply_with(&mut guard, &forwarded, want)
                .map_err(|e| LuaError::new(format!("oslo.direnv.nix_develop: {e}")))?
        };
        Ok(vec![Value::Number(oslo_base::value::Number::Int(
            count as i64,
        ))])
    });
}

thread_local! {
    /// What the *loaded* directory asked to run on the way out, in registration order.
    static ACTIVE: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    /// What the file currently loading has registered.
    ///
    /// **Two slots rather than one, and that is not tidiness.** `Direnv::arrive` unloads the old
    /// directory and loads the new one inside a single call, so the `restore` callback fires while
    /// the new file's registrations are already arriving. One slot would have the incoming file's
    /// callbacks run as the outgoing file's. `PREVIOUS_PROMPT` in `startup/environments` carries the
    /// same shape for the same reason, and its comment is the longer version of this one.
    static PENDING: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
}

/// Run and forget whatever the leaving directory registered.
///
/// Called from the `restore` callback, which `Direnv::unload` runs **before** it takes the
/// environment lock — so a callback here has the whole API, including `oslo.env.set` and
/// `oslo.run`, and sees the directory's own variables still in place.
///
/// **Drained before anything is called.** A callback is free to register another, and appending to
/// a list being iterated is how that becomes a loop nobody wrote. `oslo.spawn`'s delivery does the
/// same and says so.
///
/// **A raise is reported and the rest still run.** By the time this fires the unload is already
/// committed and the undo record is being spent; abandoning half way would leave the directory's
/// variables set with nothing left to remove them.
pub(crate) fn run_unload() {
    let callbacks = ACTIVE.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    // Reverse: a file that registers in the order it sets things up should tear down in the
    // opposite order, which is what every other unwinding in the shell does.
    for callback in callbacks.into_iter().rev() {
        if let Err(e) = crate::lua::engine::call_here(&callback, Vec::new()) {
            eprintln!("oslo: .env.lua: on_unload: {e}");
        }
    }
}

/// Make what the file just loaded registered the live set. Called after the whole arrival.
pub(crate) fn promote_unload() {
    let fresh = PENDING.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    if !fresh.is_empty() {
        ACTIVE.with(|slot| slot.borrow_mut().extend(fresh));
    }
}

fn unloading(it: &mut Table) {
    // oslo.direnv.on_unload(fn) -> true
    //
    // The other half of a directory environment. Variables and aliases are put back by the undo
    // record; anything else a file did — a completion it registered, a background job, a marker it
    // wrote — had no moment at which to be undone.
    put(it, "on_unload", |_, args| {
        let Some(callback @ Value::Function(_)) = args.first() else {
            return Err(LuaError::new(
                "oslo.direnv.on_unload: expected a function".to_string(),
            ));
        };
        if super::mark::loading_directory().is_none() {
            return Err(LuaError::new(
                "oslo.direnv.on_unload: only while a directory environment is loading".to_string(),
            ));
        }
        PENDING.with(|slot| slot.borrow_mut().push(callback.clone()));
        Ok(vec![Value::Bool(true)])
    });
}

#[cfg(feature = "watch")]
fn watch_commands(it: &mut Table) {
    put(it, "watch_command", |_, args| {
        let root = loading_base("oslo.direnv.watch_command")?;
        let value = args.first().ok_or_else(|| {
            LuaError::new("oslo.direnv.watch_command: expected a table".to_string())
        })?;
        let started = super::watch_service::start_directory(value, &root)?;
        if let Some(cleanup) = started.cleanup {
            PENDING.with(|slot| slot.borrow_mut().push(cleanup));
        }
        Ok(vec![started.handle])
    });
}

fn watching(it: &mut Table) {
    // oslo.direnv.watch_file(path, …) -> how many were added
    // oslo.direnv.watch_dir(dir)      -> true
    //
    // A directory environment reloads when its own file changes. These say what *else* counts as a
    // change — the `.tool-versions` a layout reads, the lockfile a cache key is derived from.
    //
    // The machinery was already there and already drained for both file kinds; nothing on this side
    // ever filled the list, so for a `.env.lua` it was always empty. `.envrc` has had `watch_file`
    // since the beginning.
    put(it, "watch_file", |_, args| {
        let base = loading_base("oslo.direnv.watch_file")?;
        let mut added = 0;
        for (i, _) in args.iter().enumerate() {
            let path = text(&args, i + 1, "oslo.direnv.watch_file")?;
            paths::watch(&paths::absolute(&path, &base));
            added += 1;
        }
        if added == 0 {
            return Err(LuaError::new(
                "oslo.direnv.watch_file: expected at least one path".to_string(),
            ));
        }
        Ok(vec![Value::int(added)])
    });

    // **A directory's own timestamp moves when an entry is added, removed or renamed, and does not
    // move when a file inside it is edited in place.** So this notices `config/new.toml` appearing
    // and does not notice `config/app.toml` being changed. That is exactly what `.envrc`'s
    // `watch_dir` has always done; the two are one call so they cannot drift, and the limitation is
    // written down here rather than discovered.
    put(it, "watch_dir", |_, args| {
        let base = loading_base("oslo.direnv.watch_dir")?;
        let path = text(&args, 1, "oslo.direnv.watch_dir")?;
        paths::watch(&paths::absolute(&path, &base));
        Ok(vec![Value::Bool(true)])
    });
}

/// The directory a relative watch resolves against, or a refusal.
///
/// **Refused outside a load rather than resolved against the working directory.** The list is
/// drained by the *next* arrival, so a `watch_file` called from a timer or a spawn callback would
/// quietly attach that path to whichever unrelated project loaded next. The stdlib states the same
/// rule for its own functions in words; this is that rule with a Lua spelling.
fn loading_base(function: &str) -> Result<std::path::PathBuf, LuaError> {
    match super::mark::loading_directory() {
        Some(dir) => Ok(std::path::PathBuf::from(dir)),
        None => Err(LuaError::new(format!(
            "{function}: only while a directory environment is loading"
        ))),
    }
}
