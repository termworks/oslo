//! The trap table: what a condition is bound to, and what a listing may report.
//!
//! Split from `scope.rs` because the two maps here answer different questions and keeping them
//! next to each other is the point — `signal_traps` is what *runs*, `inherited_traps` is what a
//! subshell may still *report*. Mixing them is what made `saved=$(trap)` come back empty.

use super::Environment;
use std::collections::HashMap;

/// The traps a function call or `source` took out on entry, to be put back when it returns.
#[must_use]
pub struct TrapFrame {
    saved: Vec<(&'static str, String)>,
}

impl Environment {
    /// Take out the traps a function body does not inherit: ERR without `-E`, DEBUG and RETURN
    /// without `-T` (and RETURN while a DEBUG handler runs).
    pub fn enter_function_traps(&mut self, debug_running: bool) -> TrapFrame {
        let mut names: Vec<&'static str> = Vec::new();
        if !self.errtrace() {
            names.push("ERR");
        }
        if !self.functrace() {
            names.push("DEBUG");
        }
        if !self.functrace() || debug_running {
            names.push("RETURN");
        }
        self.take_traps(&names)
    }

    /// Take out the DEBUG trap around `source`, unless `-T` lets the file inherit it.
    pub fn enter_source_traps(&mut self) -> TrapFrame {
        let names: &[&'static str] = if self.functrace() { &[] } else { &["DEBUG"] };
        self.take_traps(names)
    }

    fn take_traps(&mut self, names: &[&'static str]) -> TrapFrame {
        let saved = names
            .iter()
            .filter_map(|&name| self.signal_traps.remove(name).map(|action| (name, action)))
            .collect();
        TrapFrame { saved }
    }

    /// Put back each trap the frame took out, unless the body installed its own meanwhile.
    pub fn leave_trap_frame(&mut self, frame: TrapFrame) {
        for (name, action) in frame.saved {
            let replaced = self
                .signal_traps
                .get(name)
                .is_some_and(|current| current != "-");
            if !replaced {
                self.signal_traps.insert(name.to_string(), action);
            }
        }
    }

    pub fn set_trap(&mut self, sig: &str, handler: &str) {
        let key = sig.to_uppercase();
        // A trap set here supersedes whatever was inherited, so the old text must not go on being
        // listed beside the new one.
        self.inherited_traps.remove(&key);
        self.signal_traps.insert(key, handler.to_string());
    }

    /// Every trap this shell would actually run — what a subshell has none of.
    ///
    /// Not [`Self::listable_traps`], which is what `trap` *prints* and deliberately includes the
    /// ones a subshell inherited and reset.
    pub fn get_traps(&self) -> &HashMap<String, String> {
        &self.signal_traps
    }

    pub fn get_trap(&self, sig: &str) -> Option<&str> {
        self.signal_traps
            .get(&sig.to_uppercase())
            .map(|s| s.as_str())
    }

    /// Every trap `trap` should print: this shell's, plus any it inherited and reset.
    ///
    /// Deliberately *not* what the dispatcher reads. A subshell runs none of the inherited
    /// handlers — that is what resetting means — but `saved=$(trap)` must still see them, or the
    /// save-and-restore idiom silently saves nothing.
    pub fn listable_traps(&self) -> HashMap<String, String> {
        let mut all = self.inherited_traps.clone();
        all.extend(
            self.signal_traps
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        all
    }
}
