//! Covered end to end in `lib/tests.rs`; this file is where cases for this module alone go.

use super::{MAX_DEPTH, parse};
use crate::lex::lex;

/// **An expression a person typed must not be able to abort the shell.**
///
/// [`super::MAX_DEPTH`] bounds the recursion of a recursive-descent parser. Without it
/// `math "((((…1…))))"` at twenty thousand brackets overflowed the stack, and a stack overflow is
/// not an error a shell can report — the process is gone, taking the session with it. The shell's
/// own `$(( … ))` answers `maximum nesting level exceeded` at the same input and carries on.
///
/// Asserted either side of the limit, so the bound is the thing under test rather than the shape.
/// The deep case is far past it: an off-by-one would still pass at `MAX_DEPTH + 1`, and nothing
/// short of a real bound survives twenty thousand.
#[test]
fn brackets_nested_past_the_limit_are_refused_rather_than_overflowing() {
    let wrapped = |n: usize| format!("{}1{}", "(".repeat(n), ")".repeat(n));

    let ok = lex(&wrapped(MAX_DEPTH)).expect("lexes");
    assert!(
        parse(&ok).is_ok(),
        "{MAX_DEPTH} deep is still an expression"
    );

    for depth in [MAX_DEPTH + 1, 20_000] {
        let tokens = lex(&wrapped(depth)).expect("lexes");
        let problem = parse(&tokens).expect_err("{depth} deep must be refused");
        assert!(
            problem.contains("nested deeper"),
            "{depth} deep failed for another reason: {problem}"
        );
    }
}
