//! What a builtin returns when it wants to change something.
//!
//! ```lua
//! oslo.register_builtin{ name = "use", run = function(argv, shell)
//!   return { status = 0, export = { PROFILE = argv[2] }, cd = "/srv/" .. argv[2] }
//! end }
//! ```
//!
//! # Why a returned table rather than calls
//!
//! A builtin runs while the shell holds its own state, so `oslo.env.set` inside one raises and a
//! plain `X = "1"` is a `try_lock` that **silently drops the write**. The frame above holds a live
//! `&mut Environment`; the honest shape is therefore to *tell* it rather than to ask, which turns a
//! wall into a question about write ordering.
//!
//! # Parse is total, and separate from apply
//!
//! [`parse`] touches no `Environment` and rejects everything it does not understand — an unknown
//! key, a wrong type, a `cd` where one is not allowed — so a malformed return applies **nothing**.
//! An effect that is well-formed and *refused at runtime* (a readonly variable) is different, and
//! is reported by the shell's own machinery rather than a second time here.

use oslo_base::value::{LuaError, LuaResult, Table, Value};
use oslo_shell::env::Environment;

/// Which effects the caller is allowed to ask for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Allow {
    /// A registered builtin: everything.
    All,
    /// `pre-change-dir`, where a `cd` effect would re-enter the directory change it is answering
    /// about. Refused at parse rather than ignored, which is the whole point of this module.
    NoCd,
}

/// One returned table, read.
#[derive(Default, Debug)]
pub(crate) struct Effects {
    status: Option<Value>,
    set: Vec<(String, String)>,
    export: Vec<(String, String)>,
    unset: Vec<String>,
    alias: Vec<(String, Option<String>)>,
    positional: Option<Vec<String>>,
    cd: Option<String>,
    /// `pre-change-dir` only.
    pub(crate) refuse: bool,
}

/// Every key a returned table may carry, for the message when one is misspelt.
const KEYS: &[&str] = &[
    "status",
    "set",
    "export",
    "unset",
    "alias",
    "positional",
    "cd",
    "refuse",
];

/// Read a returned table. Nothing is applied if anything is malformed.
pub(crate) fn parse(table: &Table, allow: Allow, owner: &str) -> LuaResult<Effects> {
    let mut it = Effects::default();
    for (key, value) in table.pairs() {
        let Value::Str(name) = &key else {
            return Err(LuaError::new(format!(
                "{owner}: a returned table's keys must be strings"
            )));
        };
        let name = name.to_string();
        // **An unknown key raises rather than being ignored.** A silently dropped `setenv = {…}` is
        // the exact class of bug this whole contract exists to close.
        if !KEYS.contains(&name.as_str()) {
            return Err(LuaError::new(format!(
                "{owner}: `{name}` is not an effect; they are {}",
                KEYS.join(", ")
            )));
        }
        match name.as_str() {
            "status" => it.status = Some(value),
            "set" => it.set = pairs(&value, owner, "set")?,
            "export" => it.export = pairs(&value, owner, "export")?,
            "unset" => it.unset = names(&value, owner, "unset")?,
            "alias" => it.alias = aliases(&value, owner)?,
            "positional" => it.positional = Some(names(&value, owner, "positional")?),
            "cd" => {
                if allow == Allow::NoCd {
                    return Err(LuaError::new(format!(
                        "{owner}: `cd` cannot be returned from here — it would re-enter the \
                         directory change this is answering about"
                    )));
                }
                it.cd = Some(word(&value, owner, "cd")?);
            }
            "refuse" => it.refuse = value.truthy(),
            _ => unreachable!("checked above"),
        }
    }
    Ok(it)
}

impl Effects {
    /// Apply in the documented order and answer the status.
    ///
    /// **The order is not arbitrary.** `unset` before `set`, because the reverse silently discards a
    /// name written in both. `set` before `export`, so a name in both ends up exported, which is
    /// what writing it in both plainly means. `cd` last, because it fires the change-directory
    /// hooks — which should see the variables this builtin just set — and because it writes `PWD`
    /// itself, so running it earlier would let a stray `set = { PWD = … }` leave `pwd` lying.
    ///
    /// **No summary diagnostic.** Every applier already reports its own refusal: `set_var` prints
    /// `is read only`, and `cd` reports through its own failure path. A second sentence saying the
    /// same thing is noise.
    pub(crate) fn apply(self, env: &mut Environment) -> i32 {
        let mut all_applied = true;
        for name in &self.unset {
            env.unset_var(name);
        }
        for (name, value) in &self.set {
            // Shell-local, deliberately the opposite of the old `oslo.env.set` default: a builtin
            // that wants a child to see it says `export`.
            all_applied &= set(env, name, value, false);
        }
        for (name, value) in &self.export {
            all_applied &= set(env, name, value, true);
        }
        for (name, body) in &self.alias {
            match body {
                Some(body) => env.set_alias(name, body),
                // `false` removes, which reads as the opposite of setting one.
                None => {
                    env.remove_alias(name);
                }
            }
        }
        if let Some(positional) = self.positional {
            env.set_positional(positional);
        }
        if let Some(path) = &self.cd {
            // Through the builtin, so `PWD`, `OLDPWD`, the directory ring and the hooks all agree.
            let moved =
                oslo_shell::env::builtins::builtin_cd(env, &["cd".to_string(), path.to_string()]);
            all_applied &= matches!(moved, Ok(0));
        }
        // **An explicit status is the script's word; an absent one is the shell's.** A builtin that
        // says `status = 0` after choosing to try a readonly write is expressing intent, and is
        // believed. One that says nothing gets 1 if any effect was refused.
        match &self.status {
            Some(value) => super::status::from_lua(Some(value)),
            None if all_applied => 0,
            None => 1,
        }
    }
}

/// One variable, marked sensitive when its value is one `oslo.secret.*` handed out.
fn set(env: &mut Environment, name: &str, value: &str, export: bool) -> bool {
    let applied = env.set_var(name, value, export);
    if applied {
        env.mark_if_handed_out(name, value);
    }
    applied
}

/// `{ NAME = "value" }` — a map of names to words.
fn pairs(value: &Value, owner: &str, field: &str) -> LuaResult<Vec<(String, String)>> {
    let Value::Table(table) = value else {
        return Err(LuaError::new(format!(
            "{owner}: `{field}` must be a table of names to values"
        )));
    };
    let table = table.borrow();
    let mut out = Vec::new();
    for (key, value) in table.pairs() {
        let Value::Str(name) = &key else {
            return Err(LuaError::new(format!(
                "{owner}: `{field}` must be keyed by name"
            )));
        };
        out.push((name.to_string(), word(&value, owner, field)?));
    }
    Ok(out)
}

/// `{ "A", "B" }` — a list of words.
fn names(value: &Value, owner: &str, field: &str) -> LuaResult<Vec<String>> {
    let Value::Table(table) = value else {
        return Err(LuaError::new(format!(
            "{owner}: `{field}` must be a list of names"
        )));
    };
    let table = table.borrow();
    let mut out = Vec::new();
    let mut at = 1;
    loop {
        match table.get(&Value::int(at)) {
            Value::Nil => break,
            entry => out.push(word(&entry, owner, field)?),
        }
        at += 1;
    }
    Ok(out)
}

/// `{ g = "git", ll = false }` — set, or remove with `false`.
fn aliases(value: &Value, owner: &str) -> LuaResult<Vec<(String, Option<String>)>> {
    let Value::Table(table) = value else {
        return Err(LuaError::new(format!(
            "{owner}: `alias` must be a table of names to bodies"
        )));
    };
    let table = table.borrow();
    let mut out = Vec::new();
    for (key, value) in table.pairs() {
        let Value::Str(name) = &key else {
            return Err(LuaError::new(format!(
                "{owner}: `alias` must be keyed by name"
            )));
        };
        let body = match value {
            Value::Bool(false) => None,
            other => Some(word(&other, owner, "alias")?),
        };
        out.push((name.to_string(), body));
    }
    Ok(out)
}

/// A string or a number, which is what a shell word is. Anything else is a mistake at the call site.
fn word(value: &Value, owner: &str, field: &str) -> LuaResult<String> {
    match value {
        Value::Str(s) => Ok(s.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(LuaError::new(format!(
            "{owner}: `{field}` holds a {}, which is not a word",
            other.type_name()
        ))),
    }
}

#[cfg(test)]
#[path = "effects/tests.rs"]
mod tests;
