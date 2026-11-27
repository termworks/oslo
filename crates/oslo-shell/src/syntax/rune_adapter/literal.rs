//! The elements of an array literal: `(a b)`, `([2]=a [k]+=b)`.
//!
//! rune splits an array value at blanks, but bash reads a `[` that starts an element up to its
//! matching `]` first, blanks and all, so `([a b]=1)` and `([ 1 + 1 ]=x)` are one element each.
//! The words rune gives are joined back here wherever bash would have read them as one.

use crate::lexer::TildeMode;
use crate::syntax::lower::{convert_braced_words_with_tilde, single_word_with_tilde};
use oslo_base::ast as oslo_ast;
use oslo_base::error::Result;
use rune::{Node, SyntaxKind, Tree};

/// The elements of the array value `array`.
pub(super) fn array_elements(tree: &Tree, array: &Node) -> Result<Vec<oslo_ast::ArrayElement>> {
    let source = tree.source().text();
    let spans: Vec<(usize, usize)> = array
        .nodes()
        .filter(|node| node.kind() == SyntaxKind::Word)
        .map(|node| (node.span().start as usize, node.span().end as usize))
        .collect();
    let mut elements = Vec::new();
    let mut at = 0;
    while at < spans.len() {
        let start = spans[at].0;
        let open = source[start..spans[at].1].starts_with('[')
            && subscript_end(&source[start..spans[at].1]).is_none();
        // A bracket its word leaves open runs on into the words after it, to the one closing it.
        let last = match open {
            true => (at + 1..spans.len())
                .find(|&last| subscript_end(&source[start..spans[last].1]).is_some())
                .unwrap_or(at),
            false => at,
        };
        let text = &source[start..spans[last].1];
        match last > at && keyed(text).is_none() {
            true => elements.push(oslo_ast::ArrayElement {
                index: None,
                value: crate::lexer::parse_single_word(text)?,
                append: false,
            }),
            false => elements.extend(convert_array_element(text)?),
        }
        at = last + 1;
    }
    Ok(elements)
}

/// One element of an array literal.
///
/// An unindexed element may still expand to several words — `a=($list)` and `a=(*.c)` both do — so
/// it becomes one [`oslo_ast::ArrayElement`] per word rather than being forced into one. An
/// *indexed* element (`[3]=x`) is a single value by construction, and its subscript is one word
/// however many blanks it holds.
pub(super) fn convert_array_element(text: &str) -> Result<Vec<oslo_ast::ArrayElement>> {
    if let Some((index, value, append)) = keyed(text) {
        return Ok(vec![oslo_ast::ArrayElement {
            index: Some(crate::lexer::parse_single_word(index)?),
            value: single_word_with_tilde(value, TildeMode::Value)?,
            append,
        }]);
    }
    Ok(convert_braced_words_with_tilde(text, TildeMode::Leading)?
        .into_iter()
        .map(|value| oslo_ast::ArrayElement {
            index: None,
            value,
            append: false,
        })
        .collect())
}

/// `[index]=value` or `[index]+=value` taken apart, or `None` for a bare element.
fn keyed(text: &str) -> Option<(&str, &str, bool)> {
    let close = subscript_end(text)?;
    let index = &text[1..close];
    let rest = &text[close + 1..];
    match rest.strip_prefix("+=") {
        Some(value) => Some((index, value, true)),
        None => rest.strip_prefix('=').map(|value| (index, value, false)),
    }
}

/// Where the `]` matching the `[` that `text` starts with is, reading quotes and nested brackets
/// as bash's `parse_matched_pair` does.
fn subscript_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }
    let (mut depth, mut at) = (0usize, 0);
    let mut quote: Option<u8> = None;
    while at < bytes.len() {
        match (quote, bytes[at]) {
            (Some(b'\''), b'\'') | (Some(b'"'), b'"') => quote = None,
            (Some(b'"') | None, b'\\') => at += 1,
            (Some(_), _) => {}
            (None, b @ (b'\'' | b'"')) => quote = Some(b),
            (None, b'[') => depth += 1,
            (None, b']') => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            (None, _) => {}
        }
        at += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::keyed;

    #[test]
    fn a_subscript_ends_at_its_matching_bracket() {
        assert_eq!(keyed("[a b]=1"), Some(("a b", "1", false)));
        assert_eq!(keyed("[k]+=v"), Some(("k", "v", true)));
        assert_eq!(keyed(r#"["c]d"]=x"#), Some((r#""c]d""#, "x", false)));
        assert_eq!(keyed("[a[1]]=y"), Some(("a[1]", "y", false)));
        assert_eq!(keyed("[ab]"), None);
        assert_eq!(keyed("[a"), None);
        assert_eq!(keyed("x=1"), None);
    }
}
