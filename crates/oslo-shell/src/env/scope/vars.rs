//! Reading and writing the shell's variables.
//!
//! [`Environment::set_var`] is the choke point: an attribute decided there covers `read`, a `for`
//! loop variable and `${v:=x}` alike, where deciding it at the `name=value` site would cover one.

use super::value::{Attrs, Value, Var, convert_case};
use super::{Environment, reject_unrepresentable};
use std::collections::HashMap;

/// The help line under an assignment to a name that will not take one.
pub(crate) const READONLY: &str =
    "declared with `readonly`; the name keeps its value for the life of the shell";

thread_local! {
    /// The error an integer variable's value raised, until the command it happened in ends.
    static INTEGER_FAILURE: std::cell::RefCell<Option<oslo_base::error::ShellError>> =
        const { std::cell::RefCell::new(None) };
}

/// The error an integer variable's value raised since this was last asked.
pub(crate) fn take_integer_failure() -> Option<oslo_base::error::ShellError> {
    INTEGER_FAILURE.with(|f| f.borrow_mut().take())
}

impl Environment {
    /// A variable's value as a single string.
    ///
    /// An array answers with its element 0, because in bash `$a` and `${a[0]}` are the same
    /// reference: `a=(1 2 3); echo "$a"` prints `1`.
    pub fn get_var(&self, name: &str) -> Option<&str> {
        match name {
            "?" | "$" | "!" | "#" | "*" | "@" | "-" => None,
            "BASH_ARGV0" if !crate::env::dynamic::retired(name) => Some(&self.shell_name),
            "BASH_COMMAND" => Some(self.bash_command.as_str()),
            _ => {
                let var = self.vars.get(name)?;
                if var.is_nameref() {
                    return self.read_through_ref(name);
                }
                var.text()
            }
        }
    }

    /// The binding `name` resolves to, without following a nameref.
    pub fn variable(&self, name: &str) -> Option<&Var> {
        self.vars.get(name)
    }

    /// What `$*` joins positional parameters with: the first character of IFS, its first byte in
    /// the C locale, or nothing at all when IFS is set but empty. An unset IFS means the default,
    /// whose first character is a space.
    pub fn ifs_separator(&self) -> String {
        match self.get_var("IFS") {
            Some(ifs) if !crate::expand::locale::is_utf8(self) => {
                let bytes = oslo_base::lossless::decode(ifs);
                oslo_base::lossless::from_bytes(&bytes[..bytes.len().min(1)])
            }
            Some(ifs) => ifs.chars().next().map(String::from).unwrap_or_default(),
            None => " ".to_string(),
        }
    }

    pub fn get_param(&self, name: &str) -> Option<String> {
        match name {
            "?" => Some(self.last_status.to_string()),
            "$" => Some(self.pid.to_string()),
            "!" => self.last_bg_pid.map(|p| p.to_string()),
            "#" => Some(self.positional.len().to_string()),
            "-" => Some(self.option_flags()),
            "0" => Some(self.shell_name.clone()),
            // Only the forms that genuinely need a single string reach here: `"$@"` and `$@` are
            // resolved as a *field list* in `expand::param`.
            "*" | "@" => Some(self.positional.join(&self.ifs_separator())),
            s => {
                if let Ok(idx) = s.parse::<usize>()
                    && idx > 0
                    && idx <= self.positional.len()
                {
                    return Some(self.positional[idx - 1].clone());
                }
                // A set variable always wins over the dynamic one of the same name: `SECONDS=0`
                // and `RANDOM=42` are assignments, and an inherited `EPOCHSECONDS` is a value.
                self.get_var(name)
                    .map(|s| s.to_string())
                    .or_else(|| self.special_value(name))
            }
        }
    }

    /// Say that `name` cannot be assigned because it is read-only.
    pub(crate) fn complain_readonly(&self, name: &str, value: &str) {
        let assignment = format!("{name}={value}");
        crate::env::complain_within_from(
            &self.origin(),
            std::slice::from_ref(&assignment),
            &assignment,
            0..name.len(),
            &format!("{name}: readonly variable"),
            "readonly",
            Some(READONLY),
        );
    }

    /// Assign `name=value`. `false` if the variable is read-only or unrepresentable.
    ///
    /// A scalar assignment to an array writes **element 0**, as bash does: `a=(1 2 3); a=4`
    /// leaves `${a[@]}` as `4 2 3`. A nameref passes the assignment to the variable it names.
    pub fn set_var(&mut self, name: &str, value: &str, export: bool) -> bool {
        if name == "LINENO" {
            self.published_line = 0;
        }
        if self.vars.get(name).is_some_and(Var::is_nameref) {
            return self.write_through_ref(name, value, export);
        }
        if self.is_readonly(name) {
            self.complain_readonly(name, value);
            return false;
        }
        if reject_unrepresentable(&self.origin(), name, value) {
            return false;
        }
        // The clocks, the generator and the shell's own special names take an assignment rather
        // than being shadowed by it: `SECONDS=0` re-bases, `RANDOM=42` seeds.
        if self.special_assign(name, value) {
            return true;
        }
        let value = self.transform(name, value);
        if INTEGER_FAILURE.with(|f| f.borrow().is_some()) {
            return false;
        }
        self.store(name, value, export)
    }

    /// `value` as the attributes of `name` turn it into what is stored: an integer name evaluates
    /// it as arithmetic (an expression that fails is 0, as in bash), and a case attribute converts
    /// it afterwards.
    pub(crate) fn transform(&mut self, name: &str, value: &str) -> String {
        let attrs = self.vars.get(name).map(|v| v.attrs).unwrap_or_default();
        self.transform_with(attrs, value)
    }

    /// [`Self::transform`] for attributes already in hand.
    ///
    /// An integer value that does not evaluate is reported here and left pending for
    /// [`take_integer_failure`], since bash abandons the top-level command over it.
    pub(crate) fn transform_with(&mut self, attrs: Attrs, value: &str) -> String {
        let value = match attrs.has(Attrs::INTEGER) {
            true => match crate::expand::arithmetic::eval_arithmetic(self, value) {
                Ok(n) => n.to_string(),
                Err(e) => {
                    let e = crate::expand::arithmetic::in_subscript(self, e);
                    INTEGER_FAILURE.with(|f| *f.borrow_mut() = Some(e));
                    "0".to_string()
                }
            },
            false => value.to_string(),
        };
        match attrs.has(Attrs::CASE) {
            true => convert_case(attrs, &value),
            false => value,
        }
    }

    /// The assignment itself, once the name's attributes have had their say.
    fn store(&mut self, name: &str, value: String, export: bool) -> bool {
        let export = export || self.allexport();
        let was_exported = self.chain_exports(name);
        match self.vars.get_mut(name) {
            Some(var) => {
                match &mut var.value {
                    Value::Scalar(text) => *text = value,
                    Value::Indexed(array) => array.set(0, value),
                    Value::Assoc(map) => map.set("0", value),
                }
                var.attrs.remove(Attrs::UNSET);
                if export {
                    var.attrs.insert(Attrs::EXPORT);
                }
            }
            None => {
                let attrs = if export {
                    Attrs::EXPORT
                } else {
                    Attrs::empty()
                };
                self.vars
                    .insert(name.to_string(), Var::scalar(value, attrs));
            }
        }
        if was_exported || export || self.chain_exports(name) {
            self.sync_environ(name);
        }
        if name == "PATH" {
            // Every remembered command location was resolved through the old `PATH`.
            crate::env::builtins::hash_forget_all();
        }
        if name == "PWD" {
            self.publish_dirstack();
        }
        // `BASH_COMPAT` is the `compatNN` shopt names, spelled as a variable. bash leaves
        // `BASHOPTS` as the last `shopt` wrote it.
        if name == "BASH_COMPAT" {
            crate::env::builtins::shopt::note_compat(self.get_var(name).unwrap_or_default());
        }
        true
    }

    /// Every scalar and whether it is exported, for a caller that must put the shell back
    /// exactly as it found it.
    pub fn all_vars(&self) -> Vec<(String, String, bool)> {
        let mut out: Vec<(String, String, bool)> = self
            .scalars()
            .map(|(name, value, var)| (name.to_string(), value.to_string(), var.exported()))
            .collect();
        out.sort();
        out
    }

    /// Every exported name and its value — what a child process would be handed. Sorted.
    pub fn exported_vars(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .scalars()
            .filter(|(_, _, var)| var.exported())
            .map(|(name, value, _)| (name.to_string(), value.to_string()))
            .collect();
        out.sort();
        out
    }

    /// Every visible binding that holds a plain string.
    fn scalars(&self) -> impl Iterator<Item = (&str, &str, &Var)> {
        self.vars.iter().filter_map(|(name, var)| match &var.value {
            Value::Scalar(text) if var.is_set() => Some((name.as_str(), text.as_str(), var)),
            _ => None,
        })
    }

    /// Every binding, sorted by name the way bash sorts a listing.
    pub fn bindings(&self) -> Vec<(&str, &Var)> {
        let mut out: Vec<(&str, &Var)> = self
            .vars
            .iter()
            .map(|(name, var)| (name.as_str(), var))
            .collect();
        out.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        out
    }

    /// The set variables whose names start with `prefix`, sorted: `${!prefix*}`.
    pub fn names_with_prefix(&self, prefix: &str) -> Vec<String> {
        self.bindings()
            .into_iter()
            .filter(|(name, var)| name.starts_with(prefix) && var.is_set())
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// The locals of the innermost function, sorted, for `local` with no operands.
    pub fn local_bindings(&self) -> Vec<(&str, &Var)> {
        self.bindings()
            .into_iter()
            .filter(|(name, _)| self.is_local_here(name))
            .collect()
    }

    /// Remove `name` as `unset` does, following a nameref to what it names.
    pub fn unset_var(&mut self, name: &str) {
        if self.vars.get(name).is_some_and(Var::is_nameref) {
            match self.deref(name) {
                Some(super::nameref::Target::Name(target)) => return self.unset_var(&target),
                Some(super::nameref::Target::Element { name, subscript }) => {
                    return self.unset_element_text(&name, &subscript);
                }
                None => {}
            }
        }
        self.unset_binding(name);
    }

    /// Remove the binding `name` itself, nameref or not: `unset -n`.
    pub fn unset_binding(&mut self, name: &str) {
        if name == "LINENO" {
            self.published_line = 0;
        }
        // `unset SECONDS` makes it an ordinary variable rather than the clock, as in bash.
        crate::env::dynamic::retire(name);
        self.unbind(name);
        if name == "BASH_COMPAT" {
            crate::env::builtins::shopt::note_compat(self.get_var(name).unwrap_or_default());
        }
    }

    /// Mark `name` exported, declaring it with no value if it does not exist.
    ///
    /// `false` if its current value cannot live in `environ`.
    pub fn export_var(&mut self, name: &str) -> bool {
        let value = self.get_var(name).unwrap_or_default().to_string();
        if reject_unrepresentable(&self.origin(), name, &value) {
            return false;
        }
        self.modify_attrs(name, Attrs::EXPORT, Attrs::empty());
        true
    }

    pub fn get_all_vars(&self) -> HashMap<String, String> {
        self.scalars()
            .map(|(name, value, _)| (name.to_string(), value.to_string()))
            .collect()
    }

    /// Whether `name` would be handed to a child process.
    pub fn is_exported(&self, name: &str) -> bool {
        self.vars.get(name).is_some_and(Var::exported)
    }

    pub fn get_exported_vars(&self) -> HashMap<String, String> {
        self.scalars()
            .filter(|(_, _, var)| var.exported())
            .map(|(name, value, _)| (name.to_string(), value.to_string()))
            .collect()
    }
}
