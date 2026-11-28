//! One variable: the value it holds and the attributes `declare` gives it.
//!
//! A name holds one of three shapes — a string, an indexed array or an associative array — and a
//! set of attributes that belong to the *binding*, so a `local -i n` takes its `-i` with it when
//! the function returns. A binding can also exist without a value: `declare x`, `local x` and
//! `export x` all create one that `${x-unset}` still reports as unset.

use super::{AssocArray, ShellArray};

/// What a variable holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Scalar(String),
    Indexed(ShellArray),
    Assoc(AssocArray),
}

/// The attribute bits of one binding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attrs(u16);

impl Attrs {
    pub const EXPORT: Attrs = Attrs(1);
    pub const READONLY: Attrs = Attrs(1 << 1);
    pub const INTEGER: Attrs = Attrs(1 << 2);
    pub const LOWER: Attrs = Attrs(1 << 3);
    pub const UPPER: Attrs = Attrs(1 << 4);
    pub const CAPITAL: Attrs = Attrs(1 << 5);
    pub const TRACE: Attrs = Attrs(1 << 6);
    pub const NAMEREF: Attrs = Attrs(1 << 7);
    /// Declared but holding no value.
    pub const UNSET: Attrs = Attrs(1 << 8);
    /// Its value is a secret: traces and listings show `<secret>`. Only oslo sets it, and no
    /// `declare` letter names it.
    pub const SENSITIVE: Attrs = Attrs(1 << 9);
    /// The three case conversions, of which a binding holds at most one.
    pub const CASE: Attrs = Attrs(Self::LOWER.0 | Self::UPPER.0 | Self::CAPITAL.0);

    pub const fn empty() -> Self {
        Attrs(0)
    }

    pub const fn has(self, other: Attrs) -> bool {
        self.0 & other.0 != 0
    }

    pub fn insert(&mut self, other: Attrs) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Attrs) {
        self.0 &= !other.0;
    }

    pub const fn union(self, other: Attrs) -> Attrs {
        Attrs(self.0 | other.0)
    }
}

/// A binding: a value and its attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    pub value: Value,
    pub attrs: Attrs,
}

impl Var {
    /// A scalar with a value.
    pub fn scalar(value: impl Into<String>, attrs: Attrs) -> Self {
        Var {
            value: Value::Scalar(value.into()),
            attrs,
        }
    }

    /// A binding with no value yet.
    pub fn unset(attrs: Attrs) -> Self {
        Var {
            value: Value::Scalar(String::new()),
            attrs: attrs.union(Attrs::UNSET),
        }
    }

    pub fn is_set(&self) -> bool {
        !self.attrs.has(Attrs::UNSET)
    }

    pub fn exported(&self) -> bool {
        self.attrs.has(Attrs::EXPORT)
    }

    pub fn readonly(&self) -> bool {
        self.attrs.has(Attrs::READONLY)
    }

    pub fn is_nameref(&self) -> bool {
        self.attrs.has(Attrs::NAMEREF)
    }

    /// `$name`: a scalar's text, or element `0` of either kind of array.
    pub fn text(&self) -> Option<&str> {
        if !self.is_set() {
            return None;
        }
        match &self.value {
            Value::Scalar(text) => Some(text),
            Value::Indexed(array) => array.get(0),
            Value::Assoc(map) => map.get("0"),
        }
    }

    pub fn indexed(&self) -> Option<&ShellArray> {
        match &self.value {
            Value::Indexed(array) => Some(array),
            _ => None,
        }
    }

    pub fn assoc(&self) -> Option<&AssocArray> {
        match &self.value {
            Value::Assoc(map) => Some(map),
            _ => None,
        }
    }

    /// The attribute letters `declare -p` prints, in bash's order.
    pub fn flag_letters(&self) -> String {
        let mut out = String::new();
        let a = self.attrs;
        match self.value {
            Value::Indexed(_) => out.push('a'),
            Value::Assoc(_) => out.push('A'),
            Value::Scalar(_) => {}
        }
        for (bit, letter) in [
            (Attrs::INTEGER, 'i'),
            (Attrs::NAMEREF, 'n'),
            (Attrs::READONLY, 'r'),
            (Attrs::TRACE, 't'),
            (Attrs::EXPORT, 'x'),
            (Attrs::CAPITAL, 'c'),
            (Attrs::LOWER, 'l'),
            (Attrs::UPPER, 'u'),
        ] {
            if a.has(bit) {
                out.push(letter);
            }
        }
        out
    }
}

/// Apply a binding's case attribute to `value`, one character for one character.
///
/// `ß` has no one-character uppercase, so it stays `ß`, as bash leaves it.
pub fn convert_case(attrs: Attrs, value: &str) -> String {
    fn upper(c: char) -> char {
        let mut up = c.to_uppercase();
        match (up.next(), up.next()) {
            (Some(u), None) => u,
            _ => c,
        }
    }
    fn lower(c: char) -> char {
        let mut low = c.to_lowercase();
        match (low.next(), low.next()) {
            (Some(l), None) => l,
            _ => c,
        }
    }
    if attrs.has(Attrs::UPPER) {
        value.chars().map(upper).collect()
    } else if attrs.has(Attrs::LOWER) {
        value.chars().map(lower).collect()
    } else if attrs.has(Attrs::CAPITAL) {
        let mut chars = value.chars();
        match chars.next() {
            Some(first) => std::iter::once(upper(first))
                .chain(chars.map(lower))
                .collect(),
            None => String::new(),
        }
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flags_print_in_bash_order() {
        let mut var = Var::scalar("1", Attrs::EXPORT.union(Attrs::READONLY));
        var.attrs.insert(Attrs::INTEGER);
        assert_eq!(var.flag_letters(), "irx");
        var.value = Value::Assoc(AssocArray::default());
        assert_eq!(var.flag_letters(), "Airx");
    }

    #[test]
    fn a_declared_binding_has_no_text() {
        assert_eq!(Var::unset(Attrs::EXPORT).text(), None);
        assert_eq!(Var::scalar("", Attrs::empty()).text(), Some(""));
    }

    #[test]
    fn case_conversion_maps_one_character_to_one() {
        assert_eq!(convert_case(Attrs::UPPER, "straße"), "STRAßE");
        assert_eq!(convert_case(Attrs::LOWER, "AbC"), "abc");
        assert_eq!(convert_case(Attrs::CAPITAL, "hello wORLD"), "Hello world");
    }
}
