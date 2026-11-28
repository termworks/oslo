//! Which operands of a declaration builtin were written as compound assignments.
//!
//! `declare a=(1 2)` makes an array, but `declare 'a=(1 2)'` and `v='a=(1 2)'; declare "$v"` make
//! the scalar `(1 2)` unless `a` is an array already, as in bash 5.3. The builtin receives the
//! same text either way, so the evaluator records which of the words it expanded were written
//! with an unquoted `=(`, and the builtin takes that record when it starts.

use std::cell::RefCell;

thread_local! {
    /// One flag per word of the declaration builtin about to run, its name included.
    static WRITTEN: RefCell<Option<Vec<bool>>> = const { RefCell::new(None) };
}

/// What a builtin took: one flag per word of its arguments, or nothing.
pub(crate) struct Written(Option<Vec<bool>>);

impl Written {
    /// Take the record for the builtin now starting, so no later call can see it.
    pub(crate) fn take() -> Self {
        Self(WRITTEN.with(|written| written.borrow_mut().take()))
    }

    /// Whether the operand `nth` of `operands`, the tail of `args`, was written `name=(…)`.
    ///
    /// With no record — the builtin reached some other way than as a command's first word —
    /// every operand counts as written, which is what it was before this distinction existed.
    pub(crate) fn operand(&self, args: &[String], operands: &[String], nth: usize) -> bool {
        let at = args.len() - operands.len() + nth;
        match &self.0 {
            Some(shapes) => shapes.get(at).copied().unwrap_or(true),
            None => true,
        }
    }
}

/// Record which words of the declaration builtin about to run were written `name=(…)`.
pub(crate) fn note_written(shapes: Vec<bool>) {
    WRITTEN.with(|written| *written.borrow_mut() = Some(shapes));
}

/// Drop a record no builtin took, when a function of that name runs instead.
pub(crate) fn forget_written() {
    WRITTEN.with(|written| written.borrow_mut().take());
}
