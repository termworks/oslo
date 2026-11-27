//! Function attributes: `export -f`, which hands a function to child shells, and `readonly -f`.
//!
//! An exported function travels the way bash sends one, as an environment entry named
//! `BASH_FUNC_name%%` whose value is the definition without its name, `() { …; }`. A shell that
//! finds such an entry at startup defines the function and keeps it exported; an entry that does
//! not parse as exactly one definition of that name is reported and left alone.

use super::{Environment, environ_remove, environ_set};
use oslo_base::ast::{Command, FunctionSource};

const PREFIX: &str = "BASH_FUNC_";
const SUFFIX: &str = "%%";

/// The function an environment entry exports, if it is one.
pub(super) fn exported_function_name<'a>(key: &'a str, value: &str) -> Option<&'a str> {
    let name = key.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    (!name.is_empty() && value.starts_with("() {")).then_some(name)
}

fn environ_key(name: &str) -> String {
    format!("{PREFIX}{name}{SUFFIX}")
}

/// The definition `text` holds, if it is exactly one function named `name`, with its source text.
fn parse_definition(name: &str, text: &str) -> Option<(Command, FunctionSource)> {
    let source = format!("{name} {text}");
    let list = crate::syntax::parse_bash_script(&source).ok()?;
    let [item] = list.items.as_slice() else {
        return None;
    };
    if !item.and_or.rest.is_empty() || item.and_or.first.commands.len() != 1 {
        return None;
    }
    match &item.and_or.first.commands[0] {
        Command::FunctionDef {
            name: defined,
            body,
            source,
        } if defined == name => Some(((**body).clone(), source.clone())),
        _ => None,
    }
}

impl Environment {
    /// Define every function the environment exported to this shell.
    pub(super) fn import_functions(&mut self, entries: &[(String, String)]) {
        for (name, text) in entries {
            match parse_definition(name, text) {
                Some((body, source)) => {
                    self.exported_functions.insert(name.clone());
                    self.set_function(name, body);
                    self.set_function_source(name, source.0);
                }
                None => eprintln!(
                    "{}error importing function definition for `{name}'",
                    self.origin()
                ),
            }
        }
    }

    /// `export -f name`. `false` if there is no such function.
    pub fn export_function(&mut self, name: &str) -> bool {
        if self.get_function(name).is_none() {
            return false;
        }
        self.exported_functions.insert(name.to_string());
        self.publish_function(name);
        true
    }

    /// `export -nf name`.
    pub fn unexport_function(&mut self, name: &str) {
        if self.exported_functions.remove(name) {
            environ_remove(&environ_key(name));
        }
    }

    pub fn is_function_exported(&self, name: &str) -> bool {
        self.exported_functions.contains(name)
    }

    /// `readonly -f name`. `false` if there is no such function.
    pub fn set_function_readonly(&mut self, name: &str) -> bool {
        if self.get_function(name).is_none() {
            return false;
        }
        self.readonly_functions.insert(name.to_string());
        true
    }

    pub fn is_function_readonly(&self, name: &str) -> bool {
        self.readonly_functions.contains(name)
    }

    /// Write the current definition of an exported function into the environment.
    pub(super) fn publish_function(&self, name: &str) {
        if !self.exported_functions.contains(name) {
            return;
        }
        let Some(body) = self.get_function(name) else {
            return;
        };
        let text = crate::env::builtins::format_function(name, body);
        let header = format!("{name} () \n{{");
        let value = match text.strip_prefix(&header) {
            Some(rest) => format!("() {{{rest}"),
            None => return,
        };
        environ_set(&environ_key(name), &value);
    }

    /// Forget a function's attributes along with the function. `unset -f`.
    pub(super) fn forget_function_attrs(&mut self, name: &str) {
        self.unexport_function(name);
        self.readonly_functions.remove(name);
    }
}
