//! Reading and writing the shell's variables.
//!
//! Split from [`super`] so that file is about what an `Environment` *is* — the fields, the frames,
//! the depth guards — and this one is about the one table every part of the shell goes through.
//!
//! [`Environment::set_var`] is the choke point on purpose, and its own doc says why: an attribute
//! decided there covers `read`, a `for` loop variable and `${v:=x}` for free, where deciding it at
//! the `name=value` site would cover one of the three.

use super::{Environment, environ_remove, environ_set, reject_unrepresentable};
use std::collections::HashMap;

/// The help line under an assignment to a name that will not take one.
///
/// Worth saying because the failure is silent in the other direction: `readonly` is usually set in
/// a config or a sourced file, a long way from the line that then fails, and "is read only" does
/// not say who decided that or that it cannot be undone.
pub(crate) const READONLY: &str =
    "declared with `readonly`; the name keeps its value for the life of the shell";

impl Environment {
    /// A variable's value as a single string.
    ///
    /// An array answers with its element 0, because in bash `$a` and `${a[0]}` are the same
    /// reference: `a=(1 2 3); echo "$a"` prints `1`.
    pub fn get_var(&self, name: &str) -> Option<&str> {
        match name {
            // `$-` joins the list: it is computed from the option bitset, and a variable of that
            // name (which no assignment can create, but `environ` can carry) must not shadow it.
            "?" | "$" | "!" | "#" | "*" | "@" | "-" => None,
            "BASH_ARGV0" if !crate::env::dynamic::retired(name) => Some(&self.shell_name),
            "BASH_COMMAND" => Some(self.bash_command.as_str()),
            _ => match self.vars.get(name) {
                Some((v, _)) => Some(v.as_str()),
                None => self.arrays.get(name).and_then(|a| a.get(0)),
            },
        }
    }

    /// What `$*` joins positional parameters with: the first character of IFS, or nothing at all
    /// when IFS is set but empty. An unset IFS means the default, whose first character is a space.
    pub fn ifs_separator(&self) -> String {
        match self.get_var("IFS") {
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
            // resolved as a *field list* in `expand::param`, because collapsing them to one string
            // is what silently corrupted every wrapper script's arguments.
            "*" | "@" => Some(self.positional.join(&self.ifs_separator())),
            s => {
                if let Ok(idx) = s.parse::<usize>()
                    && idx > 0
                    && idx <= self.positional.len()
                {
                    return Some(self.positional[idx - 1].clone());
                }
                // A set variable always wins: `SECONDS=0` is an idiom, `RANDOM=42` asks for a
                // reproducible sequence, and an exported `EPOCHSECONDS` from a parent must not be
                // shadowed by our clock.
                self.get_var(name)
                    .map(|s| s.to_string())
                    .or_else(|| self.special_value(name))
            }
        }
    }

    /// Assign `name=value`. `false` if the variable is read-only or unrepresentable.
    ///
    /// A scalar assignment to a name that already holds an array writes **element 0**, which is
    /// what bash does: `a=(1 2 3); a=4` leaves `${a[@]}` as `4 2 3`. Deciding that here rather
    /// than in the assignment code means `read a`, a `for a in …` loop variable and `${a:=x}` all
    /// agree about it for free.
    pub fn set_var(&mut self, name: &str, value: &str, export: bool) -> bool {
        // An assignment to `LINENO` from a script is the shell's cue that its own record of what
        // it last published is no longer the truth. See [`Self::note_line`].
        if name == "LINENO" {
            self.published_line = 0;
        }
        if self.is_readonly(name) {
            // The whole assignment as the source line, with the caret on the name: `R` alone in a
            // box says less than the one-liner did, and `R=hello` is the line that was refused.
            let assignment = format!("{name}={value}");
            crate::env::complain_within_from(
                &self.origin(),
                std::slice::from_ref(&assignment),
                &assignment,
                0..name.len(),
                &format!("{name}: is read only"),
                "readonly",
                Some(READONLY),
            );
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
        // **An integer name evaluates what it is assigned.** `declare -i n; n=4*5` stores 20, not
        // the three characters. Decided here for the reason the doc above gives: `read`, a `for`
        // variable and `${n:=x}` are then all covered by one rule rather than three.
        //
        // An expression that does not evaluate is zero, which is bash's answer too — `n=abc` on an
        // integer name stores 0 rather than failing the assignment.
        if self.is_integer(name) {
            let evaluated = crate::expand::arithmetic::eval_arithmetic(self, value).unwrap_or(0);
            return self.store(name, &evaluated.to_string(), export);
        }
        self.store(name, value, export)
    }

    /// The assignment itself, once the name's attributes have had their say.
    ///
    /// Split from [`Self::set_var`] so the integer path above can come back through it without
    /// re-entering the attribute check and evaluating its own result a second time.
    fn store(&mut self, name: &str, value: &str, export: bool) -> bool {
        if let Some(array) = self.arrays.get_mut(name) {
            array.set(0, value);
            return true;
        }
        // `set -a` exports every name that is assigned to, which is why this belongs here and not
        // at the `name=value` site: POSIX applies it to *any* assignment, so `read`, a `for` loop
        // variable and `${v:=x}` are all covered by deciding it once. The idiom it exists for is
        // `set -a; . /etc/os-release`, which before this exported nothing at all.
        // A pending `export V` is honoured and spent here; the variable carries the flag now.
        let was_pending = self.pending_exports.remove(name);
        let is_exp = export
            || was_pending
            || self.allexport()
            || self.vars.get(name).map(|(_, exp)| *exp).unwrap_or(false);
        self.vars
            .insert(name.to_string(), (value.to_string(), is_exp));
        if is_exp {
            environ_set(name, value);
        }
        if name == "PATH" {
            // Every remembered command location was resolved through the old `PATH`, so keeping
            // them would make `PATH=/new/bin:$PATH; tool` still run the old `tool` — the one
            // failure a command cache is always blamed for. bash flushes here too.
            crate::env::builtins::hash_forget_all();
        }
        if name == "PWD" {
            self.publish_dirstack();
        }
        // `BASH_COMPAT` is the `compatNN` shopt names, spelled as a variable. bash leaves
        // `BASHOPTS` as the last `shopt` wrote it.
        if name == "BASH_COMPAT" {
            crate::env::builtins::shopt::note_compat(value);
        }
        true
    }

    /// Remove `name` entirely, in whichever shape it has. `unset a` drops the whole array, not
    /// just its element 0.
    /// Every exported name and its value — what a child process would be handed.
    ///
    /// Exported only, because that is the question a caller is asking when it wants "the
    /// environment": a shell-local variable is not part of it and would mislead anyone iterating
    /// this to decide what a command will see.
    /// Every variable and whether it is exported, for a caller that must put the shell back exactly
    /// as it found it.
    ///
    /// [`Self::exported_vars`] is the right answer to "what will a child see" and the wrong one to
    /// "what was here before": a shell-local variable that a directory environment then exports has
    /// to come back *local*, and a snapshot of the exported set alone cannot say that — it would
    /// remove the variable entirely on the way out.
    pub fn all_vars(&self) -> Vec<(String, String, bool)> {
        let mut out: Vec<(String, String, bool)> = self
            .vars
            .iter()
            .map(|(name, (value, exported))| (name.clone(), value.clone(), *exported))
            .collect();
        out.sort();
        out
    }

    pub fn exported_vars(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .vars
            .iter()
            .filter(|(_, (_, exported))| *exported)
            .map(|(name, (value, _))| (name.clone(), value.clone()))
            .collect();
        // Sorted so a caller that prints them gets a stable order; a HashMap's is arbitrary and
        // would make any test or diff over the result flap.
        out.sort();
        out
    }

    pub fn unset_var(&mut self, name: &str) {
        if name == "LINENO" {
            self.published_line = 0;
        }
        // `unset SECONDS` makes it an ordinary variable rather than the clock, as in bash — after
        // which it is empty. Without this it would go straight back to counting, so the one way to
        // stop it counting would not work.
        crate::env::dynamic::retire(name);
        self.integer_vars.remove(name);
        self.vars.remove(name);
        self.arrays.remove(name);
        // `unset` undoes a pending `export` too, or `export V; unset V; V=1` would still export.
        self.pending_exports.remove(name);
        environ_remove(name);
        if name == "BASH_COMPAT" {
            crate::env::builtins::shopt::note_compat("");
        }
    }

    /// Forget any *scalar* under `name`, leaving the array table alone.
    ///
    /// Used by the array store when a name changes shape; not public, because outside that
    /// transition "unset" always means [`Self::unset_var`].
    pub(super) fn drop_scalar(&mut self, name: &str) {
        self.vars.remove(name);
        environ_remove(name);
    }

    /// Mark an existing variable exported, creating it empty if it does not exist.
    ///
    /// `false` if its current value cannot live in `environ` — a NUL-bearing value read from a
    /// binary file, say, which `export` must refuse instead of handing to `setenv`.
    pub fn export_var(&mut self, name: &str) -> bool {
        if let Some((val, _)) = self.vars.get(name) {
            let val = val.clone();
            if reject_unrepresentable(&self.origin(), name, &val) {
                return false;
            }
            if let Some((_, exp)) = self.vars.get_mut(name) {
                *exp = true;
            }
            environ_set(name, &val);
        } else {
            if reject_unrepresentable(&self.origin(), name, "") {
                return false;
            }
            // Recorded, not created. See `pending_exports`.
            self.pending_exports.insert(name.to_string());
        }
        true
    }

    pub fn get_all_vars(&self) -> HashMap<String, String> {
        self.vars
            .iter()
            .map(|(k, (v, _))| (k.clone(), v.clone()))
            .collect()
    }

    /// Whether `name` would be handed to a child process.
    ///
    /// One lookup rather than building the whole exported map, which is what a caller that only
    /// wants to answer this about a single name was otherwise reduced to.
    pub fn is_exported(&self, name: &str) -> bool {
        self.vars.get(name).is_some_and(|(_, exported)| *exported)
    }

    pub fn get_exported_vars(&self) -> HashMap<String, String> {
        self.vars
            .iter()
            .filter(|(_, (_, exp))| *exp)
            .map(|(k, (v, _))| (k.clone(), v.clone()))
            .collect()
    }
}
