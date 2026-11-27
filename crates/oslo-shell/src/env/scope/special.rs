//! The variables the shell keeps about its own state: `$BASHPID`, `$BASH_ARGV0`, `$HISTCMD`,
//! `$BASH_SUBSHELL`, `GROUPS` and `DIRSTACK`.
//!
//! Each stays special until it is `unset`, which [`crate::env::dynamic::retired`] records.

use super::{Environment, ShellArray};
use crate::env::dynamic;

/// One write to an array, as the array store receives it.
pub(super) enum ArrayWrite<'a> {
    Whole(&'a ShellArray),
    Element(i64, &'a str),
    Append,
    Unset,
}

impl Environment {
    /// The value of a special variable that is computed rather than stored, or `None`.
    pub fn special_value(&self, name: &str) -> Option<String> {
        match name {
            "BASH_ARGV0" if !dynamic::retired(name) => Some(self.shell_name.clone()),
            _ => dynamic::value(name),
        }
    }

    /// Take an assignment to a special name; `true` when it was consumed here.
    pub(super) fn special_assign(&mut self, name: &str, value: &str) -> bool {
        if dynamic::retired(name) {
            return false;
        }
        match name {
            "BASHPID" | "HISTCMD" | "GROUPS" | "DIRSTACK" => true,
            "BASH_ARGV0" => {
                self.shell_name = value.to_string();
                true
            }
            _ => {
                let mut eval =
                    |text: &str| crate::expand::arithmetic::eval_arithmetic(self, text).ok();
                dynamic::assign(name, value, &mut eval)
            }
        }
    }

    /// Take a write to a special array; `Some(accepted)` when it was handled here.
    pub(super) fn special_array_write(&mut self, name: &str, write: ArrayWrite) -> Option<bool> {
        if dynamic::retired(name) {
            return None;
        }
        match name {
            "GROUPS" => Some(true),
            "DIRSTACK" => {
                match write {
                    ArrayWrite::Whole(array) => {
                        let pairs: Vec<(i64, String)> = array
                            .indices()
                            .zip(array.values().map(str::to_string))
                            .collect();
                        for (index, value) in pairs {
                            self.set_dirstack_entry(index, &value);
                        }
                    }
                    ArrayWrite::Element(index, value) => self.set_dirstack_entry(index, value),
                    ArrayWrite::Append | ArrayWrite::Unset => {}
                }
                self.publish_dirstack();
                Some(true)
            }
            _ => None,
        }
    }

    /// Replace entry `index` of the stack as `dirs` numbers it; entry 0 is the current directory
    /// and is not replaced.
    fn set_dirstack_entry(&mut self, index: i64, value: &str) {
        let len = self.dir_stack.len() as i64;
        if index >= 1 && index <= len {
            self.dir_stack[(len - index) as usize] = value.into();
        }
    }

    /// Publish `DIRSTACK`: the current directory, then the pushed entries, most recent first.
    pub(super) fn publish_dirstack(&mut self) {
        if dynamic::retired("DIRSTACK") {
            return;
        }
        let here = match self.vars.get("PWD") {
            Some((pwd, _)) if !pwd.is_empty() => pwd.clone(),
            _ => std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let mut entries = vec![here];
        entries.extend(
            self.dir_stack
                .iter()
                .rev()
                .map(|path| path.to_string_lossy().into_owned()),
        );
        self.vars.remove("DIRSTACK");
        self.arrays
            .insert("DIRSTACK".to_string(), ShellArray::from_values(entries));
    }

    /// The per-process state a forked child refreshes: `$BASHPID`, and `$BASH_SUBSHELL` when
    /// `deeper` says this child counts as one more level.
    pub(super) fn refresh_subshell_vars(&mut self, deeper: bool) {
        if !dynamic::retired("BASHPID") {
            self.vars
                .insert("BASHPID".to_string(), (self.current_pid.to_string(), false));
        }
        if deeper && !dynamic::retired("BASH_SUBSHELL") {
            let level = self
                .get_var("BASH_SUBSHELL")
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0);
            let exported = self.is_exported("BASH_SUBSHELL");
            self.store_special("BASH_SUBSHELL", &(level + 1).to_string(), exported);
        }
    }

    /// Set the special variables a new shell starts with.
    pub(super) fn seed_special_vars(&mut self) {
        let pid = self.current_pid.to_string();
        self.vars.insert("BASHPID".to_string(), (pid, false));
        self.store_special("BASH_SUBSHELL", "0", self.is_exported("BASH_SUBSHELL"));
        self.vars
            .insert("HISTCMD".to_string(), ("0".to_string(), false));
        self.vars.remove("GROUPS");
        self.arrays
            .insert("GROUPS".to_string(), ShellArray::from_values(groups()));
        self.publish_dirstack();
    }

    /// Publish the arguments the shell was started with as `BASH_ARGV` (last first) and
    /// `BASH_ARGC`, which bash keeps as they were at startup.
    pub fn publish_startup_args(&mut self) {
        let args: Vec<String> = self.positional.iter().rev().cloned().collect();
        let count = args.len().to_string();
        self.arrays
            .insert("BASH_ARGV".to_string(), ShellArray::from_values(args));
        self.arrays
            .insert("BASH_ARGC".to_string(), ShellArray::from_values([count]));
    }

    /// Write `name` straight into the table, past the attribute checks.
    fn store_special(&mut self, name: &str, value: &str, exported: bool) {
        self.vars
            .insert(name.to_string(), (value.to_string(), exported));
        if exported {
            super::environ_set(name, value);
        }
    }
}

/// The groups this process is in, the primary one first, as bash orders `GROUPS`.
fn groups() -> Vec<String> {
    let gid = nix::unistd::getgid();
    let mut list = nix::unistd::getgroups().unwrap_or_default();
    match list.iter().position(|g| *g == gid) {
        Some(at) => list.swap(0, at),
        None => list.insert(0, gid),
    }
    list.iter().map(|g| g.as_raw().to_string()).collect()
}
