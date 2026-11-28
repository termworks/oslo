//! `oslo.env` — shell and environment variables.
//!
//! A file of its own, like every other namespace, and it was the last one without one: these lived
//! inline in [`super`] while `oslo.fs`, `oslo.path`, `oslo.re` and the rest each had a module. The
//! split is not tidying — `mod.rs` had reached 596 of the 600 lines `scripts/check-loc.sh` allows,
//! so every namespace that wanted a line there was blocked behind this one.
//!
//! # These take the lock, and that is the whole distinction
//!
//! Everything here goes through [`borrow_env`], so everything here raises *shell state is busy*
//! inside a registered builtin or an answering hook. That is not true of most of `oslo.*` — the
//! lock is 28 functions across six files, not the whole API — and the difference is worth knowing
//! before reaching for a workaround. See `docs/features/your-own-tools.md`.

use super::envlist;
use super::util::{list, put, record, text};
use crate::lua::engine::borrow_env;
use oslo_base::value::{Table, Value};
use oslo_shell::env::Environment;
use std::sync::{Arc, Mutex};

/// Install `oslo.env`.
pub fn install(oslo: &mut Table, env: &Arc<Mutex<Environment>>) {
    let env_get = Arc::clone(env);
    put(oslo, "get", move |_, args| {
        let name = text(&args, 1, "oslo.env.get")?;
        Ok(vec![match borrow_env(&env_get)?.get_param(&name) {
            Some(value) => Value::str(value),
            None => Value::Nil,
        }])
    });

    let env_set = Arc::clone(env);
    // oslo.env.set(name, value, { export = false, secret = true }) -> true, or nil + why
    //
    // `secret = true` marks the variable sensitive and `secret = false` clears the mark; a value
    // `oslo.secret.*` handed out is marked without being asked. See `env::scope::sensitive`.
    //
    // **`export` defaults to true, and `false` is the option that did not exist.** A directory
    // environment could only ever create *exported* variables, so a project name, a chosen profile
    // or a computed cache key reached every child that project ever spawned — which is how a
    // `$DATABASE_URL`-shaped value ends up in an editor's environment and then in its crash report.
    put(oslo, "set", move |_, args| {
        let name = text(&args, 1, "oslo.env.set")?;
        let value = text(&args, 2, "oslo.env.set")?;
        // A table third argument, the shape `oslo.env.path_add{ last = true }` already uses.
        let (export, secret) = match args.get(2) {
            Some(Value::Table(options)) => {
                let options = options.borrow();
                let export = match options.get_str("export") {
                    Value::Nil => true,
                    given => given.truthy(),
                };
                let secret = match options.get_str("secret") {
                    Value::Nil => None,
                    given => Some(given.truthy()),
                };
                (export, secret)
            }
            // Absent means the old spelling, `oslo.env.set(n, v)`, which exported.
            _ => (true, None),
        };
        let mut guard = borrow_env(&env_set)?;
        // **Demoting takes two steps, and skipping the first is a silent no-op.** `set_var` ORs the
        // flag it is given with the one the variable already carries, so passing `false` to a name
        // that is exported changes nothing at all. Clearing the entry first is the only way down —
        // the same dance `Direnv::unload` does, and it says so for the same reason.
        if !export {
            // Guarded, because `unset_var` does not check readonly and `set_var` does: without this
            // the pair would delete a readonly variable and then decline to write it back.
            if guard.is_readonly(&name) {
                return Ok(vec![
                    Value::Nil,
                    Value::str(format!("{name}: is read only")),
                ]);
            }
            guard.unset_var(&name);
        }
        // **The answer is not discarded.** `set_var` reports `false` for a readonly name and for one
        // that cannot be represented, having already said which on stderr; returning `true`
        // regardless is how a config comes to believe a write happened that did not.
        if guard.set_var(&name, &value, export) {
            match secret {
                Some(true) => guard.mark_sensitive(&name),
                Some(false) => guard.clear_sensitive(&name),
                None => guard.mark_if_handed_out(&name, &value),
            }
            Ok(vec![Value::Bool(true)])
        } else {
            Ok(vec![
                Value::Nil,
                Value::str(format!("{name}: could not be set")),
            ])
        }
    });

    // oslo.env.unset(name) -> true. The other half of set_var; without it a script could create a
    // variable and never remove one.
    let env_unset = Arc::clone(env);
    put(oslo, "unset", move |_, args| {
        let name = text(&args, 1, "oslo.env.unset")?;
        borrow_env(&env_unset)?.unset_var(&name);
        Ok(vec![Value::Bool(true)])
    });

    // oslo.env.all() -> { NAME = value, … }, the exported environment as one table.
    // oslo.env.all{ exported = false } -> every variable, shell-local ones included.
    //
    // `get_var` answers one name at a time, which cannot express "what is set?" — a script could
    // not iterate the environment, filter it, or copy it. The default is exported names, because
    // those are what a child process would see and that is usually the question. `false` is for the
    // caller asking what *this shell* holds — and until `set{ export = false }` existed, there was
    // nothing shell-local for it to find.
    let env_all = Arc::clone(env);
    put(oslo, "all", move |_, args| {
        let exported_only = match args.first() {
            Some(Value::Table(options)) => !matches!(
                options.borrow().get_str("exported"),
                Value::Bool(false) | Value::Nil
            ),
            _ => true,
        };
        let guard = borrow_env(&env_all)?;
        let mut table = Table::new();
        if exported_only {
            for (name, value) in guard.exported_vars() {
                table.set(Value::str(name), Value::str(value));
            }
        } else {
            for (name, value, _) in guard.all_vars() {
                table.set(Value::str(name), Value::str(value));
            }
        }
        Ok(vec![Value::table(table)])
    });

    // oslo.env.snapshot() -> { { name = …, value = …, exported = … }, … }
    //
    // Every variable with the flag it carries, which `all` cannot express: it answers a name-to-value
    // map, and the export flag is the third fact. A list of records rather than a map because that
    // is the shape `oslo.rows.*` and the structured verbs already speak.
    //
    // **There is deliberately no `restore`.** Putting a snapshot back means also *removing* what is
    // set now and absent from it, and doing that across a `cd` would fight the directory
    // environment's own undo record — two mechanisms describing the same variables with no agreed
    // order. `PWD` and `OLDPWD` alone would move the shell. Undoing a directory's work is
    // `.env.lua`'s business and it already has one.
    let env_snapshot = Arc::clone(env);
    put(oslo, "snapshot", move |_, _| {
        let guard = borrow_env(&env_snapshot)?;
        Ok(vec![list(guard.all_vars().into_iter().map(
            |(name, value, exported)| {
                record(vec![
                    ("name", Value::str(name)),
                    ("value", Value::str(value)),
                    ("exported", Value::Bool(exported)),
                ])
            },
        ))])
    });

    // `$PATH` and its relatives as the lists they are. See [`super::envlist`].
    envlist::install(oslo, env);
}
