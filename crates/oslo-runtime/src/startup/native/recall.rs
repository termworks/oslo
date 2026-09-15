//! Walking the Up/Down history, and the keys that jump in it: `alt-.`, `alt-<` and `alt->`.
//!
//! `back` is how many steps into history the walk is — `0` is the line being composed, which
//! `composing` holds while the walk is away from it. `history` is oldest first.

/// Down: one step towards the newest, and out to the composed line past it.
pub(super) fn next(
    history: &[String],
    back: &mut usize,
    composing: &mut Option<String>,
) -> Option<String> {
    match *back {
        0 => None,
        // Out the far end of the walk: the line being composed comes back.
        1 => {
            *back = 0;
            composing.take()
        }
        _ => {
            *back -= 1;
            history.iter().rev().nth(*back - 1).cloned()
        }
    }
}

/// The line `n` commands ago — `1` is the previous one — without moving the walk.
pub(super) fn line(history: &[String], n: usize) -> Option<String> {
    history.iter().rev().nth(n.checked_sub(1)?).cloned()
}

/// The oldest entry, with the walk moved there so Down goes on towards the newest.
pub(super) fn oldest(
    history: &[String],
    back: &mut usize,
    composing: &mut Option<String>,
    typed: &str,
) -> Option<String> {
    let oldest = history.first()?.clone();
    if *back == 0 {
        *composing = Some(typed.to_string());
    }
    *back = history.len();
    Some(oldest)
}

/// Out of the walk, back to the line being composed.
pub(super) fn newest(back: &mut usize, composing: &mut Option<String>) -> Option<String> {
    if *back == 0 {
        return None;
    }
    *back = 0;
    composing.take()
}
