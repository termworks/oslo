//! The names the shell knows besides variables: aliases, functions and builtins.
//!
//! A second `impl Environment` in a child module, which sees the private fields because it is a
//! descendant of the one that declares them. Split out because `scope.rs` crossed the 600-line
//! limit and this is the seam that means something: everything here answers "what does this word
//! resolve to", where the rest of `scope.rs` answers "what state is this shell in".

use super::*;

impl Environment {
    pub fn set_alias(&mut self, name: &str, value: &str) {
        self.aliases.insert(name.to_string(), value.to_string());
    }

    pub fn get_alias(&self, name: &str) -> Option<&str> {
        self.aliases.get(name).map(|s| s.as_str())
    }

    pub fn get_aliases(&self) -> &HashMap<String, String> {
        &self.aliases
    }

    pub fn remove_alias(&mut self, name: &str) {
        self.aliases.remove(name);
    }

    /// Hand the shell a variable it has not been asked for yet: the name, and the line that
    /// produces the value. Run by `expand::param::materialise`, the first time the name is read.
    pub fn set_lazy_var(&mut self, name: &str, body: &str) {
        self.lazy.insert(name.to_string(), body.to_string());
    }

    pub fn remove_lazy_var(&mut self, name: &str) {
        self.lazy.remove(name);
    }

    /// The recipe for `name`, **removed as it is handed over**.
    ///
    /// Taken rather than read, because running it is the next thing the caller does and a body that
    /// mentions its own name would otherwise expand itself for ever. Gone means "already asked
    /// for": the second read of `$GITHUB_TOKEN` finds an ordinary variable, which is the point.
    pub fn take_lazy_var(&mut self, name: &str) -> Option<String> {
        self.lazy.remove(name)
    }

    /// The names still waiting to be asked for, sorted — for anything that lists what a shell has.
    pub fn lazy_var_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.lazy.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Define `name`, keeping an `export -f` mark and handing children the new definition.
    pub fn set_function(&mut self, name: &str, body: Command) {
        self.functions.insert(name.to_string(), Arc::new(body));
        let file = self.defining_file();
        self.function_files.insert(name.to_string(), file);
        self.function_sources.remove(name);
        self.publish_function(name);
    }

    /// Remember the text `name` was defined with, for printing it back as bash does.
    pub fn set_function_source(&mut self, name: &str, source: Option<Arc<str>>) {
        match source {
            Some(text) => self.function_sources.insert(name.to_string(), text),
            None => self.function_sources.remove(name),
        };
        self.publish_function(name);
    }

    /// The definition of `name` as `declare -f` prints it: bash's layout from the source text
    /// when it is known, else rebuilt from the tree.
    pub fn function_definition(&self, name: &str) -> Option<String> {
        let body = self.get_function(name)?;
        let printed = self
            .function_sources
            .get(name)
            .and_then(|text| crate::syntax::print::function_text(text, name, true));
        Some(printed.unwrap_or_else(|| crate::env::builtins::format_function(name, body)))
    }

    /// The body of `name` as bash exports it in `BASH_FUNC_name%%`: `() { …}` on few lines.
    pub fn function_export_text(&self, name: &str) -> Option<String> {
        let text = self.function_sources.get(name)?;
        crate::syntax::print::function_text(text, "", false)
    }

    /// Forget the function `name`; `true` if there was one.
    ///
    /// `unset -f` is the only caller, and without this it had nothing to call: the functions
    /// table was exposed read-only, so a function could be defined and never taken back.
    /// The answer is not `unset`'s exit status — bash exits 0 for `unset -f nosuchfn` — but it
    /// is what a caller that needs to know would ask for.
    pub fn remove_function(&mut self, name: &str) -> bool {
        self.function_files.remove(name);
        self.function_sources.remove(name);
        self.forget_function_attrs(name);
        self.functions.remove(name).is_some()
    }

    pub fn get_function(&self, name: &str) -> Option<&Command> {
        self.functions.get(name).map(|body| &**body)
    }

    /// The body, shared rather than copied.
    ///
    /// **What calling a function costs.** A body is a recursive tree down to its `String`s, and
    /// the caller needs it owned because running it borrows the environment mutably — so every
    /// call used to deep-copy the entire function. A sixty-line helper called from a prompt is
    /// hundreds of allocations per call, for a body nothing ever mutates.
    pub fn shared_function(&self, name: &str) -> Option<Arc<Command>> {
        self.functions.get(name).map(Arc::clone)
    }

    pub fn get_functions(&self) -> &HashMap<String, Arc<Command>> {
        &self.functions
    }

    /// Register `name` as a builtin, replacing any earlier one.
    ///
    /// The single point of entry: whatever is registered here is what `is_builtin`, the
    /// dispatcher, completion and `type` will all report and run.
    pub fn register_custom_builtin(&mut self, name: &str, func: BuiltinFn) {
        self.builtins.register(name, func);
    }

    /// Take a builtin back out again, answering whether there was one.
    ///
    /// The other half of the entry point, for builtins with a lifetime shorter than the shell's.
    pub fn unregister_custom_builtin(&mut self, name: &str) -> bool {
        self.builtins.unregister(name)
    }

    /// The implementation registered for `name`, if any. Prefer [`Self::exec_custom_builtin`]:
    /// this exists for callers that need to know a builtin is callable without calling it.
    pub fn get_builtin(&self, name: &str) -> Option<BuiltinFn> {
        self.builtins.lookup(name)
    }

    /// Every registered builtin name.
    ///
    /// The single source of truth for completion and `type`, so adding a builtin does not
    /// require remembering to update a list somewhere else.
    pub fn builtin_names(&self) -> impl Iterator<Item = &str> {
        self.builtins.names()
    }

    /// Whether `name` is a builtin — that is, whether it is in the registry.
    ///
    /// This used to be a `matches!` over a hand-written list that had drifted from both the
    /// registry and the dispatcher's own `match` (PLAN R5.6): `type` called `set` an external
    /// command while the shell ran it as a builtin, and a name in the list with no
    /// implementation dispatched to `Ok(0)` — a command that silently did nothing.
    pub fn is_builtin(&self, name: &str) -> bool {
        self.builtins.contains(name)
    }

    /// Turn a registered builtin on or off, as `enable` does; `false` if there is no such builtin.
    pub fn set_builtin_enabled(&mut self, name: &str, on: bool) -> bool {
        self.builtins.set_enabled(name, on)
    }

    /// Every builtin name, sorted bytewise, with whether it is enabled — `enable`'s listing.
    pub fn builtin_states(&self) -> Vec<(String, bool)> {
        let mut states: Vec<(String, bool)> = self
            .builtins
            .states()
            .map(|(name, on)| (name.to_string(), on))
            .collect();
        states.sort();
        states
    }
}
