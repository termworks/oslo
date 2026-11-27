//! Name references: `declare -n r=x` makes every read and write of `r` one of `x`.
//!
//! A reference may name an array element (`declare -n r='a[1]'`), and a reference may name another
//! reference. The chain is followed at most [`MAX_DEPTH`] links, as bash follows it, so a loop is
//! a name that resolves to nothing rather than a hang.

use super::Environment;
use super::value::{Value, Var};
use std::borrow::Cow;

const MAX_DEPTH: usize = 8;

/// What a reference stands for once it is followed.
pub enum Target {
    Name(String),
    Element { name: String, subscript: String },
}

/// Split `a[sub]` into its name and subscript. The `]` that closes the first `[` must end the
/// text, counting nested brackets, so `a[x]y]` is not a subscripted name.
pub fn split_subscript(text: &str) -> Option<(&str, &str)> {
    let open = text.find('[')?;
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    let close = open + i;
                    return (close + 1 == text.len())
                        .then(|| (&text[..open], &text[open + 1..close]));
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether `text` is a name a reference may hold: a variable, or one element of one.
pub fn is_reference_name(text: &str) -> bool {
    match split_subscript(text) {
        Some((name, subscript)) => super::is_valid_identifier(name) && !subscript.is_empty(),
        None => super::is_valid_identifier(text),
    }
}

impl Environment {
    /// The name a nameref holds, without following it further. `${!ref}` and `[[ -R ref ]]`.
    pub fn nameref_target(&self, name: &str) -> Option<&str> {
        let var = self.vars.get(name).filter(|v| v.is_nameref())?;
        match &var.value {
            Value::Scalar(text) if var.is_set() => Some(text),
            _ => None,
        }
    }

    /// Follow `name` through every nameref to what it finally stands for.
    ///
    /// `None` for a reference that holds no name yet, and for a chain that loops.
    pub fn deref(&self, name: &str) -> Option<Target> {
        let mut current = name.to_string();
        for _ in 0..MAX_DEPTH {
            let Some(var) = self.vars.get(&current).filter(|v| v.is_nameref()) else {
                return Some(Target::Name(current));
            };
            let next = match &var.value {
                Value::Scalar(text) if var.is_set() && !text.is_empty() => text.clone(),
                _ => return None,
            };
            if let Some((array, subscript)) = split_subscript(&next) {
                return Some(Target::Element {
                    name: array.to_string(),
                    subscript: subscript.to_string(),
                });
            }
            current = next;
        }
        None
    }

    /// The plain name `name` stands for: itself, or the variable its reference chain ends at.
    pub(crate) fn ref_target_name<'a>(&self, name: &'a str) -> Cow<'a, str> {
        if !self.vars.get(name).is_some_and(Var::is_nameref) {
            return Cow::Borrowed(name);
        }
        match self.deref(name) {
            Some(Target::Name(target)) => Cow::Owned(target),
            Some(Target::Element { name, .. }) => Cow::Owned(name),
            None => Cow::Borrowed(name),
        }
    }

    /// `$ref` where `ref` is a nameref.
    pub(super) fn read_through_ref(&self, name: &str) -> Option<&str> {
        match self.deref(name)? {
            Target::Name(target) => self.vars.get(&target)?.text(),
            Target::Element { name, subscript } => self.element_text(&name, &subscript),
        }
    }

    /// One element, with a subscript simple enough to read without evaluating arithmetic: a
    /// number, or a variable holding one. An associative array takes the subscript as its key.
    pub(crate) fn element_text(&self, name: &str, subscript: &str) -> Option<&str> {
        let var = self.vars.get(name)?;
        match &var.value {
            Value::Assoc(map) => map.get(subscript),
            Value::Indexed(array) => array.get(self.simple_index(subscript)?),
            Value::Scalar(_) => (self.simple_index(subscript)? == 0)
                .then(|| var.text())
                .flatten(),
        }
    }

    fn simple_index(&self, subscript: &str) -> Option<i64> {
        let text = subscript.trim();
        if let Ok(n) = text.parse::<i64>() {
            return Some(n);
        }
        let value = self.get_var(text).unwrap_or("0");
        value.trim().parse::<i64>().ok()
    }

    /// `ref=value` where `ref` is a nameref: the write goes to what it names, and a reference that
    /// names nothing yet takes `value` as the name to refer to.
    pub(super) fn write_through_ref(&mut self, name: &str, value: &str, export: bool) -> bool {
        match self.deref(name) {
            Some(Target::Name(target)) => self.set_var(&target, value, export),
            Some(Target::Element { name, subscript }) => {
                crate::env::builtins::arrays::assign_element(self, &name, &subscript, value)
                    .unwrap_or(false)
            }
            None => self.set_reference(name, value),
        }
    }

    /// Point the nameref `name` at `target`.
    pub fn set_reference(&mut self, name: &str, target: &str) -> bool {
        if !is_reference_name(target) {
            eprintln!(
                "{}`{target}': invalid variable name for name reference",
                self.origin()
            );
            return false;
        }
        if let Some(var) = self.vars.get_mut(name) {
            var.value = Value::Scalar(target.to_string());
            var.attrs.remove(super::value::Attrs::UNSET);
        }
        true
    }
}
