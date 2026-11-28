//! Variables whose value is a secret.
//!
//! A sensitive binding expands, exports and compares like any other. What changes is what the shell
//! *prints* about it: `set -x`, `set`, `declare -p`, `typeset -p`, `export -p` and `readonly -p`
//! show [`HIDDEN`] where the value would be.
//!
//! Only oslo marks a binding, so a bash script never meets one:
//!
//! * a stored variable whose recipe runs `oslo secret get` or `oslo secret run`;
//! * a value Lua was handed by `oslo.secret.*`, when it is assigned to a variable from Lua;
//! * `oslo.env.set(name, value, { secret = true })`.
//!
//! The mark belongs to the binding: `unset` drops it, and a `local` of the same name starts without
//! one. A child process is handed the value and not the mark.

use super::Environment;
use super::value::{Attrs, Value, Var};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::hash::{BuildHasher, RandomState};

/// What a trace line or a listing shows in place of a sensitive value.
pub const HIDDEN: &str = "<secret>";

thread_local! {
    /// Keyed hashes of the values `oslo.secret.*` handed to Lua. Never the values themselves.
    static HANDED: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
    static KEYS: RandomState = RandomState::new();
}

fn digest(value: &[u8]) -> u64 {
    KEYS.with(|keys| keys.hash_one(value))
}

/// Remember that `value` left the secrets store, so a variable later given exactly it is marked.
pub fn note(value: &[u8]) {
    if value.is_empty() {
        return;
    }
    let hash = digest(value);
    HANDED.with(|set| set.borrow_mut().insert(hash));
}

/// Whether `value` is one [`note`] was told about.
pub fn was_handed_out(value: &str) -> bool {
    if value.is_empty() || HANDED.with(|set| set.borrow().is_empty()) {
        return false;
    }
    let hash = digest(value.as_bytes());
    HANDED.with(|set| set.borrow().contains(&hash))
}

/// Whether a stored variable's recipe reads the secrets store: `oslo secret get` or `run`, with or
/// without `--store`, and with `oslo` written as a path or a bare name.
///
/// The words are matched anywhere in the body, not only in command position.
pub fn recipe_reads_a_secret(body: &str) -> bool {
    let words: Vec<&str> = body
        .split(|c: char| c.is_whitespace() || "$()`;|&\"'{}<>".contains(c))
        .filter(|word| !word.is_empty())
        .collect();
    words.iter().enumerate().any(|(at, word)| {
        let program = *word == "oslo" || word.ends_with("/oslo");
        if !program || words.get(at + 1) != Some(&"secret") {
            return false;
        }
        let mut rest = words[at + 2..].iter();
        while let Some(next) = rest.next() {
            match *next {
                "--store" => {
                    rest.next();
                }
                flag if flag.starts_with("--store=") => {}
                verb => return matches!(verb, "get" | "run"),
            }
        }
        false
    })
}

/// `text` with every one of `values` replaced by [`HIDDEN`]; `values` longest first.
pub fn redact<'a>(text: &'a str, values: &[String]) -> Cow<'a, str> {
    let mut out = Cow::Borrowed(text);
    for value in values {
        if out.contains(value.as_str()) {
            out = Cow::Owned(out.replace(value.as_str(), HIDDEN));
        }
    }
    out
}

impl Environment {
    /// Mark the binding `name` resolves to as holding a secret, declaring it if it is new.
    pub fn mark_sensitive(&mut self, name: &str) {
        self.modify_attrs(name, Attrs::SENSITIVE, Attrs::empty());
    }

    /// Take the mark off again.
    pub fn clear_sensitive(&mut self, name: &str) {
        if self.vars.contains_key(name) {
            self.modify_attrs(name, Attrs::empty(), Attrs::SENSITIVE);
        }
    }

    /// Whether `name` — or the array `name[sub]` belongs to — holds a secret.
    pub fn is_sensitive(&self, name: &str) -> bool {
        let base = name.split_once('[').map_or(name, |(base, _)| base);
        let base = base.strip_suffix('+').unwrap_or(base);
        let target = self.ref_target_name(base);
        self.vars
            .get(&*target)
            .is_some_and(|var| var.attrs.has(Attrs::SENSITIVE))
    }

    /// Mark `name` when `value` is one the secrets store handed to Lua.
    pub fn mark_if_handed_out(&mut self, name: &str, value: &str) {
        if was_handed_out(value) {
            self.mark_sensitive(name);
        }
    }

    /// Drop a binding that holds only the mark and no value: a lazy variable whose recipe failed.
    pub(crate) fn drop_empty_mark(&mut self, name: &str) {
        let bare = Attrs::SENSITIVE.union(Attrs::UNSET);
        if self.vars.get(name).is_some_and(|var| {
            var.attrs.union(bare) == bare && matches!(var.value, Value::Scalar(_))
        }) {
            self.vars.remove(name);
        }
    }

    /// Every non-empty value a sensitive binding holds, shadowed ones included, longest first.
    pub fn sensitive_values(&self) -> Vec<String> {
        let shadowed = self
            .scope_stack
            .iter()
            .flat_map(|frame| frame.saved.values().flatten());
        let mut out: Vec<String> = self
            .vars
            .values()
            .chain(shadowed)
            .filter(|var| var.attrs.has(Attrs::SENSITIVE) && var.is_set())
            .flat_map(texts)
            .filter(|text| !text.is_empty())
            .collect();
        out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        out.dedup();
        out
    }
}

/// Every string a binding holds.
fn texts(var: &Var) -> Vec<String> {
    match &var.value {
        Value::Scalar(text) => vec![text.clone()],
        Value::Indexed(array) => array.iter().map(|(_, v)| v.to_string()).collect(),
        Value::Assoc(map) => map.iter().map(|(_, v)| v.to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recipe_that_reads_the_store_is_recognised() {
        for body in [
            "$(oslo secret get gh-token)",
            "$(/usr/local/bin/oslo secret get gh-token)",
            "`oslo secret --store work get deploy`",
            "$(oslo secret --store=work run X=y -- printenv X)",
            "\"$(oslo  secret\tget x)\"",
        ] {
            assert!(recipe_reads_a_secret(body), "{body}");
        }
        for body in [
            "$(oslo secret list)",
            "$(oslo secrets get x)",
            "$(pass show oslo/key)",
            "/srv/data",
        ] {
            assert!(!recipe_reads_a_secret(body), "{body}");
        }
    }

    #[test]
    fn a_value_is_redacted_wherever_it_appears() {
        let values = vec!["ghp_long_token".to_string(), "ghp".to_string()];
        assert_eq!(
            redact("curl -H 'token ghp_long_token' ghp", &values),
            "curl -H 'token <secret>' <secret>"
        );
        assert!(matches!(redact("nothing here", &values), Cow::Borrowed(_)));
    }

    #[test]
    fn only_a_noted_value_is_recognised() {
        assert!(!was_handed_out("never-noted"));
        note(b"noted-value");
        assert!(was_handed_out("noted-value"));
        assert!(!was_handed_out("noted-valu"));
        assert!(!was_handed_out(""));
    }

    #[test]
    fn the_mark_rides_the_binding() {
        let mut env = Environment::new();
        env.set_var("TOK", "abc123", true);
        assert!(!env.is_sensitive("TOK"));
        env.mark_sensitive("TOK");
        assert!(env.is_sensitive("TOK"));
        assert!(env.is_sensitive("TOK[0]"));
        env.set_var("TOK", "def456", true);
        assert!(env.is_sensitive("TOK"), "an assignment keeps the mark");
        assert_eq!(env.get_var("TOK"), Some("def456"), "and the value is plain");
        assert_eq!(env.sensitive_values(), vec!["def456".to_string()]);
        env.unset_var("TOK");
        assert!(!env.is_sensitive("TOK"), "unset drops it");
    }

    #[test]
    fn an_unfilled_mark_is_dropped() {
        let mut env = Environment::new();
        env.mark_sensitive("NEVER_SET");
        assert!(env.variable("NEVER_SET").is_some());
        env.drop_empty_mark("NEVER_SET");
        assert!(env.variable("NEVER_SET").is_none());
    }
}
