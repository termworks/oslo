//! Associative arrays, iterated in the order bash's hash table walks them.
//!
//! bash keeps an associative array in a chained hash table of 1024 buckets: the key is hashed with
//! 32-bit FNV-1 over its bytes (each byte sign-extended, as a C `char` is), a new key goes to the
//! *head* of its bucket, and the table grows fourfold once it holds twice as many keys as buckets,
//! re-inserting every entry at the head of its new bucket in walk order. `${!m[@]}` walks the
//! buckets in order and each bucket from its head. Reproducing that is what makes `declare -p m`
//! and every loop over `"${!m[@]}"` print what bash prints.
//!
//! An entry's place in that walk is its bucket and a rank that falls with every insertion, so the
//! walk is a `BTreeMap` keyed by `(bucket, rank)`; lookups go through a `HashMap` beside it.

use std::collections::{BTreeMap, HashMap};

const INITIAL_BUCKETS: u32 = 1024;

/// bash's `hash_string`: FNV-1, multiply then xor, over sign-extended bytes.
fn hash(key: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for byte in key.bytes() {
        h = h.wrapping_mul(16_777_619);
        h ^= byte as i8 as i32 as u32;
    }
    h
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    bucket: u32,
    rank: i64,
    value: String,
}

/// A map from string keys to string values, in bash's iteration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssocArray {
    entries: HashMap<String, Entry>,
    order: BTreeMap<(u32, i64), String>,
    buckets: u32,
    next_rank: i64,
}

impl Default for AssocArray {
    fn default() -> Self {
        AssocArray {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            buckets: INITIAL_BUCKETS,
            next_rank: 0,
        }
    }
}

impl AssocArray {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|e| e.value.as_str())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Store `value` under `key`. An existing key keeps its place in the walk.
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.value = value.into();
            return;
        }
        if self.entries.len() as u64 >= u64::from(self.buckets) * 2 {
            self.grow();
        }
        let bucket = hash(key) & (self.buckets - 1);
        let rank = self.take_rank();
        self.order.insert((bucket, rank), key.to_string());
        self.entries.insert(
            key.to_string(),
            Entry {
                bucket,
                rank,
                value: value.into(),
            },
        );
    }

    pub fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.order.remove(&(entry.bucket, entry.rank));
        }
    }

    /// The keys, in walk order. `${!m[@]}`.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.order.values().map(String::as_str)
    }

    /// The values, in walk order. `${m[@]}`.
    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.keys().map(|key| self.entries[key].value.as_str())
    }

    /// Key and value pairs, in walk order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.keys()
            .map(|key| (key, self.entries[key].value.as_str()))
    }

    /// Each new head of a bucket ranks below every entry already there.
    fn take_rank(&mut self) -> i64 {
        self.next_rank -= 1;
        self.next_rank
    }

    /// bash's `hash_rehash`: four times the buckets, each entry re-inserted at a head in walk order.
    fn grow(&mut self) {
        self.buckets *= 4;
        let walk: Vec<String> = self.order.values().cloned().collect();
        self.order.clear();
        for key in walk {
            let bucket = hash(&key) & (self.buckets - 1);
            let rank = self.take_rank();
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.bucket = bucket;
                entry.rank = rank;
            }
            self.order.insert((bucket, rank), key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AssocArray;

    fn keys_after(inserted: &[&str]) -> Vec<String> {
        let mut map = AssocArray::default();
        for key in inserted {
            map.set(key, "1");
        }
        map.keys().map(str::to_string).collect()
    }

    /// Orders read from bash 5.3 itself.
    #[test]
    fn the_walk_matches_bash() {
        assert_eq!(keys_after(&["a", "b", "c"]), ["c", "b", "a"]);
        assert_eq!(
            keys_after(&["a b", "x$y", "-", "foo", "bar", "baz", "qux"]),
            ["x$y", "-", "qux", "foo", "bar", "baz", "a b"]
        );
        assert_eq!(
            keys_after(&["é", "ü", "ß", "aé", "bü", "x", "y", "z", "日本", "ñ"]),
            ["日本", "z", "y", "x", "aé", "bü", "ß", "ñ", "ü", "é"]
        );
    }

    #[test]
    fn an_existing_key_keeps_its_place_and_a_removed_one_comes_back_first() {
        let mut map = AssocArray::default();
        for key in ["a", "b", "c"] {
            map.set(key, "1");
        }
        map.set("a", "9");
        assert_eq!(map.keys().collect::<Vec<_>>(), ["c", "b", "a"]);
        assert_eq!(map.get("a"), Some("9"));
        map.remove("c");
        assert_eq!(map.len(), 2);
        map.set("c", "1");
        assert_eq!(map.keys().collect::<Vec<_>>(), ["c", "b", "a"]);
    }

    /// Past 2048 keys the table grows, and the walk is still bash's.
    #[test]
    fn growing_keeps_every_entry() {
        let mut map = AssocArray::default();
        for i in 0..5000 {
            map.set(&format!("key{i}"), i.to_string());
        }
        assert_eq!(map.len(), 5000);
        assert_eq!(map.get("key4321"), Some("4321"));
        assert_eq!(
            map.keys().take(6).collect::<Vec<_>>(),
            [
                "key4327", "key4326", "key4325", "key4324", "key4323", "key2479"
            ]
        );
    }
}
