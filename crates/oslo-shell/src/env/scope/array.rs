//! Indexed arrays.
//!
//! An array is **sparse**, which is the whole reason it is a `BTreeMap` and not a `Vec`:
//! `a[5]=x` leaves `${#a[@]}` at 1 and `${!a[@]}` at `5`, and `unset 'a[1]'` punches a hole that
//! the remaining indices keep their old numbers across. A vector would have to invent four empty
//! elements and would then answer `6` — a plausible wrong number, which is the failure mode this
//! shell is being audited for.
//!
//! Arrays and scalars share one namespace, exactly as in bash. `a=(1 2 3); echo "$a"` prints `1`
//! and `a=(1 2 3); a=4` replaces element 0 only, so an array's element 0 is *not* mirrored into
//! the scalar table: [`Environment::get_var`] and [`Environment::set_var`] consult the array store
//! first and every other caller inherits the behaviour for free.
//!
//! Associative arrays (`declare -A`) are deliberately absent — see PLAN.md's "Not doing" table.
//! `declare -A` reports that rather than building a second value shape nothing else understands.

use super::Environment;
use super::special::ArrayWrite;
use std::collections::BTreeMap;

/// An indexed array: a sparse map from index to element.
///
/// Indices are `i64` because bash's are, and because a negative subscript on *read* counts back
/// from the end. Nothing negative is ever stored: a subscript is resolved against the
/// highest index in use before it reaches the map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellArray {
    elements: BTreeMap<i64, String>,
}

impl ShellArray {
    /// Build a dense array from `values`, numbered from 0 — what `a=(x y z)` produces.
    pub fn from_values<I, S>(values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            elements: values
                .into_iter()
                .enumerate()
                .map(|(i, v)| (i as i64, v.into()))
                .collect(),
        }
    }

    /// How many elements exist — the count, not the highest index. `${#a[@]}`.
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// The indices in use, ascending. `${!a[@]}`.
    pub fn indices(&self) -> impl Iterator<Item = i64> + '_ {
        self.elements.keys().copied()
    }

    /// Every element in index order. `${a[@]}`.
    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.elements.values().map(String::as_str)
    }

    /// Turn a subscript as written into the index it names, or `None` if it points before the
    /// start. A negative subscript counts back from the highest index in use, so `${a[-1]}` is
    /// the last element even when the array is sparse.
    fn resolve(&self, index: i64) -> Option<i64> {
        if index >= 0 {
            return Some(index);
        }
        let highest = self.elements.keys().next_back().copied().unwrap_or(-1);
        let resolved = highest + 1 + index;
        (resolved >= 0).then_some(resolved)
    }

    /// One element, or `None` when nothing was ever stored there.
    pub fn get(&self, index: i64) -> Option<&str> {
        let index = self.resolve(index)?;
        self.elements.get(&index).map(String::as_str)
    }

    /// Store one element, replacing whatever was there.
    pub fn set(&mut self, index: i64, value: impl Into<String>) {
        if let Some(index) = self.resolve(index) {
            self.elements.insert(index, value.into());
        }
    }

    /// Append after the highest index in use — what `a+=(x)` and an unindexed element of an array
    /// literal both need.
    pub fn push(&mut self, value: impl Into<String>) {
        let next = self.next_index();
        self.elements.insert(next, value.into());
    }

    /// The index `a+=(x)` would write to.
    pub fn next_index(&self) -> i64 {
        self.elements
            .keys()
            .next_back()
            .map_or(0, |highest| highest + 1)
    }

    /// Remove one element, leaving a hole. `unset 'a[1]'`.
    pub fn remove(&mut self, index: i64) {
        if let Some(index) = self.resolve(index) {
            self.elements.remove(&index);
        }
    }

    /// The elements joined by `sep` — `${a[*]}`, and what a `declare -p` line renders from.
    pub fn joined(&self, sep: &str) -> String {
        self.values().collect::<Vec<_>>().join(sep)
    }
}

/// Whether `text` is an array literal — `(…)` — and if so, what is inside it.
///
/// The declaration builtins receive their operands as already-joined `name=value` strings, so this
/// is the one place that can tell `declare -a a=(1 2)` from `declare a='(1 2)'`. Only used there;
/// an assignment in the AST carries its shape explicitly.
pub fn array_literal_body(text: &str) -> Option<&str> {
    text.strip_prefix('(')?.strip_suffix(')')
}

impl Environment {
    /// The array stored under `name`, if any.
    pub fn get_array(&self, name: &str) -> Option<&ShellArray> {
        self.arrays.get(name)
    }

    /// Replace `name`'s value with `array`, discarding any scalar of the same name.
    ///
    /// The entry point another subsystem uses to publish a computed array: `PIPESTATUS` is written
    /// here by [`Environment::set_pipeline_status`], and `BASH_REMATCH` is written here by
    /// `[[ =~ ]]`. `false` if the variable is read-only.
    pub fn set_array(&mut self, name: &str, array: ShellArray) -> bool {
        if self.is_readonly(name) {
            crate::env::complain_from(
                &self.origin(),
                &[name.to_string()],
                name,
                &format!("{name}: is read only"),
                "readonly",
                Some(crate::env::scope::vars::READONLY),
            );
            return false;
        }
        if let Some(done) = self.special_array_write(name, ArrayWrite::Whole(&array)) {
            return done;
        }
        // A name can be a scalar or an array, never both: leaving the scalar behind would make
        // `get_var` answer with the stale string it happens to check first.
        self.drop_scalar(name);
        self.arrays.insert(name.to_string(), array);
        true
    }

    /// Set one element, promoting a scalar to element 0 first.
    ///
    /// `b=hello; b[2]=world` leaves `${b[@]}` as `hello world`, so the scalar is not discarded —
    /// it becomes element 0, which is what it already was as far as `${b[0]}` was concerned.
    pub fn set_array_element(&mut self, name: &str, index: i64, value: &str) -> bool {
        if self.is_readonly(name) {
            crate::env::complain_from(
                &self.origin(),
                &[name.to_string()],
                name,
                &format!("{name}: is read only"),
                "readonly",
                Some(crate::env::scope::vars::READONLY),
            );
            return false;
        }
        if let Some(done) = self.special_array_write(name, ArrayWrite::Element(index, value)) {
            return done;
        }
        self.promote_scalar(name);
        self.arrays
            .entry(name.to_string())
            .or_default()
            .set(index, value);
        true
    }

    /// Append one element after the highest index in use.
    pub fn append_array_element(&mut self, name: &str, value: &str) -> bool {
        if self.is_readonly(name) {
            crate::env::complain_from(
                &self.origin(),
                &[name.to_string()],
                name,
                &format!("{name}: is read only"),
                "readonly",
                Some(crate::env::scope::vars::READONLY),
            );
            return false;
        }
        if let Some(done) = self.special_array_write(name, ArrayWrite::Append) {
            return done;
        }
        self.promote_scalar(name);
        self.arrays.entry(name.to_string()).or_default().push(value);
        true
    }

    /// Make `name` an array if it is not one already, without disturbing an existing value.
    /// `declare -a name`.
    pub fn declare_array(&mut self, name: &str) {
        self.promote_scalar(name);
        self.arrays.entry(name.to_string()).or_default();
    }

    /// Drop one element. `unset 'a[1]'`.
    pub fn unset_array_element(&mut self, name: &str, index: i64) {
        if self.special_array_write(name, ArrayWrite::Unset).is_some() {
            return;
        }
        if let Some(array) = self.arrays.get_mut(name) {
            array.remove(index);
        }
    }

    /// Every array name, for `declare -p` with no operands.
    pub fn array_names(&self) -> impl Iterator<Item = &str> {
        self.arrays.keys().map(String::as_str)
    }

    /// Turn an existing scalar into element 0 of a new array.
    fn promote_scalar(&mut self, name: &str) {
        if self.arrays.contains_key(name) {
            return;
        }
        let existing = self.get_var(name).map(str::to_string);
        self.drop_scalar(name);
        let mut array = ShellArray::default();
        if let Some(value) = existing {
            array.set(0, value);
        }
        self.arrays.insert(name.to_string(), array);
    }
}

#[cfg(test)]
mod tests {
    use super::{ShellArray, array_literal_body};
    use crate::env::Environment;

    #[test]
    fn an_array_is_sparse() {
        let mut a = ShellArray::from_values(["x"]);
        a.set(5, "y");
        // The count is the number of elements, not the highest index plus one.
        assert_eq!(a.len(), 2);
        assert_eq!(a.indices().collect::<Vec<_>>(), vec![0, 5]);
        assert_eq!(a.values().collect::<Vec<_>>(), vec!["x", "y"]);
        assert_eq!(a.get(3), None);
    }

    /// A hole keeps the indices on either side of it where they were.
    #[test]
    fn removing_an_element_leaves_a_hole() {
        let mut a = ShellArray::from_values(["a", "b", "c"]);
        a.remove(1);
        assert_eq!(a.indices().collect::<Vec<_>>(), vec![0, 2]);
        assert_eq!(a.joined(" "), "a c");
    }

    #[test]
    fn a_negative_subscript_counts_back_from_the_end() {
        let a = ShellArray::from_values(["1", "2", "3"]);
        assert_eq!(a.get(-1), Some("3"));
        assert_eq!(a.get(-3), Some("1"));
        // Before the start is not an error, only an absent element.
        assert_eq!(a.get(-4), None);
    }

    /// Appending goes after the highest index, not after the count.
    #[test]
    fn appending_follows_the_highest_index() {
        let mut a = ShellArray::default();
        a.set(9, "x");
        a.push("y");
        assert_eq!(a.indices().collect::<Vec<_>>(), vec![9, 10]);
    }

    /// A scalar and an array share one name: assigning an array replaces the scalar, and a
    /// scalar assignment afterwards writes element 0 instead of destroying the array.
    #[test]
    fn a_scalar_and_an_array_share_one_name() {
        let mut env = Environment::new();
        env.set_var("oslo_a1", "scalar", false);
        env.set_array("oslo_a1", ShellArray::from_values(["1", "2", "3"]));
        assert_eq!(env.get_var("oslo_a1"), Some("1"));
        env.set_var("oslo_a1", "4", false);
        assert_eq!(env.get_array("oslo_a1").unwrap().joined(" "), "4 2 3");
    }

    /// `b=hello; b[2]=world` keeps the scalar as element 0.
    #[test]
    fn an_element_assignment_promotes_an_existing_scalar() {
        let mut env = Environment::new();
        env.set_var("oslo_a2", "hello", false);
        env.set_array_element("oslo_a2", 2, "world");
        let a = env.get_array("oslo_a2").unwrap();
        assert_eq!(a.joined(" "), "hello world");
        assert_eq!(a.indices().collect::<Vec<_>>(), vec![0, 2]);
    }

    #[test]
    fn unsetting_a_variable_removes_its_array_too() {
        let mut env = Environment::new();
        env.set_array("oslo_a3", ShellArray::from_values(["x"]));
        env.unset_var("oslo_a3");
        assert!(env.get_array("oslo_a3").is_none());
        assert_eq!(env.get_var("oslo_a3"), None);
    }

    /// The contract another subsystem publishes a computed array through.
    ///
    /// `PIPESTATUS` is written by `Environment::set_pipeline_status`; `BASH_REMATCH` is written
    /// by `[[ =~ ]]` with the whole match at index 0 and each capture group after it. Nothing
    /// else is needed on either side: once the array is in, `${BASH_REMATCH[1]}`, `${#…[@]}` and
    /// `"${…[@]}"` all work through the ordinary expansion path.
    #[test]
    fn a_computed_array_is_published_through_set_array() {
        let mut env = Environment::new();
        env.set_array(
            "BASH_REMATCH",
            ShellArray::from_values(["2024-06", "2024", "06"]),
        );
        let matched = env.get_array("BASH_REMATCH").expect("published");
        assert_eq!(matched.get(0), Some("2024-06"));
        assert_eq!(matched.get(1), Some("2024"));
        assert_eq!(matched.len(), 3);
        // …and `$BASH_REMATCH` unsubscripted is the whole match, as bash has it.
        assert_eq!(env.get_var("BASH_REMATCH"), Some("2024-06"));
    }

    /// Every pipeline publishes `PIPESTATUS`, so `${PIPESTATUS[@]}` needs nothing of its own.
    #[test]
    fn the_pipeline_statuses_are_published_as_an_array() {
        let mut env = Environment::new();
        env.set_pipeline_status(vec![1, 0, 3]);
        let statuses = env.get_array("PIPESTATUS").expect("published");
        assert_eq!(statuses.joined(" "), "1 0 3");
        assert_eq!(env.pipeline_status(), &[1, 0, 3]);
    }

    #[test]
    fn an_array_literal_is_recognised_by_its_parentheses() {
        assert_eq!(array_literal_body("(1 2)"), Some("1 2"));
        assert_eq!(array_literal_body("()"), Some(""));
        assert_eq!(array_literal_body("plain"), None);
        assert_eq!(array_literal_body("(unclosed"), None);
    }
}
