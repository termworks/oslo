//! Dynamic scoping: the frames a function call, `local` and a command prefix push and pop.
//!
//! The variable table always holds the binding each name resolves to *now*. A frame holds what it
//! shadowed — the binding every name it localised had before — so popping restores rather than
//! merges, and a callee sees its caller's locals until it declares its own.
//!
//! Following one name down through the frames that saved it gives every binding of that name from
//! the innermost to the global one. `unset`, `declare -g` and the environment a child is handed
//! all walk that chain.

use super::Environment;
use super::value::{Attrs, Var};
use crate::env::options::ShellOptions;
use std::collections::HashMap;

/// What pushed a frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ScopeKind {
    /// A function call: the scope `local` writes to and `unset` measures against.
    Function,
    /// The assignments in front of one command, `FOO=bar cmd`.
    Temp,
}

/// One scope: the shadowed binding of every name it localised.
///
/// `None` means the name had no binding, which popping turns back into no binding.
pub(super) struct ScopeFrame {
    pub(super) kind: ScopeKind,
    pub(super) saved: HashMap<String, Option<Var>>,
    /// `local -`: the options to put back when the frame pops.
    pub(super) options: Option<ShellOptions>,
}

impl ScopeFrame {
    fn new(kind: ScopeKind) -> Self {
        ScopeFrame {
            kind,
            saved: HashMap::new(),
            options: None,
        }
    }
}

impl Environment {
    /// Begin a function's scope.
    pub fn push_scope(&mut self) {
        self.scope_stack.push(ScopeFrame::new(ScopeKind::Function));
    }

    /// Begin the scope of one command's prefix assignments.
    pub fn push_temp_scope(&mut self) {
        self.scope_stack.push(ScopeFrame::new(ScopeKind::Temp));
    }

    /// End a prefix assignment's scope keeping what it bound, as a special builtin in POSIX mode
    /// does.
    pub fn keep_temp_scope(&mut self) {
        if self
            .scope_stack
            .last()
            .is_some_and(|f| f.kind == ScopeKind::Temp)
        {
            self.scope_stack.pop();
        }
    }

    /// Keep `name`'s binding past the prefix assignment's scope that made it: `x=1 export x`
    /// leaves `x` set, as bash does when an attribute is given to such a variable.
    pub(super) fn adopt_temp_binding(&mut self, name: &str) {
        if let Some(at) = self.owner_frame(name)
            && self.scope_stack[at].kind == ScopeKind::Temp
        {
            self.scope_stack[at].saved.remove(name);
        }
    }

    pub fn pop_scope(&mut self) {
        let Some(frame) = self.scope_stack.pop() else {
            return;
        };
        if let Some(options) = frame.options {
            self.set_options(options);
        }
        for (name, original) in frame.saved {
            let was_exported = self.chain_exports(&name);
            match original {
                Some(var) => {
                    self.vars.insert(name.clone(), var);
                }
                None => {
                    self.vars.remove(&name);
                }
            }
            if was_exported || self.chain_exports(&name) {
                self.sync_environ(&name);
            }
        }
    }

    /// `local -`: put the current options back when the innermost function returns.
    pub fn save_options_locally(&mut self) {
        let options = self.options();
        if let Some(frame) = self.function_frame_mut() {
            frame.options.get_or_insert(options);
        }
    }

    fn function_frame_mut(&mut self) -> Option<&mut ScopeFrame> {
        self.scope_stack
            .iter_mut()
            .rev()
            .find(|f| f.kind == ScopeKind::Function)
    }

    /// Index of the frame that owns `name`'s current binding: the innermost one that saved it.
    pub(super) fn owner_frame(&self, name: &str) -> Option<usize> {
        self.scope_stack
            .iter()
            .rposition(|frame| frame.saved.contains_key(name))
    }

    /// Whether `name` is a local of the innermost function now running.
    pub fn is_local_here(&self, name: &str) -> bool {
        let here = self
            .scope_stack
            .iter()
            .rposition(|f| f.kind == ScopeKind::Function);
        here.is_some() && self.owner_frame(name) == here
    }

    /// Remember `name`'s current binding in the innermost frame so popping restores it.
    pub(super) fn save_for_restore(&mut self, name: &str) {
        let Some(frame) = self.scope_stack.last() else {
            return;
        };
        if frame.saved.contains_key(name) {
            return;
        }
        let current = self.vars.get(name).cloned();
        if let Some(frame) = self.scope_stack.last_mut() {
            frame.saved.insert(name.to_string(), current);
        }
    }

    /// Make `name` a local of the innermost frame, with no value unless one is inherited.
    ///
    /// The new binding takes the export attribute of the one it shadows, and a binding a command
    /// prefix made (`x=4 f` with `local x` inside) gives it its value too — both as bash does.
    /// Answers `false` without doing anything for a name that is read-only, and does nothing at
    /// all outside every frame.
    pub fn make_local(&mut self, name: &str) -> bool {
        let Some(innermost) = self.scope_stack.len().checked_sub(1) else {
            return true;
        };
        // `v=1 local v=2`: the local belongs to the function, not to the prefix's frame above it.
        let target = self
            .scope_stack
            .iter()
            .rposition(|f| f.kind == ScopeKind::Function)
            .unwrap_or(innermost);
        if self.scope_stack[target].saved.contains_key(name) {
            return true;
        }
        let shadowed = self.vars.get(name);
        if shadowed.is_some_and(Var::readonly) {
            return false;
        }
        let from_prefix = self
            .owner_frame(name)
            .is_some_and(|i| self.scope_stack[i].kind == ScopeKind::Temp);
        let local = match shadowed {
            // `shopt -s localvar_inherit`: a copy of what it shadows, all but the nameref mark.
            Some(outer) if crate::env::builtins::shopt::on("localvar_inherit") => {
                let mut copy = outer.clone();
                copy.attrs.remove(Attrs::NAMEREF);
                copy
            }
            Some(outer) if from_prefix && outer.is_set() => {
                Var::scalar(outer.text().unwrap_or_default(), outer.attrs)
            }
            Some(outer) if outer.exported() => Var::unset(Attrs::EXPORT),
            _ => Var::unset(Attrs::empty()),
        };
        // What the prefix frames above shadowed is what the function's frame restores.
        let mut below = None;
        for frame in &mut self.scope_stack[target + 1..] {
            if let Some(saved) = frame.saved.remove(name) {
                below.get_or_insert(saved);
            }
        }
        let original = below.unwrap_or_else(|| self.vars.get(name).cloned());
        self.scope_stack[target]
            .saved
            .insert(name.to_string(), original);
        self.vars.insert(name.to_string(), local);
        true
    }

    /// Whether a local is hiding `name`'s global binding.
    pub fn global_is_hidden(&self, name: &str) -> bool {
        self.scope_stack
            .iter()
            .any(|frame| frame.saved.contains_key(name))
    }

    /// Run `f` with `name`'s global binding standing in for whatever local hides it: `declare -g`.
    ///
    /// The global is moved into the table for the duration and moved back afterwards, so every
    /// ordinary operation on the table works on it unchanged.
    pub fn with_global_binding<R>(&mut self, name: &str, f: impl FnOnce(&mut Self) -> R) -> R {
        let Some(outermost) = self
            .scope_stack
            .iter()
            .position(|frame| frame.saved.contains_key(name))
        else {
            return f(self);
        };
        let local = self.vars.remove(name);
        let global = self.scope_stack[outermost]
            .saved
            .get_mut(name)
            .and_then(Option::take);
        if let Some(var) = global {
            self.vars.insert(name.to_string(), var);
        }
        let result = f(self);
        let global = self.vars.remove(name);
        if let Some(slot) = self.scope_stack[outermost].saved.get_mut(name) {
            *slot = global;
        }
        if let Some(var) = local {
            self.vars.insert(name.to_string(), var);
        }
        self.sync_environ(name);
        result
    }

    /// Remove `name` the way `unset` does.
    ///
    /// A local of the function now running stays a local, with no value and no attributes, until
    /// it is assigned again or the function returns; under `shopt -s localvar_unset` so does a
    /// caller's. Any other binding is removed outright, which reveals whatever it was shadowing —
    /// a caller's local, or the global.
    pub(super) fn unbind(&mut self, name: &str) {
        let was_exported = self.chain_exports(name);
        let callers_local = || {
            crate::env::builtins::shopt::on("localvar_unset")
                && self
                    .owner_frame(name)
                    .is_some_and(|i| self.scope_stack[i].kind == ScopeKind::Function)
        };
        if self.is_local_here(name) || callers_local() {
            self.vars
                .insert(name.to_string(), Var::unset(Attrs::empty()));
        } else {
            match self.owner_frame(name) {
                Some(owner) => {
                    let revealed = self.scope_stack[owner].saved.remove(name).flatten();
                    match revealed {
                        Some(var) => self.vars.insert(name.to_string(), var),
                        None => self.vars.remove(name),
                    };
                }
                None => {
                    self.vars.remove(name);
                }
            }
        }
        if was_exported || self.chain_exports(name) {
            self.sync_environ(name);
        }
    }

    /// Set `name` in the innermost scope. `false` if the assignment was refused.
    pub fn set_local_var(&mut self, name: &str, value: &str) -> bool {
        if super::reject_unrepresentable(&self.origin(), name, value) {
            return false;
        }
        if !self.make_local(name) {
            self.complain_readonly(name, value);
            return false;
        }
        self.set_var(name, value, false)
    }

    /// Set a variable that is exported for the lifetime of the innermost scope only.
    ///
    /// What a command prefix (`FOO=bar cmd`) needs: `cmd` sees `FOO` in its environment, and the
    /// shell does not once it returns.
    pub fn set_local_exported_var(&mut self, name: &str, value: &str) -> bool {
        if super::reject_unrepresentable(&self.origin(), name, value) {
            return false;
        }
        if self.is_readonly(name) {
            self.complain_readonly(name, value);
            return false;
        }
        self.save_for_restore(name);
        self.set_var(name, value, true)
    }

    /// Make `name` an array in the innermost scope — `local a=(…)`.
    pub fn set_local_array(&mut self, name: &str, array: super::ShellArray) -> bool {
        if !self.make_local(name) {
            self.complain_readonly(name, "");
            return false;
        }
        self.set_array(name, array)
    }
}

#[cfg(test)]
mod tests {
    use crate::env::Environment;
    use crate::env::scope::ShellArray;

    /// A local array must not outlive its frame, nor leave the scalar it shadowed behind.
    #[test]
    fn a_local_array_is_undone_by_its_frame() {
        let mut env = Environment::new();
        env.set_var("oslo_f1", "outer", false);
        env.push_scope();
        env.set_local_array("oslo_f1", ShellArray::from_values(["1", "2"]));
        assert_eq!(env.get_var("oslo_f1"), Some("1"));
        env.pop_scope();
        assert!(env.get_array("oslo_f1").is_none());
        assert_eq!(env.get_var("oslo_f1"), Some("outer"));
    }

    #[test]
    fn a_local_array_with_no_outer_value_disappears() {
        let mut env = Environment::new();
        env.push_scope();
        env.set_local_array("oslo_f2", ShellArray::from_values(["x"]));
        env.pop_scope();
        assert!(env.get_array("oslo_f2").is_none());
        assert_eq!(env.get_var("oslo_f2"), None);
    }

    /// `local x` declares `x` without giving it a value.
    #[test]
    fn a_local_without_a_value_is_unset() {
        let mut env = Environment::new();
        env.set_var("oslo_f3", "outer", false);
        env.push_scope();
        assert!(env.make_local("oslo_f3"));
        assert_eq!(env.get_var("oslo_f3"), None);
        env.pop_scope();
        assert_eq!(env.get_var("oslo_f3"), Some("outer"));
    }

    /// Unsetting a caller's local reveals what that local was hiding.
    #[test]
    fn unsetting_an_outer_local_reveals_the_global() {
        let mut env = Environment::new();
        env.set_var("oslo_f4", "global", false);
        env.push_scope();
        env.set_local_var("oslo_f4", "local");
        env.push_scope();
        env.unset_var("oslo_f4");
        assert_eq!(env.get_var("oslo_f4"), Some("global"));
        env.pop_scope();
        assert_eq!(env.get_var("oslo_f4"), Some("global"));
        env.pop_scope();
        assert_eq!(env.get_var("oslo_f4"), Some("global"));
    }

    /// Unsetting a local of the running function leaves it local and unset.
    #[test]
    fn unsetting_a_local_here_keeps_it_local() {
        let mut env = Environment::new();
        env.set_var("oslo_f5", "global", false);
        env.push_scope();
        env.set_local_var("oslo_f5", "local");
        env.unset_var("oslo_f5");
        assert_eq!(env.get_var("oslo_f5"), None);
        env.set_var("oslo_f5", "again", false);
        env.pop_scope();
        assert_eq!(env.get_var("oslo_f5"), Some("global"));
    }
}
