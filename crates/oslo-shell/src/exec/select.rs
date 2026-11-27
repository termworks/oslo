//! `select name [in words]; do … done`: bash's menu loop.
//!
//! Each turn prints `$PS3` (default `#? `) on stderr and reads one line into `REPLY` with `read`'s
//! own rules. A number naming an item sets `name` to it, anything else sets `name` empty, and the
//! body runs either way. An empty line reprints the menu without running the body; end of input
//! writes a newline to stdout and leaves the loop with status 1. The menu is printed before the
//! first prompt and again only after an empty reply, as bash does with `KSH_COMPATIBILITY_SELECT`.

use super::compound::{LoopStep, run_loop_body};
use crate::env::Environment;
use crate::expand::expand_word;
use oslo_base::ast::{CommandList, Word};
use oslo_base::error::Result;
use std::io::Write;

/// Width of `") "` between an index and its item.
const RP_SPACE_LEN: usize = 2;
/// Tab stops the menu columns are padded to.
const TAB_SIZE: usize = 8;

pub(crate) fn eval_select(
    env: &mut Environment,
    var_name: &str,
    items: Option<&[Word]>,
    body: &CommandList,
) -> Result<i32> {
    let words = match items {
        Some(items) => {
            let mut words = Vec::new();
            for word in items {
                words.extend(expand_word(env, word)?);
            }
            words
        }
        None => env.get_positional().to_vec(),
    };
    if words.is_empty() {
        return Ok(0);
    }

    let mut status = 0;
    let mut show_menu = true;
    env.enter_loop();
    let result = loop {
        let prompt = env.get_var("PS3").unwrap_or("#? ").to_string();
        let selection = match query(env, &words, &prompt, show_menu) {
            Ok(Some(selection)) => selection,
            Ok(None) => break Ok(1),
            Err(e) => break Err(e),
        };
        if !env.set_var(var_name, &selection, false) {
            break Ok(1);
        }
        match run_loop_body(env, body, &mut status) {
            LoopStep::Next => {}
            LoopStep::Stop => break Ok(status),
            LoopStep::Unwind(e) => break Err(e),
        }
        show_menu = env.get_var("REPLY").is_some_and(str::is_empty);
    };
    env.exit_loop();
    result
}

/// Prompt until a non-empty line arrives, and say which item it chose.
///
/// `None` is end of input. `Some("")` is a reply that names no item.
fn query(
    env: &mut Environment,
    words: &[String],
    prompt: &str,
    mut show_menu: bool,
) -> Result<Option<String>> {
    loop {
        let mut out = String::new();
        if show_menu {
            out = menu(words, columns(env));
        }
        out.push_str(prompt);
        let _ = std::io::stderr().write_all(out.as_bytes());

        let status = crate::env::builtins::builtin_read(env, &["read".to_string()])?;
        if status != 0 {
            let _ = std::io::stdout().flush();
            let _ = nix::unistd::write(unsafe { std::os::fd::BorrowedFd::borrow_raw(1) }, b"\n");
            return Ok(None);
        }
        let reply = env.get_var("REPLY").unwrap_or_default().to_string();
        if reply.is_empty() {
            show_menu = true;
            continue;
        }
        let chosen = legal_number(&reply)
            .filter(|n| *n >= 1 && *n as u64 <= words.len() as u64)
            .map(|n| words[n as usize - 1].clone());
        return Ok(Some(chosen.unwrap_or_default()));
    }
}

/// The width the menu is laid out in: `$COLUMNS`, else the terminal's, else 80.
fn columns(env: &Environment) -> usize {
    let set = env.get_var("COLUMNS").map(atoi).filter(|n| *n > 0);
    match set {
        Some(n) => n as usize,
        None => oslo_ui::dropdown::terminal_cols(),
    }
}

/// C's `atoi`: optional blanks and sign, then as many digits as there are.
fn atoi(text: &str) -> i64 {
    let text = text.trim_start();
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let value = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |acc, d| {
            acc.saturating_mul(10).saturating_add(i64::from(d - b'0'))
        });
    if negative { -value } else { value }
}

/// bash's `legal_number`: a base-10 integer with optional surrounding blanks and nothing else.
fn legal_number(text: &str) -> Option<i64> {
    let trimmed = text.trim_matches([' ', '\t', '\n']);
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    trimmed.parse().ok()
}

/// Decimal digits in `n`.
fn number_len(n: usize) -> usize {
    n.to_string().len()
}

/// The numbered list, column-major, in bash's layout.
fn menu(words: &[String], cols_available: usize) -> String {
    let count = words.len();
    let indices_len = number_len(count);
    let widest = words
        .iter()
        .map(|word| oslo_ui::dropdown::display_width(word))
        .max()
        .unwrap_or(0);
    let max_elem_len = widest + indices_len + RP_SPACE_LEN + 2;

    let cols = (cols_available / max_elem_len).max(1);
    let rows = count.div_ceil(cols);
    // A menu that would fit on one row is printed as one column instead.
    let rows = match rows {
        1 => count,
        rows => rows,
    };
    let first_indices_len = number_len(rows);

    let mut out = String::new();
    for row in 0..rows {
        let mut index = row;
        let mut pos = 0;
        loop {
            let width = if pos == 0 {
                first_indices_len
            } else {
                indices_len
            };
            let word = &words[index];
            out.push_str(&format!("{:>width$}) {word}", index + 1));
            let elem_len = oslo_ui::dropdown::display_width(word) + width + RP_SPACE_LEN;
            index += rows;
            if index >= count {
                break;
            }
            indent(&mut out, pos + elem_len, pos + max_elem_len);
            pos += max_elem_len;
        }
        out.push('\n');
    }
    out
}

/// Pad from column `from` to column `to` with tabs where a tab stop fits, spaces otherwise.
fn indent(out: &mut String, mut from: usize, to: usize) {
    while from < to {
        if to / TAB_SIZE > from / TAB_SIZE {
            out.push('\t');
            from += TAB_SIZE - from % TAB_SIZE;
        } else {
            out.push(' ');
            from += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Vec<String> {
        (1..=n).map(|i| i.to_string()).collect()
    }

    #[test]
    fn a_short_list_is_one_column() {
        let words: Vec<String> = ["alpha", "beta"].map(String::from).to_vec();
        assert_eq!(menu(&words, 80), "1) alpha\n2) beta\n");
    }

    #[test]
    fn a_long_list_is_laid_out_column_major_with_tabs() {
        let text = menu(&items(25), 80);
        let first = text.lines().next().unwrap();
        assert_eq!(
            first,
            "1) 1\t 4) 4\t 7) 7\t10) 10\t13) 13\t16) 16\t19) 19\t22) 22\t25) 25"
        );
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn replies_are_numbers_with_optional_blanks() {
        assert_eq!(legal_number(" 2 "), Some(2));
        assert_eq!(legal_number("+3"), Some(3));
        assert_eq!(legal_number("2x"), None);
        assert_eq!(legal_number(""), None);
    }
}
