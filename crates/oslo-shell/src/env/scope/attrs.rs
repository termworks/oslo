//! The attributes on a binding, and the environment a child is handed because of them.
//!
//! A child's environment is the first binding in each name's chain — innermost local first, the
//! global last — that is both set and exported. That is bash's rule, and it has one surprising
//! consequence it keeps: `local E` with no value hides an exported `E` from the function but not
//! from the programs it runs.

use super::value::{Attrs, Value, Var};
use super::{Environment, environ_remove, environ_set};

impl Environment {
    /// Turn attributes on and off for whatever `name` resolves to, declaring it if it is new.
    pub fn modify_attrs(&mut self, name: &str, on: Attrs, off: Attrs) {
        let target = self.ref_target_name(name);
        self.modify_binding_attrs(&target, on, off);
    }

    /// Turn attributes on and off for the binding `name` itself, never following a nameref.
    pub fn modify_binding_attrs(&mut self, name: &str, on: Attrs, off: Attrs) {
        let was_exported = self.chain_exports(name);
        let var = self
            .vars
            .entry(name.to_string())
            .or_insert_with(|| Var::unset(Attrs::empty()));
        if on.has(Attrs::CASE) {
            var.attrs.remove(Attrs::CASE);
        }
        var.attrs.insert(on);
        var.attrs.remove(off);
        if was_exported || self.chain_exports(name) {
            self.sync_environ(name);
        }
    }

    pub fn set_readonly(&mut self, name: &str) {
        self.modify_attrs(name, Attrs::READONLY, Attrs::empty());
    }

    /// Lift the read-only mark from a variable the shell itself rewrites, such as `SHELLOPTS`.
    pub(super) fn release_readonly(&mut self, name: &str) {
        if self.vars.contains_key(name) {
            self.modify_binding_attrs(name, Attrs::empty(), Attrs::READONLY);
        }
    }

    /// Whether assigning to `name` would be refused, following a nameref to what it names.
    pub fn is_readonly(&self, name: &str) -> bool {
        let target = self.ref_target_name(name);
        self.vars.get(&*target).is_some_and(Var::readonly)
    }

    /// Mark `name` read-only for whichever scope owns it: a local's mark leaves with its frame.
    pub fn set_readonly_here(&mut self, name: &str) {
        self.set_readonly(name);
    }

    /// Mark `name` as holding an integer: every assignment to it is arithmetic from now on.
    pub fn set_integer(&mut self, name: &str) {
        self.modify_attrs(name, Attrs::INTEGER, Attrs::empty());
    }

    /// Whether `name` holds an integer, including the clocks and counters bash marks `-i`.
    pub fn is_integer(&self, name: &str) -> bool {
        let target = self.ref_target_name(name);
        self.vars
            .get(&*target)
            .is_some_and(|v| v.attrs.has(Attrs::INTEGER))
            || crate::env::dynamic::is_integer(&target)
    }

    /// Drop the attribute, for `declare +i`.
    pub fn clear_integer(&mut self, name: &str) {
        self.modify_attrs(name, Attrs::empty(), Attrs::INTEGER);
    }

    /// The bindings of `name` from the innermost to the global one.
    fn chain<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Var> + 'a {
        let shadowed = self
            .scope_stack
            .iter()
            .rev()
            .filter_map(move |frame| frame.saved.get(name))
            .map_while(Option::as_ref);
        self.vars.get(name).into_iter().chain(shadowed)
    }

    /// The value a child would see for `name`, if any.
    fn export_value<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        let var = self.chain(name).find(|v| v.is_set() && v.exported())?;
        match &var.value {
            Value::Scalar(text) => Some(text),
            _ => None,
        }
    }

    /// Whether any binding of `name` carries the export attribute.
    pub(super) fn chain_exports(&self, name: &str) -> bool {
        self.chain(name).any(Var::exported)
    }

    /// Bring the process environment into line with `name`'s bindings.
    pub(super) fn sync_environ(&self, name: &str) {
        match self.export_value(name) {
            Some(value) => environ_set(name, value),
            None => environ_remove(name),
        }
    }
}
