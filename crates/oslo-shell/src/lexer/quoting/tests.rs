use super::*;
use crate::lexer::{Lexer, Token};
use oslo_base::ast::WordPart;

fn body(src: &str) -> Vec<WordPart> {
    parse_heredoc_body(src)
        .unwrap_or_else(|e| panic!("heredoc body {src:?} did not scan: {e}"))
        .parts
}

fn parts(src: &str) -> Vec<WordPart> {
    match Lexer::new(src).next() {
        Ok(Token::Word(w)) => w.parts,
        other => panic!("expected a word from {src:?}, got {other:?}"),
    }
}

/// **An `extglob` group is part of its word**, spaces and `|` included, and quotes and `$` inside
/// it keep their meaning. rune reads it that way; re-lexing the word must not undo that.
#[test]
fn an_extglob_group_keeps_its_word_together() {
    let got = parts("@(\"a b\"|$v)");
    assert!(
        matches!(
            got.as_slice(),
            [WordPart::Literal(open), WordPart::DoubleQuoted(_), WordPart::Literal(bar), _, WordPart::Literal(close)]
                if open == "@(" && bar == "|" && close == ")"
        ),
        "{got:?}"
    );
    let mut lexer = Lexer::new("+(a b) next");
    assert!(matches!(lexer.next(), Ok(Token::Word(_))));
    assert!(
        matches!(lexer.next(), Ok(Token::Word(w)) if w.parts == [WordPart::Literal("next".into())]),
        "the space after the group still ends the word"
    );
}

/// **A comment inside a `$( … )` inside a heredoc body is not shell.**
///
/// A lone apostrophe in one opened a quote that ran to the end of the file, so the `)` closing
/// the substitution was never seen and the whole script failed to parse. Found by syntax
/// checking every `#!/bin/sh` script on a Debian system against dash: 287 of 288 agreed, and
/// the one that did not was `/usr/bin/xdg-terminal-exec`, which carries
/// `# Don't complain about nonexistent directories` inside exactly this construct.
#[test]
fn a_comment_in_a_substitution_in_a_heredoc_is_not_shell() {
    // The apostrophe must not open anything, and the `)` must still close the substitution.
    let parts = body("$(\n# Don't\necho hi\n)\n");
    assert!(
        matches!(parts.first(), Some(WordPart::CommandSubstitution(_))),
        "the substitution did not close: {parts:?}"
    );
    // An unbalanced paren or quote in the comment is just as harmless.
    for comment in ["# a stray ( here", "# a stray \" here", "# it's fine"] {
        let src = format!("$(\n{comment}\necho hi\n)\n");
        assert!(
            parse_heredoc_body(&src).is_ok(),
            "{comment:?} broke the scan"
        );
    }
}

/// A `#` inside a word is an ordinary character, so the comment rule must not eat one.
#[test]
fn a_hash_inside_a_word_is_not_a_comment() {
    for src in ["$(echo ab#cd)\n", "$(v=x; echo ${v#x}y)\n", "$(echo a#b)\n"] {
        let parts = parse_heredoc_body(src)
            .unwrap_or_else(|e| panic!("{src:?} did not scan: {e}"))
            .parts;
        assert!(
            matches!(parts.first(), Some(WordPart::CommandSubstitution(_))),
            "{src:?} lost its substitution: {parts:?}"
        );
    }
}

/// **`${#name}` is a length, not a comment**, and its `#` is the very first character inside
/// the braces — where a "comment starts at a word boundary" rule is most tempted to fire.
///
/// A parameter expansion holds no command list, so it can hold no comment; only `$( … )` can.
/// Getting this wrong ate the whole expansion and printed `"len=[${#n}]"` back verbatim, which
/// is worth a test of its own because it breaks a construct every script uses.
#[test]
fn a_length_expansion_is_not_a_comment() {
    for src in ["${#n}\n", "[${#name}]\n", "${#1}\n", "${#@}\n"] {
        let parts = parse_heredoc_body(src)
            .unwrap_or_else(|e| panic!("{src:?} did not scan: {e}"))
            .parts;
        assert!(
            parts.iter().any(|p| matches!(p, WordPart::Variable { .. })),
            "{src:?} lost its expansion: {parts:?}"
        );
    }
}

/// The whole point of the variant: an escaped character has to be distinguishable from text
/// the user typed bare, or `echo \*` globs.
#[test]
fn a_backslash_escape_is_not_a_literal() {
    assert_eq!(parts("\\*"), vec![WordPart::Escaped("*".into())]);
    assert_eq!(
        parts("a\\ b"),
        vec![
            WordPart::Literal("a".into()),
            WordPart::Escaped(" ".into()),
            WordPart::Literal("b".into()),
        ]
    );
}

#[test]
fn adjacent_escapes_stay_separate_parts() {
    assert_eq!(
        parts("\\*\\?"),
        vec![WordPart::Escaped("*".into()), WordPart::Escaped("?".into()),]
    );
}

/// `\if` is a word, not the reserved word `if`: the keyword check only fires on a lone literal.
#[test]
fn an_escaped_keyword_is_not_a_keyword() {
    assert_eq!(
        parts("\\if"),
        vec![WordPart::Escaped("i".into()), WordPart::Literal("f".into()),]
    );
}

/// A backslash inside double quotes is the double-quote rule's job; the run is quoted either
/// way, so it stays a plain literal.
#[test]
fn double_quoted_escapes_are_still_literals() {
    assert_eq!(
        parts("\"a\\$b\""),
        vec![WordPart::DoubleQuoted(vec![WordPart::Literal(
            "a$b".into()
        )])]
    );
}

/// A digit run before `<` or `>` is still an fd number; a backslash anywhere rules that out.
#[test]
fn escaping_defeats_the_io_number_rule() {
    assert!(matches!(Lexer::new("2>").next(), Ok(Token::IoNumber(2))));
    assert!(matches!(Lexer::new("\\2>").next(), Ok(Token::Word(_))));
    assert!(matches!(Lexer::new("\"2\">").next(), Ok(Token::Word(_))));
}

// --- R2.3: backquote command substitution ---

#[test]
fn a_backquote_run_is_a_command_substitution() {
    assert_eq!(
        parts("`echo hi`"),
        vec![WordPart::CommandSubstitution("echo hi".into())]
    );
}

/// Backquotes are live inside double quotes, and adjacent to other parts of the same word.
#[test]
fn backquotes_are_scanned_in_every_live_context() {
    assert_eq!(
        parts("\"v: `echo q`\""),
        vec![WordPart::DoubleQuoted(vec![
            WordPart::Literal("v: ".into()),
            WordPart::CommandSubstitution("echo q".into()),
        ])]
    );
    assert_eq!(
        parts("a`f`b"),
        vec![
            WordPart::Literal("a".into()),
            WordPart::CommandSubstitution("f".into()),
            WordPart::Literal("b".into()),
        ]
    );
}

/// Inside backquotes exactly three escapes exist; every other backslash is data the inner
/// parse still has to see, so `\n` must survive as two characters.
#[test]
fn backquote_escapes_are_stripped_only_for_the_three_special_characters() {
    assert_eq!(
        parts("`echo \\`x\\` \\$v \\\\ \\n`"),
        vec![WordPart::CommandSubstitution("echo `x` $v \\ \\n".into())]
    );
}

/// A single quote is not a quote inside backquotes as far as *this* scan is concerned, but an
/// unterminated backquote is still an error rather than silent literal text.
#[test]
fn an_unterminated_backquote_is_a_syntax_error() {
    assert!(Lexer::new("`echo hi").next().is_err());
}

// --- R2.7: quote-aware `$( )` scanning ---

#[test]
fn a_quoted_paren_does_not_end_a_substitution() {
    assert_eq!(
        parts("\"$(echo \"a)b\")\""),
        vec![WordPart::DoubleQuoted(vec![WordPart::CommandSubstitution(
            "echo \"a)b\"".into()
        )])]
    );
    assert_eq!(
        parts("$(echo '(paren)')"),
        vec![WordPart::CommandSubstitution("echo '(paren)'".into())]
    );
}

/// `$((` is not always arithmetic: it is also a substitution whose first command is a
/// subshell. What follows the balanced group decides which.
#[test]
fn a_leading_subshell_is_not_arithmetic() {
    assert_eq!(parts("$((1+2))"), vec![WordPart::Arithmetic("1+2".into())]);
    assert_eq!(
        parts("$(((1+2)*3))"),
        vec![WordPart::Arithmetic("(1+2)*3".into())]
    );
    assert_eq!(
        parts("$((echo a); echo b)"),
        vec![WordPart::CommandSubstitution("(echo a); echo b".into())]
    );
}

#[test]
fn an_unterminated_substitution_is_a_syntax_error() {
    assert!(Lexer::new("$(echo hi").next().is_err());
    assert!(Lexer::new("${x:-y").next().is_err());
}

/// Round 11 A1. `skip_whitespace` stepped over these and `scan_word_parts` refused to consume
/// them, so the lexer returned part-less words forever and every caller looping to `Eof` hung
/// while allocating. They are ordinary word characters in bash, and now here.
#[test]
fn unicode_blanks_are_word_characters_not_separators() {
    for blank in [
        '\u{0b}', '\u{0c}', '\u{a0}', '\u{2028}', '\u{2007}', '\u{85}',
    ] {
        let src = format!("a{blank}b");
        assert_eq!(
            parts(&src),
            vec![WordPart::Literal(src.clone())],
            "U+{:04X} split a word",
            blank as u32
        );
    }
}

// --- R11.B2: here-document bodies ---

/// The reason a body cannot go through the word scanner. All three of these end a word, or
/// open a quote that never closes, and in a heredoc all three are ordinary characters.
#[test]
fn quotes_blanks_and_tildes_are_plain_text_in_a_body() {
    assert_eq!(
        body("it's \"fine\"\t~root *\n"),
        vec![WordPart::Literal("it's \"fine\"\t~root *\n".into())]
    );
    assert!(
        super::parse_word_source("it's fine").is_err(),
        "the word scanner accepts an unterminated quote, so this test proves nothing"
    );
}

/// The live set is the double-quoted one, so expansions still become parts.
#[test]
fn expansions_in_a_body_are_still_scanned() {
    assert_eq!(
        body("x=$v\n"),
        vec![
            WordPart::Literal("x=".into()),
            WordPart::Variable {
                name: "v".into(),
                expansion_type: oslo_base::ast::ParamExpansion::Normal,
            },
            WordPart::Literal("\n".into()),
        ]
    );
    assert_eq!(
        body("$(echo a)`echo b`$((1+1))"),
        vec![
            WordPart::CommandSubstitution("echo a".into()),
            WordPart::CommandSubstitution("echo b".into()),
            WordPart::Arithmetic("1+1".into()),
        ]
    );
}

/// A backslash escapes `$`, a backtick, another backslash and a newline — and nothing else.
/// `\"` in particular is two characters here, where inside double quotes it is one.
#[test]
fn only_four_escapes_exist_in_a_body() {
    assert_eq!(body("\\$v"), vec![WordPart::Literal("$v".into())]);
    assert_eq!(body("\\`x"), vec![WordPart::Literal("`x".into())]);
    assert_eq!(body("\\\\"), vec![WordPart::Literal("\\".into())]);
    assert_eq!(body("a\\\nb"), vec![WordPart::Literal("ab".into())]);
    assert_eq!(
        body("\\\"q\\\""),
        vec![WordPart::Literal("\\\"q\\\"".into())]
    );
    assert_eq!(body("\\n\\t"), vec![WordPart::Literal("\\n\\t".into())]);
}

/// A body ends where its delimiter line was, which the caller has already cut off. Running
/// out of input is therefore the normal ending, not the error it is inside quotes.
#[test]
fn a_body_ends_at_end_of_input_rather_than_erroring() {
    assert_eq!(body(""), vec![]);
    assert_eq!(
        body("no trailing newline"),
        vec![WordPart::Literal("no trailing newline".into())]
    );
    assert!(
        Lexer::new("unterminated").scan_double_quote().is_err(),
        "the shared scanner must still refuse an unterminated double quote"
    );
}

/// The property that makes the hang impossible rather than merely fixed: whatever the input,
/// `next()` either consumes something or reaches `Eof`/an error.
#[test]
fn every_token_consumes_at_least_one_character() {
    for src in [
        "a\u{0b}b",
        "echo a\u{a0}b",
        "\u{2028}",
        "\u{a0}",
        "\u{0c}\u{0b}\u{85}",
        "a b\tc\rd\ne",
        "''",
        "\"\"",
        "x=1\u{0b}2",
        "~\u{a0}",
    ] {
        let mut lexer = Lexer::new(src);
        // One token per character is the ceiling; anything past it is a scanner that stopped
        // advancing, which is the bug this bounds rather than a test of tokenization.
        for _ in 0..=src.chars().count() {
            let before = lexer.offset();
            match lexer.next() {
                Ok(Token::Eof) | Err(_) => break,
                Ok(tok) => assert!(
                    lexer.offset() > before,
                    "{src:?}: {tok:?} consumed nothing at offset {before}"
                ),
            }
        }
    }
}

/// **`\` before a newline is a line continuation: both characters go.**
///
/// Every other character is escaped *to itself* and kept as a [`WordPart::Escaped`]. Doing that to
/// a newline spliced a real line break into the middle of the word, so
/// `P=/usr/bin:\` + newline + `/usr/local/bin` produced a `$P` containing one — where bash and dash
/// join the halves. POSIX 2.2.1 names the newline as the single exception to the escape rule.
#[test]
fn a_backslash_before_a_newline_joins_the_word() {
    assert_eq!(
        parts("a\\\nb"),
        vec![WordPart::Literal("ab".into())],
        "the continuation left something behind"
    );
    // The halves of a split path become one word with nothing between them.
    assert_eq!(
        parts("/usr/bin:\\\n/usr/local/bin"),
        vec![WordPart::Literal("/usr/bin:/usr/local/bin".into())]
    );
    // And nothing else changes: a backslash before any other character still escapes it, which is
    // what keeps `echo \*` from globbing.
    assert_eq!(
        parts("\\*"),
        vec![WordPart::Escaped("*".into())],
        "an ordinary escape was swallowed with the newline case"
    );
}
