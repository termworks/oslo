//! Indexed arrays.
//!
//! An array is **sparse**, which is why it is a `BTreeMap` and not a `Vec`: `a[5]=x` leaves
//! `${#a[@]}` at 1 and `${!a[@]}` at `5`, and `unset 'a[1]'` punches a hole that the remaining
//! indices keep their old numbers across.
//!
//! Arrays and scalars share one namespace, exactly as in bash: `a=(1 2 3); echo "$a"` prints `1`
//! and `a=(1 2 3); a=4` replaces element 0 only.

use std::collections::BTreeMap;

/// An indexed array: a sparse map from index to element.
///
/// Indices are `i64` because bash's are, and because a negative subscript counts back from the
/// end. Nothing negative is ever stored: a subscript is resolved against the highest index in use
/// before it reaches the map.
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

    /// Index and element pairs, ascending.
    pub fn iter(&self) -> impl Iterator<Item = (i64, &str)> {
        self.elements.iter().map(|(i, v)| (*i, v.as_str()))
    }

    /// The highest index in use, if any element exists.
    pub fn max_index(&self) -> Option<i64> {
        self.elements.keys().next_back().copied()
    }

    /// Turn a subscript as written into the index it names, or `None` if it points before the
    /// start. A negative subscript counts back from the highest index in use, so `${a[-1]}` is
    /// the last element even when the array is sparse.
    pub fn resolve(&self, index: i64) -> Option<i64> {
        if index >= 0 {
            return Some(index);
        }
        let resolved = self.max_index().unwrap_or(-1) + 1 + index;
        (resolved >= 0).then_some(resolved)
    }

    /// One element, or `None` when nothing was ever stored there.
    pub fn get(&self, index: i64) -> Option<&str> {
        let index = self.resolve(index)?;
        self.elements.get(&index).map(String::as_str)
    }

    /// Store one element, replacing whatever was there. A subscript before the start is ignored;
    /// callers that must report it check [`Self::resolve`] first.
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
        self.max_index().map_or(0, |highest| highest + 1)
    }

    /// Remove one element, leaving a hole. `unset 'a[1]'`.
    pub fn remove(&mut self, index: i64) {
        if let Some(index) = self.resolve(index) {
            self.elements.remove(&index);
        }
    }

    /// The elements joined by `sep` — `${a[*]}`.
    pub fn joined(&self, sep: &str) -> String {
        self.values().collect::<Vec<_>>().join(sep)
    }

    /// `${a[@]:offset:count}`: the elements from *index* `offset` on, `count` of them.
    ///
    /// The offset is an index rather than a position, so a hole shifts nothing: with elements at
    /// 1, 5 and 7, `:2` starts at 5. A negative offset counts back from one past the highest
    /// index, and one that lands before 0 selects nothing.
    pub fn slice(&self, offset: i64, count: Option<usize>) -> Vec<String> {
        let start = match offset {
            o if o < 0 => self.max_index().unwrap_or(-1) + 1 + o,
            o => o,
        };
        if start < 0 {
            return Vec::new();
        }
        self.elements
            .range(start..)
            .take(count.unwrap_or(usize::MAX))
            .map(|(_, v)| v.clone())
            .collect()
    }
}

/// Whether `text` is an array literal — `(…)` — and if so, what is inside it.
///
/// The declaration builtins receive their operands as already-joined `name=value` strings, so this
/// is the one place that can tell `declare -a a=(1 2)` from `declare a='(1 2)'`.
pub fn array_literal_body(text: &str) -> Option<&str> {
    text.strip_prefix('(')?.strip_suffix(')')
}

#[cfg(test)]
mod tests {
    use super::{ShellArray, array_literal_body};

    #[test]
    fn an_array_is_sparse() {
        let mut a = ShellArray::from_values(["x"]);
        a.set(5, "y");
        assert_eq!(a.len(), 2);
        assert_eq!(a.indices().collect::<Vec<_>>(), vec![0, 5]);
        assert_eq!(a.values().collect::<Vec<_>>(), vec!["x", "y"]);
        assert_eq!(a.get(3), None);
    }

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
        assert_eq!(a.get(-4), None);
    }

    #[test]
    fn appending_follows_the_highest_index() {
        let mut a = ShellArray::default();
        a.set(9, "x");
        a.push("y");
        assert_eq!(a.indices().collect::<Vec<_>>(), vec![9, 10]);
    }

    /// Offsets are indices: elements at 1, 5 and 7 answer bash's slices.
    #[test]
    fn a_slice_counts_by_index() {
        let mut a = ShellArray::default();
        a.set(1, "x");
        a.set(5, "y");
        a.set(7, "z");
        assert_eq!(a.slice(2, None), ["y", "z"]);
        assert_eq!(a.slice(5, Some(1)), ["y"]);
        assert_eq!(a.slice(-5, None), ["y", "z"]);
        assert_eq!(a.slice(0, Some(2)), ["x", "y"]);
        assert!(a.slice(-9, None).is_empty());
    }

    #[test]
    fn an_array_literal_is_recognised_by_its_parentheses() {
        assert_eq!(array_literal_body("(1 2)"), Some("1 2"));
        assert_eq!(array_literal_body("()"), Some(""));
        assert_eq!(array_literal_body("plain"), None);
        assert_eq!(array_literal_body("(unclosed"), None);
    }
}
