use super::{Table, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// **Freeing a deep table must not recurse.** See [`super::Table`]'s `Drop`.
///
/// A hundred thousand levels, built as a chain the way `c.n = {}; c = c.n` does from Lua. The
/// derived teardown freed each child from inside freeing its parent and aborted the process
/// somewhere past twenty-five thousand — and a `Drop` cannot report anything, so the session
/// simply ended. Run on a test thread, whose stack is smaller than the shell's, so a return to
/// recursion fails here before it fails anywhere else.
#[test]
fn a_deep_table_is_freed_without_recursing() {
    let root = Rc::new(RefCell::new(Table::new()));
    let mut tip = Rc::clone(&root);
    for _ in 0..100_000 {
        let next = Rc::new(RefCell::new(Table::new()));
        tip.borrow_mut()
            .set(Value::str("n"), Value::Table(Rc::clone(&next)));
        tip = next;
    }
    drop(tip);
    // The whole chain goes here. Reaching the next line at all is the assertion.
    drop(root);
}

/// A table another value still holds is not freed early, and one that holds itself does not
/// hang — the two properties the queue had to keep.
#[test]
fn sharing_and_cycles_are_unchanged() {
    let shared = Rc::new(RefCell::new(Table::new()));
    let holder = Rc::new(RefCell::new(Table::new()));
    holder
        .borrow_mut()
        .set(Value::str("a"), Value::Table(Rc::clone(&shared)));
    drop(holder);
    assert_eq!(
        Rc::strong_count(&shared),
        1,
        "the shared table went with its holder"
    );

    let looped = Rc::new(RefCell::new(Table::new()));
    looped
        .borrow_mut()
        .set(Value::str("self"), Value::Table(Rc::clone(&looped)));
    // Dropping our handle leaves the cycle holding itself, exactly as an `Rc` cycle always
    // did. Returning from this is the point: it must not spin.
    drop(looped);
}
