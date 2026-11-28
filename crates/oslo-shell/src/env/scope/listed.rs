//! The bindings as `declare -p`, `set` and a bare `declare` list them, and the reads that change
//! what they show. See `crate::env::dynamic::cells`.

use super::value::{Attrs, Value, Var};
use super::{AssocArray, Environment, ShellArray};
use crate::env::dynamic;

impl Environment {
    /// Every binding as a listing shows it, sorted by name the way bash sorts one: a computed
    /// variable no read has shown has no value yet, and one computed outside the table is listed
    /// with what it last answered.
    pub fn listing(&self) -> Vec<(String, Var)> {
        let mut out: Vec<(String, Var)> = self
            .vars
            .iter()
            .map(|(name, var)| {
                let shown = match (dynamic::unread(name), dynamic::remembered(name)) {
                    (true, _) => blank(var),
                    (false, Some(value)) => Var::scalar(value, var.attrs),
                    (false, None) => var.clone(),
                };
                (name.clone(), shown)
            })
            .collect();
        for name in dynamic::COMPUTED {
            if self.vars.contains_key(name) || dynamic::retired(name) {
                continue;
            }
            let attrs = match dynamic::is_integer(name) {
                true => Attrs::INTEGER,
                false => Attrs::empty(),
            };
            let var = match dynamic::remembered(name) {
                Some(value) => Var::scalar(value, attrs),
                None => Var::unset(attrs),
            };
            out.push((name.to_string(), var));
        }
        out.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        out
    }

    /// The script read `name`. A listing shows its value from now on, and `BASH_ALIASES` and
    /// `BASH_CMDS` take in the aliases and the remembered commands there are now.
    pub fn note_read(&mut self, name: &str) {
        dynamic::mark_read(name);
        if dynamic::retired(name) {
            return;
        }
        match name {
            "BASH_COMMAND" => dynamic::remember(name, &self.bash_command),
            "BASH_ARGV0" => dynamic::remember(name, &self.shell_name),
            "LINENO" | "HISTCMD" => {
                let value = self.vars.get(name).and_then(Var::text).unwrap_or_default();
                dynamic::remember(name, &value.to_string());
            }
            "BASH_ALIASES" | "BASH_CMDS" if self.is_assoc(name) => {
                let mut map = AssocArray::default();
                match name {
                    "BASH_ALIASES" => {
                        let mut aliases: Vec<(&String, &String)> =
                            self.get_aliases().iter().collect();
                        aliases.sort();
                        for (alias, value) in aliases {
                            map.set(alias, value);
                        }
                    }
                    _ => {
                        for (command, path) in crate::env::builtins::hash_entries() {
                            map.set(&command, &oslo_base::lossless::encode(path.as_os_str()));
                        }
                    }
                }
                let attrs = self.vars.get(name).map_or(Attrs::empty(), |var| var.attrs);
                let value = Value::Assoc(map);
                self.vars.insert(name.to_string(), Var { value, attrs });
            }
            _ => {}
        }
    }

    /// A write of `value` under `key` to `BASH_ALIASES` or `BASH_CMDS` defines that alias or
    /// remembers that command, as in bash.
    pub(super) fn mirror_write(&mut self, name: &str, key: &str, value: &str) {
        if dynamic::retired(name) {
            return;
        }
        dynamic::mark_read(name);
        match name {
            "BASH_ALIASES" => self.set_alias(key, value),
            "BASH_CMDS" => crate::env::builtins::hash_assign(key, value),
            _ => {}
        }
    }

    /// The two empty associative arrays a new shell starts with.
    pub(super) fn seed_mirrors(&mut self) {
        for name in ["BASH_ALIASES", "BASH_CMDS"] {
            let value = Value::Assoc(AssocArray::default());
            let attrs = Attrs::empty();
            self.vars.insert(name.to_string(), Var { value, attrs });
        }
    }
}

/// `var` before any read has given it a value: an array is empty, a string unset.
fn blank(var: &Var) -> Var {
    let value = match var.value {
        Value::Indexed(_) => Value::Indexed(ShellArray::default()),
        Value::Assoc(_) => Value::Assoc(AssocArray::default()),
        Value::Scalar(_) => return Var::unset(var.attrs),
    };
    Var {
        value,
        attrs: var.attrs,
    }
}
