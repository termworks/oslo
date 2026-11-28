//! Which builtins take *assignments* rather than words, and how one is recognised.
//!
//! POSIX calls these declaration utilities. The difference is not cosmetic: their `name=value`
//! operands are expanded as assignments, so the value is neither field-split nor pathname-expanded.
//! `local x=$(echo a b)` binds `a b`; expanding it like an ordinary word bound `a` and passed `b`
//! as a separate argument, silently.
//!
//! That silence is why this has its own file. The bug it fixes was found in
//! `local command=${2/#"$widget"/__atuin_history --keymap-mode=emacs}`, where losing everything
//! after the first space left a shell integration computing keybindings that were all truncated to
//! their first word — with no diagnostic anywhere.

use oslo_base::ast::{Word, WordPart};

/// Builtins whose `name=value` arguments are assignments rather than ordinary words.
///
/// POSIX calls these declaration utilities, and the difference is not cosmetic: their operands
/// are expanded as assignments, which means no field splitting and no pathname expansion. A value
/// with a space in it survives, and one with a `*` is not replaced by the directory listing.
pub(super) fn is_declaration_builtin(name: &str) -> bool {
    matches!(
        name,
        "export" | "local" | "readonly" | "declare" | "typeset"
    )
}

/// Whether a word is shaped like `name=…` or `name[subscript]=…`.
///
/// Decided on the word's *literal* text, before expansion, exactly as the parser would: a word
/// whose `=` only appears after a variable is expanded is not an assignment. `local $x` where `x`
/// holds `a=b` declares whatever `$x` splits into, and must go on doing so.
pub(super) fn looks_like_an_assignment(word: &Word) -> bool {
    let Some(WordPart::Literal(text)) = word.parts.first() else {
        return false;
    };
    let Some(equals) = text.find('=') else {
        return false;
    };
    let name = &text[..equals];
    let name = name.strip_suffix('+').unwrap_or(name);
    // `a[0]=` is an assignment to an element; the subscript is arbitrary text here because it is
    // arithmetic that has not been evaluated yet.
    let name = match name.find('[') {
        Some(open) if name.ends_with(']') => &name[..open],
        Some(_) => return false,
        None => name,
    };
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether an assignment-shaped word was written `name=(…)`, its parenthesis unquoted in the
/// word's literal text: what bash's parser marks a compound assignment.
pub(super) fn written_as_compound(word: &Word) -> bool {
    let Some(WordPart::Literal(text)) = word.parts.first() else {
        return false;
    };
    text.split_once('=')
        .is_some_and(|(_, rest)| rest.starts_with('('))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_declaration_builtins_are_the_posix_ones() {
        for name in ["export", "local", "readonly", "declare", "typeset"] {
            assert!(is_declaration_builtin(name), "{name}");
        }
        for name in ["echo", "printf", "set", "", "locale"] {
            assert!(!is_declaration_builtin(name), "{name}");
        }
    }

    fn word(text: &str) -> Word {
        Word {
            parts: vec![WordPart::Literal(text.to_string())],
        }
    }

    #[test]
    fn an_assignment_is_a_name_an_optional_subscript_and_an_equals() {
        for text in [
            "x=", "x=1", "_x=1", "x1=1", "x+=1", "a[0]=1", "a[i+1]=1", "a[0]+=1",
        ] {
            assert!(looks_like_an_assignment(&word(text)), "{text}");
        }
    }

    #[test]
    fn anything_else_is_an_ordinary_word() {
        // No `=` at all, a name that cannot be one, and the flags these builtins take.
        for text in ["x", "-r", "1x=1", "=1", "x y=1", "a[0=1"] {
            assert!(!looks_like_an_assignment(&word(text)), "{text}");
        }
    }

    /// Decided on the *literal* text, before expansion. `local $spec` where `spec` holds `a=b`
    /// declares whatever `$spec` splits into, and must go on doing so — so a word that only
    /// becomes an assignment after expansion is not one here.
    #[test]
    fn a_word_that_is_only_an_assignment_after_expansion_is_not_one() {
        let expanded = Word {
            parts: vec![WordPart::Variable {
                name: "spec".to_string(),
                expansion_type: oslo_base::ast::ParamExpansion::Normal,
            }],
        };
        assert!(!looks_like_an_assignment(&expanded));
    }
}
