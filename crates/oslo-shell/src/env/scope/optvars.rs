//! `$SHELLOPTS` and `$BASHOPTS`: the two option sets, published as readonly variables.
//!
//! Written through the ordinary variable API: the readonly mark is lifted for the write and put
//! back, and an inherited export is kept.

use super::Environment;
use crate::env::builtins::shopt;
use crate::env::options::ShellOption;

impl Environment {
    /// Rewrite both variables from the options now in force.
    pub fn publish_option_vars(&mut self) {
        let shellopts = self.options.shellopts();
        self.publish_readonly("SHELLOPTS", &shellopts);
        self.publish_readonly("BASHOPTS", &shopt::bashopts());
    }

    fn publish_readonly(&mut self, name: &str, value: &str) {
        if self.is_readonly(name) && self.get_var(name) == Some(value) {
            return;
        }
        let exported = self.is_exported(name);
        // `set -a` exports what a script assigns, not what the shell publishes about itself.
        let allexport = self.options.is_set(ShellOption::AllExport);
        self.options.set(ShellOption::AllExport, false);
        self.release_readonly(name);
        self.set_var(name, value, exported);
        self.set_readonly(name);
        self.options.set(ShellOption::AllExport, allexport);
    }

    /// Turn on what an inherited `SHELLOPTS` or `BASHOPTS` names, then publish both.
    pub(super) fn seed_option_vars(&mut self) {
        if self.is_exported("SHELLOPTS")
            && let Some(list) = self.get_var("SHELLOPTS").map(str::to_string)
        {
            for option in list.split(':').filter_map(ShellOption::from_name) {
                self.options.set(option, true);
            }
        }
        if self.is_exported("BASHOPTS")
            && let Some(list) = self.get_var("BASHOPTS").map(str::to_string)
        {
            for name in list.split(':') {
                let _ = shopt::set_option(name, true);
            }
        }
        self.publish_option_vars();
    }
}
