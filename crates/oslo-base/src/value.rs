//! The dynamic value the shell and its Lua share.
//!
//! **Here rather than in the Lua crate, because most of its users are not Lua.** The structured
//! pipeline builds these for `ls` and `ps`, settings are read into them, hooks and reports carry
//! them — forty-odd files that have no VM anywhere in sight. `luna`'s own values cannot serve: they
//! carry a garbage-collector lifetime and exist only inside `Lua::enter`, so a `Table` cannot be
//! returned, stored in a struct, or built by `data::rows` at all. The two are converted at the one
//! place they meet, in `oslo-runtime`'s Lua layer.
//!
//! Lua 5.4 has six types a script can hold: nil, boolean, number, string, table and function.
//! Two of those need care.
//!
//! **Number is one type with two subtypes.** `3` is an integer and `3.0` is a float, they compare
//! equal, and `//` and `%` behave differently for each. Collapsing them to `f64` would be simpler
//! and would quietly change what `1//0`, `math.maxinteger` and `string.format("%d", x)` do — so
//! the distinction is kept, as [`Number`](value::Number).
//!
//! **Tables are shared, mutable and cyclic.** `a = {}; b = a; b.x = 1` must be visible through
//! `a`, and `t.self = t` is legal Lua. That is `Rc<RefCell<…>>`, and it means reference cycles
//! leak. Real Lua collects them with a tracing GC; oslo does not have one, which is a deliberate
//! limit recorded in PLAN.md rather than an oversight. A shell script's tables are small and
//! short-lived, and the alternative is writing a garbage collector.

pub mod error;
mod number;

pub use number::Number;

pub use error::{LuaError, LuaResult};

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

/// A key in a table's hash part.
///
/// Its own type because `Value` cannot be `Hash`: floats are not, and tables hash by identity
/// rather than contents. Lua also requires that `t[2]` and `t[2.0]` are the same slot, so an
/// integral float normalises to an integer on the way in — without that, `t[2.0] = 1; print(t[2])`
/// answers `nil`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Bool(bool),
    Int(i64),
    /// Non-integral floats, keyed by their bit pattern.
    Float(u64),
    Str(Rc<str>),
    /// A string key that is not text. Disjoint from [`Key::Str`] — see [`Value::Bytes`].
    Bytes(Rc<[u8]>),
    /// A table or function used as a key, identified by address.
    Ref(usize),
}

impl Key {
    /// The key `value` indexes with, or `None` for nil and NaN, which cannot be keys.
    pub fn from_value(value: &Value) -> Option<Key> {
        Some(match value {
            Value::Nil => return None,
            Value::Bool(b) => Key::Bool(*b),
            Value::Number(n) => match n.as_int() {
                Some(i) => Key::Int(i),
                None => {
                    let f = n.as_float();
                    if f.is_nan() {
                        return None;
                    }
                    Key::Float(f.to_bits())
                }
            },
            Value::Str(s) => Key::Str(Rc::clone(s)),
            Value::Bytes(b) => Key::Bytes(Rc::clone(b)),
            Value::Table(t) => Key::Ref(Rc::as_ptr(t) as usize),
            Value::Function(f) => Key::Ref(Rc::as_ptr(f) as usize),
        })
    }

    /// The value this key stands for, for iteration.
    pub fn to_value(&self) -> Value {
        match self {
            Key::Bool(b) => Value::Bool(*b),
            Key::Int(i) => Value::Number(Number::Int(*i)),
            Key::Float(bits) => Value::Number(Number::Float(f64::from_bits(*bits))),
            Key::Str(s) => Value::Str(Rc::clone(s)),
            Key::Bytes(b) => Value::Bytes(Rc::clone(b)),
            // A table used as a key cannot be recovered from its address alone; the table stores
            // the original alongside it. See `Table::pairs`.
            Key::Ref(_) => Value::Nil,
        }
    }
}

/// A Lua table: the array part, the hash part, and an optional metatable.
///
/// The array part exists because `#t`, `ipairs` and `table.insert` are all sequence operations,
/// and doing them through a hash map makes every one of them O(n) with a hash per element.
#[derive(Debug, Default)]
pub struct Table {
    /// Values at 1..=n, Lua's sequence part.
    array: Vec<Value>,
    hash: HashMap<Key, Value>,
    /// The hash part's keys in the order they were first written.
    ///
    /// **Lua does not promise an order for `pairs`, but a shell has to have one.** Without this,
    /// iteration follows the hash map, so a table built the same way twice iterates differently
    /// between runs — a config that walks a settings table gets a different order each start, and
    /// a record printed as columns prints them shuffled. Both are the kind of bug that is noticed
    /// long after it is introduced and blamed on something else.
    ///
    /// Kept beside the map rather than replacing it: lookup stays O(1), and only iteration and
    /// removal pay anything.
    order: Vec<Key>,
    /// Keys that are tables or functions, kept so iteration can hand back the original value.
    key_objects: HashMap<usize, Value>,
    pub metatable: Option<Rc<RefCell<Table>>>,
}

impl Table {
    pub fn new() -> Self {
        Self::default()
    }

    /// Raw read under a literal name — `t.get_str("size")`.
    ///
    /// **Not inlined, and that is the whole point.** `Value::str` allocates an `Rc<str>` and copies
    /// the name into it, and the lookup then clones a key and drops two values. Written out at each
    /// of the two hundred-odd call sites that pass a literal, that is two hundred copies of the
    /// same dozen instructions, each with its own unwind landing pads. Behind `#[inline(never)]`
    /// there is one — measured at 28,208 bytes of the shipped binary.
    ///
    /// It also reads better, which is why the call sites were worth changing rather than leaving to
    /// a smarter allocator: `t.get_str("size")` says what `t.get_str("size")` meant.
    #[inline(never)]
    pub fn get_str(&self, key: &str) -> Value {
        self.get(&Value::str(key))
    }

    /// Raw write under a literal name. The counterpart of [`get_str`](Self::get_str).
    #[inline(never)]
    pub fn set_str(&mut self, key: &str, value: Value) {
        self.set(Value::str(key), value);
    }

    /// Raw read, ignoring any `__index` metamethod.
    pub fn get(&self, key: &Value) -> Value {
        if let Value::Number(n) = key
            && let Some(i) = n.as_int()
            && i >= 1
            && (i as usize) <= self.array.len()
        {
            return self.array[i as usize - 1].clone();
        }
        Key::from_value(key)
            .and_then(|k| self.hash.get(&k).cloned())
            .unwrap_or(Value::Nil)
    }

    /// Raw write, ignoring any `__newindex` metamethod.
    ///
    /// Assigning nil *removes* the key: in Lua `t.x = nil` and "never assigned" are the same
    /// state, which is why `next` never yields a nil value.
    pub fn set(&mut self, key: Value, value: Value) {
        if let Value::Number(n) = &key
            && let Some(i) = n.as_int()
            && i >= 1
        {
            let index = i as usize;
            if index <= self.array.len() {
                self.array[index - 1] = value;
                // A nil at the end shortens the sequence; one in the middle leaves a hole, which
                // Lua explicitly leaves undefined for `#`.
                while matches!(self.array.last(), Some(Value::Nil)) {
                    self.array.pop();
                }
                return;
            }
            if index == self.array.len() + 1 && !matches!(value, Value::Nil) {
                self.array.push(value);
                // The hash part may hold the elements that follow; migrate them across so that
                // filling a gap joins the two halves rather than leaving `#t` short.
                let mut next = self.array.len() as i64 + 1;
                while let Some(v) = self.hash.remove(&Key::Int(next)) {
                    self.order.retain(|k| k != &Key::Int(next));
                    self.array.push(v);
                    next += 1;
                }
                return;
            }
        }

        let Some(k) = Key::from_value(&key) else {
            return;
        };
        if let Key::Ref(addr) = &k {
            self.key_objects.insert(*addr, key.clone());
        }
        if matches!(value, Value::Nil) {
            self.hash.remove(&k);
            self.order.retain(|existing| existing != &k);
            if let Key::Ref(addr) = &k {
                self.key_objects.remove(addr);
            }
        } else {
            // Only a key that is genuinely new joins the order. Assigning over an existing key
            // leaves it where it was, which is what someone editing a table expects: changing a
            // value should not move the column.
            if self.hash.insert(k.clone(), value).is_none() {
                self.order.push(k);
            }
        }
    }

    /// The border `#t` reports.
    pub fn length(&self) -> i64 {
        self.array.len() as i64
    }

    /// Every key/value pair, for `pairs`. Array part first, in order.
    pub fn pairs(&self) -> Vec<(Value, Value)> {
        let mut out: Vec<(Value, Value)> = self
            .array
            .iter()
            .enumerate()
            .filter(|(_, v)| !matches!(v, Value::Nil))
            .map(|(i, v)| (Value::Number(Number::Int(i as i64 + 1)), v.clone()))
            .collect();
        // Insertion order, not the map's. See `order`.
        for k in &self.order {
            let Some(v) = self.hash.get(k) else {
                continue;
            };
            let key = match k {
                Key::Ref(addr) => self.key_objects.get(addr).cloned().unwrap_or(Value::Nil),
                other => other.to_value(),
            };
            out.push((key, v.clone()));
        }
        out
    }

    /// The sequence part, for `ipairs` and for handing a table to the core as an argv vector.
    pub fn sequence(&self) -> &[Value] {
        &self.array
    }
}

/// **Tearing a table down must not recurse, because a script chooses how deep one is.**
///
/// A table holds tables, so the derived teardown freed a child from inside freeing its parent:
/// twenty-five thousand levels — which `for i = 1, 30000 do c.n = {}; c = c.n end` writes in a
/// line — exhausted the stack and aborted the process. A shell cannot report that; the session is
/// simply gone, and it happened while *dropping* a value, which is to say at a point no `Result`
/// reaches and no caller can guard.
///
/// The children are moved onto a queue and freed from there, so the depth costs heap rather than
/// frames. **What is freed, and when, is exactly what it was**: [`Rc::try_unwrap`] hands back the
/// contents only for the last owner, so a table another value still holds is left alone, and a
/// table that holds itself never reaches a count of one and leaks — which is what an `Rc` cycle
/// already did. Only the recursion is gone.
impl Drop for Table {
    fn drop(&mut self) {
        let mut waiting = Vec::new();
        take_tables(self, &mut waiting);
        while let Some(child) = waiting.pop() {
            // Not the last owner: someone else's to free, exactly as before.
            let Ok(cell) = Rc::try_unwrap(child) else {
                continue;
            };
            let mut inner = cell.into_inner();
            take_tables(&mut inner, &mut waiting);
            // `inner` is dropped here with nothing nested left in it, so its own `drop` finds an
            // empty queue and stops — one frame, not one per level.
        }
    }
}

/// Move every table this one holds onto `waiting`, leaving it with none.
///
/// Every field that can carry one: the sequence, the hash part, the objects kept so a table used
/// as a *key* can be handed back, and the metatable.
fn take_tables(table: &mut Table, waiting: &mut Vec<Rc<RefCell<Table>>>) {
    let mut take = |value: Value| {
        if let Value::Table(nested) = value {
            waiting.push(nested);
        }
    };
    for value in table.array.drain(..) {
        take(value);
    }
    for (_, value) in table.hash.drain() {
        take(value);
    }
    for (_, value) in table.key_objects.drain() {
        take(value);
    }
    table.order.clear();
    if let Some(meta) = table.metatable.take() {
        waiting.push(meta);
    }
}

/// A callable.
///
/// `Native` is the whole reason this evaluator exists: it is how Lua reaches oslo's Rust core, so
/// that `sh.ls()` and `ls` in shell syntax run the same code.
pub enum Function {
    /// A Rust function, named for what an error message has to be able to say.
    Native { name: &'static str },
    /// Something callable that lives inside the VM.
    ///
    /// Opaque on purpose: a Lua closure belongs to the interpreter's heap and cannot be represented
    /// here without dragging its lifetime along. The Lua layer puts a stashed handle in and takes it
    /// back out; everything else only ever asks *whether* a value is callable, which is the whole of
    /// what `matches!(v, Value::Function(_))` needs.
    Held(Rc<dyn Any>),
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Function::Native { name } => write!(f, "native {name}"),
            Function::Held(_) => write!(f, "lua function"),
        }
    }
}

/// A Lua value.
#[derive(Debug, Clone, Default)]
pub enum Value {
    #[default]
    Nil,
    Bool(bool),
    Number(Number),
    Str(Rc<str>),
    /// A string that is *not* text: bytes that are not valid UTF-8.
    ///
    /// **The invariant is what makes two variants safe.** A Lua string is a byte string and there is
    /// exactly one of them, so `t["a"]` and `t[<the bytes "a">]` have to be the same key and have to
    /// compare equal. That would be a trap if the two variants overlapped — and they cannot, because
    /// [`Value::bytes`] is the only way to build this one and it hands valid UTF-8 to
    /// [`Value::Str`]. Valid and invalid UTF-8 are disjoint, so no `Bytes` can name the same string
    /// as any `Str`, and equality and keying stay exactly Lua's.
    ///
    /// What it buys is that `oslo.fs.read` on a PNG answers the PNG. Before this the boundary went
    /// through `String::from_utf8_lossy` and every byte that was not text came back as `U+FFFD` —
    /// a read that looked like it worked and had quietly changed the file.
    ///
    /// Everything that matches `Value::Str` and falls through to a catch-all therefore treats this
    /// as "not text", which is right: a path, a variable name and a command word are all text, and
    /// a caller handing one of them a run of arbitrary bytes has made a mistake worth a message.
    Bytes(Rc<[u8]>),
    Table(Rc<RefCell<Table>>),
    Function(Rc<Function>),
}

impl Value {
    pub fn str(s: impl AsRef<str>) -> Value {
        Value::Str(Rc::from(s.as_ref()))
    }

    /// A Lua string from bytes: text when they are text, and the bytes themselves when they are not.
    ///
    /// The only constructor of [`Value::Bytes`], which is what holds its invariant. See there.
    pub fn bytes(b: impl AsRef<[u8]>) -> Value {
        let b = b.as_ref();
        match std::str::from_utf8(b) {
            Ok(text) => Value::Str(Rc::from(text)),
            Err(_) => Value::Bytes(Rc::from(b)),
        }
    }

    /// The bytes of a string value, whichever variant carries them.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Str(s) => Some(s.as_bytes()),
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn int(i: i64) -> Value {
        Value::Number(Number::Int(i))
    }

    pub fn float(f: f64) -> Value {
        Value::Number(Number::Float(f))
    }

    pub fn table(t: Table) -> Value {
        Value::Table(Rc::new(RefCell::new(t)))
    }

    /// Lua's truth: only `nil` and `false` are false.
    ///
    /// Notably `0` and `""` are **true**, which is the opposite of most scripting languages and
    /// the single most common source of ported-code bugs.
    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// The name `type()` reports.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::Str(_) | Value::Bytes(_) => "string",
            Value::Table(_) => "table",
            Value::Function(_) => "function",
        }
    }

    /// Lua's `==`.
    ///
    /// Numbers compare across subtypes (`1 == 1.0` is true); tables and functions compare by
    /// identity, never by contents.
    pub fn lua_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => match (a.as_int(), b.as_int()) {
                (Some(x), Some(y)) => x == y,
                _ => a.as_float() == b.as_float(),
            },
            (Value::Str(a), Value::Str(b)) => a == b,
            // Never equal to a `Str`: one is valid UTF-8 and the other is not. See `Value::Bytes`.
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::Table(a), Value::Table(b)) => Rc::ptr_eq(a, b),
            (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// The string `tostring()` gives, before any `__tostring` metamethod.
    pub fn to_display(&self) -> String {
        match self {
            Value::Nil => "nil".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::Str(s) => s.to_string(),
            // Lossy, and only here: `tostring` has to answer *something*, and this is the one
            // place a byte string is being asked for its rendering rather than its contents.
            Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            Value::Table(t) => format!("table: {:p}", Rc::as_ptr(t)),
            Value::Function(f) => format!("function: {:p}", Rc::as_ptr(f)),
        }
    }

    /// Lua's automatic string-to-number coercion, used by arithmetic.
    pub fn as_number(&self) -> Option<Number> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Str(s) => parse_number(s),
            _ => None,
        }
    }
}

/// Parse a Lua numeric literal: decimal, hex, and floats with exponents.
pub fn parse_number(text: &str) -> Option<Number> {
    let t = text.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16)
            .ok()
            .map(Number::Int)
            // Lua wraps a hex literal that overflows rather than rejecting it.
            .or_else(|| {
                u64::from_str_radix(hex, 16)
                    .ok()
                    .map(|u| Number::Int(u as i64))
            });
    }
    if let Some(neg) = t.strip_prefix('-')
        && let Some(hex) = neg.trim_start().strip_prefix("0x")
    {
        return i64::from_str_radix(hex, 16).ok().map(|i| Number::Int(-i));
    }
    // An integer literal stays an integer; anything with `.`, `e` or `E` is a float, even when it
    // has no fractional part, because `1e2` is a float in Lua and prints as `100.0`.
    if !t.contains(['.', 'e', 'E', 'n', 'i'])
        && let Ok(i) = t.parse::<i64>()
    {
        return Some(Number::Int(i));
    }
    t.parse::<f64>().ok().map(Number::Float)
}

#[cfg(test)]
mod tests {
    use super::{Table, Value};

    fn string_keys(t: &Table) -> Vec<String> {
        t.pairs()
            .iter()
            .filter_map(|(k, _)| match k {
                Value::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect()
    }

    /// `pairs` follows the order the keys were written in, every time.
    ///
    /// Lua promises no order here, but a shell needs one: a record's columns are drawn and
    /// serialised in iteration order, and a hash map's order changes between runs — so a table
    /// built identically twice would print its columns shuffled. This is the prerequisite the
    /// structured pipeline rests on; see `docs/features/structured-pipelines.md`.
    #[test]
    fn the_hash_part_iterates_in_insertion_order() {
        let names = [
            "filesystem",
            "size",
            "used",
            "avail",
            "use_percent",
            "mounted_on",
        ];
        // Built the same way twice: both must agree with each other and with the source order.
        for _ in 0..2 {
            let mut t = Table::new();
            for name in names {
                t.set(Value::str(name), Value::int(1));
            }
            assert_eq!(
                string_keys(&t),
                names,
                "columns keep the order they were written"
            );
        }
    }

    /// Writing over a key leaves it where it was — changing a value must not move the column.
    #[test]
    fn assigning_again_does_not_reorder() {
        let mut t = Table::new();
        for name in ["a", "b", "c"] {
            t.set(Value::str(name), Value::int(1));
        }
        t.set_str("a", Value::int(99));
        assert_eq!(string_keys(&t), ["a", "b", "c"]);
    }

    /// A removed key forgets its place, and comes back once — not twice, and not where it was.
    #[test]
    fn removing_a_key_forgets_its_place() {
        let mut t = Table::new();
        for name in ["a", "b", "c"] {
            t.set(Value::str(name), Value::Bool(true));
        }
        t.set_str("b", Value::Nil);
        assert_eq!(string_keys(&t), ["a", "c"]);
        t.set_str("b", Value::Bool(true));
        assert_eq!(string_keys(&t), ["a", "c", "b"]);
    }
}

#[cfg(test)]
#[path = "value/bytes_tests.rs"]
mod byte_string_tests;

#[cfg(test)]
#[path = "value/drop_tests.rs"]
mod drop_tests;
