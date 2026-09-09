//! Depth, in both directions.
//!
//! A script chooses how deep a table is and the stack is not a script's to spend, so neither
//! crossing may recurse per level. The two directions are separate call stacks and were fixed
//! separately; a test each keeps them that way.

use super::convert::{from_lua, into_lua};
use luna::Lua;
use oslo_base::value::{Table, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// Deeper than the interpreter's stack has frames for a per-level recursion.
const DEEP: usize = 100_000;

/// A chain of `DEEP` tables, each the sole entry of the one above it.
fn chain() -> Value {
    let mut at = Value::Bool(true);
    for _ in 0..DEEP {
        let mut table = Table::new();
        table.set_str("next", at);
        at = Value::Table(Rc::new(RefCell::new(table)));
    }
    at
}

/// How many `next` links deep a shell value goes, counted without recursing.
fn depth(value: &Value) -> usize {
    let mut at = value.clone();
    let mut levels = 0;
    while let Value::Table(table) = at {
        let next = table.borrow().get_str("next");
        if matches!(next, Value::Nil) {
            break;
        }
        at = next;
        levels += 1;
    }
    levels
}

/// The way *out*: what a Rust-implemented `oslo.*` hands back crosses here.
///
/// `oslo.state.set(key, value)` returns the value it stored, so a deep table reached this in a
/// line — in through the queue this test's sibling covers and straight back out through a
/// recursion. Both halves had to be iterative before either was safe.
#[test]
fn a_deep_table_crosses_into_the_vm_without_recursing() {
    let original = chain();
    let mut lua = Lua::core();
    lua.enter(|ctx| {
        let crossed = into_lua(ctx, &original);
        assert!(
            matches!(crossed, luna::Value::Table(_)),
            "a deep table did not cross as a table"
        );
        // Counted back on the shell's side rather than the VM's: what matters is that every level
        // arrived, and `from_lua` is the only walker here that is already known not to recurse.
        assert_eq!(depth(&from_lua(ctx, crossed)), DEEP, "levels went missing");
    });
}

/// The way *in*, kept alongside its mirror so neither direction can regress alone.
#[test]
fn a_deep_table_comes_back_from_the_vm_without_recursing() {
    let mut lua = Lua::core();
    let original = chain();
    lua.enter(|ctx| {
        let back = from_lua(ctx, into_lua(ctx, &original));
        assert_eq!(depth(&back), DEEP, "levels went missing on the way back");
    });
}

/// A table that contains itself still terminates, and still comes back containing itself.
///
/// The queue is what makes this work: the VM's table is recorded as seen *before* its entries are
/// walked, so the entry pointing back at it finds the one being built. A depth cap would answer
/// with a different value — one that no longer contains itself — and the JSON encoder, whose job
/// is to name a cyclic table, would find nothing to refuse.
#[test]
fn a_cycle_crosses_out_without_spinning() {
    let table = Rc::new(RefCell::new(Table::new()));
    table
        .borrow_mut()
        .set_str("self", Value::Table(Rc::clone(&table)));
    table.borrow_mut().set_str("tag", Value::str("cyclic"));
    let original = Value::Table(table);

    let mut lua = Lua::core();
    lua.enter(|ctx| {
        let luna::Value::Table(crossed) = into_lua(ctx, &original) else {
            panic!("a cyclic table did not cross as a table");
        };
        let inner = crossed.get(
            ctx,
            luna::Value::String(luna::String::from_slice(&ctx, b"self")),
        );
        assert!(
            matches!(inner, Ok(luna::Value::Table(t)) if t == crossed),
            "the cycle came out as a copy rather than as itself"
        );
    });
}

/// Two references to one table stay one table on the far side.
///
/// `oslo.from_json == oslo.json.decode` is the shipped case: they are the same `Rc`, and crossing
/// each occurrence separately makes two callbacks that compare unequal.
#[test]
fn a_shared_table_crosses_once() {
    let shared = Rc::new(RefCell::new(Table::new()));
    shared.borrow_mut().set_str("id", Value::str("only one"));
    let mut holder = Table::new();
    holder.set_str("left", Value::Table(Rc::clone(&shared)));
    holder.set_str("right", Value::Table(shared));
    let original = Value::Table(Rc::new(RefCell::new(holder)));

    let mut lua = Lua::core();
    lua.enter(|ctx| {
        let luna::Value::Table(crossed) = into_lua(ctx, &original) else {
            panic!("the holder did not cross as a table");
        };
        let at = |key: &[u8]| {
            crossed.get(
                ctx,
                luna::Value::String(luna::String::from_slice(&ctx, key)),
            )
        };
        let (left, right) = (at(b"left"), at(b"right"));
        assert!(
            matches!(
                (left, right),
                (Ok(luna::Value::Table(l)), Ok(luna::Value::Table(r))) if l == r
            ),
            "one table crossed as two"
        );
    });
}
