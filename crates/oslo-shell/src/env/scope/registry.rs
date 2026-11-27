//! The builtin registry: the one table that decides what a builtin is.
//!
//! Three lists used to disagree — the `matches!` in `Environment::is_builtin`, a hardcoded
//! `match` in the dispatcher, and the map behind [`Environment::register_custom_builtin`] — so
//! a name could be a builtin to `type` and not to the dispatcher, or dispatch to a hardcoded
//! function while the map held a different one (PLAN R5.6, R9.8). Registration is now the only
//! way in: [`crate::env::builtins::register_default_builtins`] fills this table and
//! `is_builtin`, the dispatcher in [`crate::exec::simple`] and `type` all read it back. Adding a
//! builtin is one `register_custom_builtin` call and nothing else.

use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub type BuiltinFn = fn(&mut Environment, &[String]) -> Result<i32>;

/// A builtin supplied at run time — today only by `oslo.register_builtin` from Lua.
///
/// It has to be a closure rather than a [`BuiltinFn`], because the thing it has to remember (a
/// key into the Lua registry) is not knowable when this crate is compiled. That is the whole of
/// PLAN R9.8: `register_builtin` used to accept the callback, throw it away, register the stub
/// `|_, _| Ok(0)` and *still* put the name in this table, so `oslo.register_builtin('ls', …)`
/// turned `ls /` into a command that printed nothing and exited 0.
type DynBuiltin = Arc<dyn Fn(&mut Environment, &[String]) -> Result<i32> + Send + Sync>;

/// How a registered name is implemented.
///
/// Two variants rather than boxing everything: every builtin oslo ships is a plain function, and
/// making the common path pay for an allocation and a virtual call to accommodate the rare
/// script-registered one would be the wrong trade.
enum Builtin {
    Native(BuiltinFn),
    Dynamic(DynBuiltin),
}

/// The POSIX "special built-in utilities" (XCU 2.14).
///
/// Listed by name rather than flagged at registration, because being special is something POSIX
/// says about the *name*, not about the implementation: `exec` is special whoever provides it,
/// and a Lua-registered builtin called `set` would not stop `set` being special. Keeping the
/// list here also means a registration site in another module cannot forget the flag.
///
/// Two consequences POSIX attaches to the name: a special builtin is found *before* shell
/// functions during command search, and its failure is fatal to a non-interactive shell.
/// [`crate::exec::simple`] owns the first; `exec::simple::posix::resolve_builtin_result` owns the
/// second, and a builtin opts into it by answering `ShellError::utility_error` rather than a
/// status. `.`, `unset`, `eval` and `export` do.
const SPECIAL_BUILTINS: &[&str] = &[
    ":", ".", "break", "continue", "eval", "exec", "exit", "export", "readonly", "return", "set",
    "shift", "times", "trap", "unset",
];

/// Whether `name` is a POSIX special builtin. True even for names oslo does not implement yet,
/// so that adding one does not silently change its search order.
pub fn is_special_builtin(name: &str) -> bool {
    SPECIAL_BUILTINS.contains(&name.trim())
}

/// Name → implementation, for every builtin this shell has.
#[derive(Default)]
pub struct BuiltinRegistry {
    table: HashMap<String, Builtin>,
    /// Names `enable -n` turned off: still registered, invisible to dispatch until re-enabled.
    disabled: HashSet<String>,
}

impl BuiltinRegistry {
    /// Add or replace a builtin. Last registration wins, which is what lets a script-supplied
    /// builtin override a default one.
    pub fn register(&mut self, name: &str, func: BuiltinFn) {
        self.table
            .insert(name.trim().to_string(), Builtin::Native(func));
    }

    /// Remove a builtin, answering whether there was one.
    ///
    /// For a builtin that exists only for the length of something — a plugin being unloaded, a
    /// name a `.env.lua` registered and the directory it belongs to being left. Registration stays
    /// the single way in, so what this removes is exactly what `type`, completion and the
    /// dispatcher stop seeing.
    pub fn unregister(&mut self, name: &str) -> bool {
        self.table.remove(name.trim()).is_some()
    }

    fn register_dynamic(&mut self, name: &str, func: DynBuiltin) {
        self.table
            .insert(name.trim().to_string(), Builtin::Dynamic(func));
    }

    /// The implementation of `name`, if it is a builtin at all.
    ///
    /// Trimmed on the way in: the command word reaching the dispatcher can carry whitespace the
    /// lexer did not strip, and answering "not a builtin" for `" echo"` sent it to PATH instead.
    ///
    /// A dynamic builtin cannot *be* a `fn` pointer, so it is answered with
    /// [`invoke_dynamic_builtin`], which finds the closure again from argv[0]. Every caller of
    /// this shell's builtins passes the builtin's own name as argv[0] — the dispatcher in
    /// `exec::simple`, `command` and `builtin` all do, and `builtin` shifts its arguments
    /// specifically to preserve it — so the name is always recoverable.
    pub fn lookup(&self, name: &str) -> Option<BuiltinFn> {
        match self.entry(name)? {
            Builtin::Native(func) => Some(*func),
            Builtin::Dynamic(_) => Some(invoke_dynamic_builtin),
        }
    }

    /// The registered implementation of `name`, unless a **feature** that provides it is off.
    ///
    /// **The one place a builtin is turned off**, so that a disabled one is invisible to the
    /// dispatcher, to `type`, to `command -v` and to completion at once rather than in four places
    /// that can disagree with each other. Nothing is removed from the table: the bit is a mask, so
    /// turning the feature back on restores the builtin exactly as it was registered.
    ///
    /// The word then falls through to `$PATH` like any other — the same route `\direnv` already
    /// takes; see `exec::simple::escape`. Written for `direnv` specifically, so that an `.envrc`
    /// directory could be handed to the real one, and that is once again exactly what it is for:
    /// oslo reads `.env.lua` and leaves `.envrc` to direnv, so a configuration that turns the
    /// `direnv` feature off in an `.envrc` directory gets the real binary there and oslo's builtin
    /// everywhere else. See `oslo.feature.when`.
    fn entry(&self, name: &str) -> Option<&Builtin> {
        if oslo_base::feature::builtin_is_off(name) || self.disabled.contains(name.trim()) {
            return None;
        }
        self.table.get(name.trim())
    }

    /// `enable name` / `enable -n name`; `false` when `name` was never registered.
    pub fn set_enabled(&mut self, name: &str, on: bool) -> bool {
        let name = name.trim();
        if !self.table.contains_key(name) || oslo_base::feature::builtin_is_off(name) {
            return false;
        }
        match on {
            true => self.disabled.remove(name),
            false => self.disabled.insert(name.to_string()),
        };
        true
    }

    /// Every registered name a feature has not turned off, with whether `enable` left it on.
    pub fn states(&self) -> impl Iterator<Item = (&str, bool)> {
        self.table
            .keys()
            .map(String::as_str)
            .filter(|name| !oslo_base::feature::builtin_is_off(name))
            .map(|name| (name, !self.disabled.contains(name)))
    }

    fn dynamic(&self, name: &str) -> Option<DynBuiltin> {
        match self.entry(name)? {
            Builtin::Dynamic(func) => Some(Arc::clone(func)),
            Builtin::Native(_) => None,
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entry(name).is_some()
    }

    /// Every builtin this shell currently has. Filtered by the same mask, so a name completion
    /// offers is a name that will run.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.table
            .keys()
            .map(String::as_str)
            .filter(|name| !oslo_base::feature::builtin_is_off(name))
    }
}

impl Environment {
    /// Register a builtin whose implementation is a closure rather than a plain function.
    ///
    /// The entry point for `oslo.register_builtin`. Generic rather than taking a boxed closure so
    /// that no private type appears in a public signature; the argument is stored behind an `Arc`
    /// and shared, so registering the same closure twice is cheap.
    ///
    /// `args[0]` is the builtin's own name, as it is for every other builtin — the dispatcher
    /// relies on that to find this closure again.
    pub fn register_dynamic_builtin<F>(&mut self, name: &str, func: F)
    where
        F: Fn(&mut Environment, &[String]) -> Result<i32> + Send + Sync + 'static,
    {
        self.builtins.register_dynamic(name, Arc::new(func));
    }
}

/// The `fn` pointer stand-in for every dynamic builtin.
///
/// [`BuiltinRegistry::lookup`] must answer with something callable as a bare function, so it
/// answers with this and the closure is recovered here from argv[0]. The `Arc` is cloned out
/// before the call because the closure needs `&mut Environment`, which it cannot have while the
/// registry it lives in is still borrowed.
fn invoke_dynamic_builtin(env: &mut Environment, args: &[String]) -> Result<i32> {
    let name = args.first().map(String::as_str).unwrap_or_default();
    match env.builtins.dynamic(name) {
        Some(func) => func(env, args),
        // Reachable only if a caller dispatched with an argv[0] that is not the builtin's name.
        // Loud and non-zero: the mistake this whole item exists to undo was a silent `Ok(0)`.
        None => {
            eprintln!(
                "{}{}: registered builtin could not be resolved",
                env.origin(),
                name
            );
            Ok(127)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_special_builtin;
    use crate::env::Environment;
    use std::sync::{Arc, Mutex};

    /// The registry is the source of truth only if everything that used to be hardcoded is in
    /// it. These are the names the old `matches!` in `is_builtin` listed, plus the three it
    /// omitted (`set`, `shift`, `type`) that the map already had — one list now, so a name
    /// cannot be a builtin to one caller and not to another.
    #[test]
    fn every_default_builtin_is_registered_and_dispatchable() {
        let env = Environment::new();
        for name in [
            "cd", "pwd", "echo", "export", "unset", "set", "shift", "exit", "break", "continue",
            "return", "alias", "unalias", "type", "eval", "source", ".", "read", "local", "pushd",
            "popd", "dirs", "readonly", "test", "[", "[[", "trap", "umask", "wait", "kill", "true",
            "false", "nav",
        ] {
            assert!(env.is_builtin(name), "{name} should be a builtin");
            assert!(
                env.get_builtin(name).is_some(),
                "{name} answers is_builtin but has no implementation to dispatch to"
            );
        }
    }

    /// `is_builtin` and `builtin_names` must agree in both directions, or completion and `type`
    /// go on disagreeing with the dispatcher the way they did before R5.6.
    #[test]
    fn listed_names_are_exactly_the_dispatchable_ones() {
        let _feature = oslo_base::feature::TEST_STATE
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let env = Environment::new();
        let names: Vec<String> = env.builtin_names().map(str::to_string).collect();
        assert!(!names.is_empty());
        for name in &names {
            assert!(env.is_builtin(name), "{name} is listed but not a builtin");
        }
    }

    #[test]
    fn a_leading_space_still_names_a_builtin() {
        let env = Environment::new();
        assert!(env.is_builtin(" echo"));
        assert!(env.get_builtin("echo ").is_some());
    }

    #[test]
    fn special_builtins_are_the_posix_set() {
        for special in [
            ":", ".", "eval", "exec", "exit", "export", "set", "trap", "unset",
        ] {
            assert!(is_special_builtin(special), "{special} is special in POSIX");
        }
        // Regular builtins: a function is allowed to shadow these even in POSIX mode.
        for regular in [
            "cd", "echo", "read", "test", "[", "true", "umask", "wait", "kill",
        ] {
            assert!(!is_special_builtin(regular), "{regular} is not special");
        }
        assert!(!is_special_builtin("ls"));
    }

    /// PLAN R9.8. A closure-backed builtin has to be reachable the same way a native one is:
    /// through `is_builtin`, `get_builtin` and — the part that used to be missing — an
    /// `exec_custom_builtin` that actually runs the thing that was registered.
    #[test]
    fn a_dynamic_builtin_runs_and_returns_its_status() {
        let mut env = Environment::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        env.register_dynamic_builtin("greet", move |_env, args| {
            recorder.lock().unwrap().extend(args.iter().cloned());
            Ok(7)
        });

        assert!(env.is_builtin("greet"));
        assert!(env.get_builtin("greet").is_some());
        let status = env
            .exec_custom_builtin("greet", &["greet".into(), "world".into()])
            .expect("registered builtin must dispatch");
        assert_eq!(status.unwrap(), 7);
        // argv[0] is the builtin's own name, exactly as a native builtin receives it.
        assert_eq!(*seen.lock().unwrap(), vec!["greet", "world"]);
    }

    /// The saboteur this item removes: registering a name used to insert it into the table with
    /// a `|_, _| Ok(0)` body, so `ls /` became a silent success. Whatever is registered now is
    /// what runs, and a registration that replaces a native builtin replaces its behaviour too.
    #[test]
    fn a_dynamic_builtin_overrides_the_native_one_of_the_same_name() {
        let mut env = Environment::new();
        assert_eq!(
            env.exec_custom_builtin("true", &["true".into()])
                .unwrap()
                .unwrap(),
            0
        );
        env.register_dynamic_builtin("true", |_env, _args| Ok(42));
        assert_eq!(
            env.exec_custom_builtin("true", &["true".into()])
                .unwrap()
                .unwrap(),
            42
        );
        // …and back again: a later native registration wins in turn.
        env.register_custom_builtin("true", |_, _| Ok(0));
        assert_eq!(
            env.exec_custom_builtin("true", &["true".into()])
                .unwrap()
                .unwrap(),
            0
        );
    }

    /// `builtin greet` and `command greet` reach the registry directly, so they must reach the
    /// closure too rather than the `fn`-pointer stand-in that stands for it.
    #[test]
    fn a_dynamic_builtin_is_reachable_through_the_builtin_builtin() {
        let mut env = Environment::new();
        env.register_dynamic_builtin("greet", |_env, args| Ok(args.len() as i32));
        let argv: Vec<String> = ["builtin", "greet", "a", "b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            crate::env::builtins::builtin_builtin(&mut env, &argv).unwrap(),
            3
        );
    }

    /// An error out of the closure is the builtin's error, not a swallowed success.
    #[test]
    fn a_failing_dynamic_builtin_propagates_its_error() {
        let mut env = Environment::new();
        env.register_dynamic_builtin("boom", |_env, _args| {
            Err(oslo_base::error::ShellError::ExecutionError("boom".into()))
        });
        let err = env
            .exec_custom_builtin("boom", &["boom".into()])
            .unwrap()
            .unwrap_err();
        assert!(err.to_string().contains("boom"));
    }
}
