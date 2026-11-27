//! Tokeniser for arithmetic expressions.
//!
//! Splitting the scan from the grammar is what makes the full operator ladder tractable: the
//! previous character-at-a-time parser could not tell `<` from `<<` from `<<=` without
//! backtracking, so it gave up and silently dropped everything it did not recognise.

use oslo_base::error::{Result, ShellError};

/// One lexical unit of an arithmetic expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Number(i64),
    Ident(String),
    /// `[` and `]`, which only ever appear around an array subscript.
    LBracket,
    RBracket,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Pow,

    Not,
    Tilde,

    Lt,
    Le,
    Gt,
    Ge,
    EqEq,
    Ne,

    Shl,
    Shr,

    Amp,
    Pipe,
    Caret,

    AndAnd,
    OrOr,

    Question,
    Colon,
    Comma,

    LParen,
    RParen,

    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
    ShlAssign,
    ShrAssign,
    AndAssign,
    OrAssign,
    XorAssign,

    Inc,
    Dec,
}

impl Token {
    /// The compound-assignment operators, mapped to the arithmetic they perform.
    ///
    /// Returns `None` for every token that is not an assignment, and `Some(None)` for plain `=`.
    pub fn assign_op(&self) -> Option<Option<CompoundOp>> {
        Some(match self {
            Token::Assign => None,
            Token::AddAssign => Some(CompoundOp::Add),
            Token::SubAssign => Some(CompoundOp::Sub),
            Token::MulAssign => Some(CompoundOp::Mul),
            Token::DivAssign => Some(CompoundOp::Div),
            Token::ModAssign => Some(CompoundOp::Rem),
            Token::ShlAssign => Some(CompoundOp::Shl),
            Token::ShrAssign => Some(CompoundOp::Shr),
            Token::AndAssign => Some(CompoundOp::BitAnd),
            Token::OrAssign => Some(CompoundOp::BitOr),
            Token::XorAssign => Some(CompoundOp::BitXor),
            _ => return None,
        })
    }
}

/// The arithmetic half of a compound assignment such as `x <<= 2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
}

fn err(msg: String) -> ShellError {
    ShellError::ExpansionError(msg)
}

/// Scan `expr` into tokens. Anything the arithmetic grammar has no token for — `$`, `"`, a stray
/// backslash — is an error here rather than a silently truncated expression.
pub fn tokenize(expr: &str) -> Result<Vec<Token>> {
    let src: Vec<char> = expr.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;

    while i < src.len() {
        let c = src[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() {
            let (tok, next) = lex_number(&src, i)?;
            out.push(tok);
            i = next;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < src.len() && (src[i].is_alphanumeric() || src[i] == '_') {
                i += 1;
            }
            out.push(Token::Ident(src[start..i].iter().collect()));
            continue;
        }
        let (tok, len) = lex_operator(&src, i)?;
        if matches!(tok, Token::Inc | Token::Dec) && !steps_a_variable(&out, &src[i + 2..]) {
            // Not an increment: `1 +++ 2` is `1 + + + 2`, read one sign at a time.
            out.push(if tok == Token::Inc {
                Token::Plus
            } else {
                Token::Minus
            });
            i += 1;
            continue;
        }
        out.push(tok);
        i += len;
    }

    Ok(out)
}

/// Whether a `++` or `--` with `before` lexed and `after` still to come steps a variable: it
/// follows a name or a subscript, or a name follows it.
fn steps_a_variable(before: &[Token], after: &[char]) -> bool {
    if matches!(before.last(), Some(Token::Ident(_) | Token::RBracket)) {
        return true;
    }
    after
        .iter()
        .find(|c| !c.is_whitespace())
        .is_some_and(|c| c.is_alphabetic() || *c == '_')
}

/// Longest-match operator scan. Three-character forms first, then two, then one — the order is
/// load-bearing: matching `<` before `<<=` is exactly the bug that made `$((1<<4))` yield 1.
fn lex_operator(src: &[char], i: usize) -> Result<(Token, usize)> {
    let at = |n: usize| src.get(i + n).copied().unwrap_or('\0');
    let (a, b, c) = (at(0), at(1), at(2));

    let three = match (a, b, c) {
        ('<', '<', '=') => Some(Token::ShlAssign),
        ('>', '>', '=') => Some(Token::ShrAssign),
        _ => None,
    };
    if let Some(tok) = three {
        return Ok((tok, 3));
    }

    let two = match (a, b) {
        ('*', '*') => Some(Token::Pow),
        ('<', '<') => Some(Token::Shl),
        ('>', '>') => Some(Token::Shr),
        ('<', '=') => Some(Token::Le),
        ('>', '=') => Some(Token::Ge),
        ('=', '=') => Some(Token::EqEq),
        ('!', '=') => Some(Token::Ne),
        ('&', '&') => Some(Token::AndAnd),
        ('|', '|') => Some(Token::OrOr),
        ('+', '+') => Some(Token::Inc),
        ('-', '-') => Some(Token::Dec),
        ('+', '=') => Some(Token::AddAssign),
        ('-', '=') => Some(Token::SubAssign),
        ('*', '=') => Some(Token::MulAssign),
        ('/', '=') => Some(Token::DivAssign),
        ('%', '=') => Some(Token::ModAssign),
        ('&', '=') => Some(Token::AndAssign),
        ('|', '=') => Some(Token::OrAssign),
        ('^', '=') => Some(Token::XorAssign),
        _ => None,
    };
    if let Some(tok) = two {
        return Ok((tok, 2));
    }

    let one = match a {
        '+' => Token::Plus,
        '-' => Token::Minus,
        '*' => Token::Star,
        '/' => Token::Slash,
        '%' => Token::Percent,
        '!' => Token::Not,
        '~' => Token::Tilde,
        '<' => Token::Lt,
        '>' => Token::Gt,
        '&' => Token::Amp,
        '|' => Token::Pipe,
        '^' => Token::Caret,
        '?' => Token::Question,
        ':' => Token::Colon,
        ',' => Token::Comma,
        '(' => Token::LParen,
        ')' => Token::RParen,
        '=' => Token::Assign,
        '[' => Token::LBracket,
        ']' => Token::RBracket,
        _ => {
            return Err(err(format!(
                "Invalid character in arithmetic expression: {a}"
            )));
        }
    };
    Ok((one, 1))
}

/// Value of `c` as a digit, or `None` if it is not a digit character at all.
///
/// Bases up to 36 fold case together (`16#FF` and `16#ff` are both 255); past 36 the alphabet
/// needs the upper case letters for 36-61, so the two cases separate. `@` and `_` finish base 64.
fn digit_value(c: char, base: u32) -> Option<u32> {
    Some(match c {
        '0'..='9' => c as u32 - '0' as u32,
        'a'..='z' => c as u32 - 'a' as u32 + 10,
        'A'..='Z' if base <= 36 => c as u32 - 'A' as u32 + 10,
        'A'..='Z' => c as u32 - 'A' as u32 + 36,
        '@' => 62,
        '_' => 63,
        _ => return None,
    })
}

/// Scan one numeric literal: `0x`/`0X` hex, leading-zero octal, `base#digits`, or decimal.
///
/// Accumulation is `u64` and wrapping so that `9223372036854775808` reaches the parser as
/// `i64::MIN` the way C's `intmax_t` arithmetic does, instead of being rounded down to 0. A digit
/// the base cannot represent is a hard error, because `$((08))` meaning 8 would quietly corrupt
/// every `chmod`-style computation.
fn lex_number(src: &[char], start: usize) -> Result<(Token, usize)> {
    if src[start] == '0' && matches!(src.get(start + 1), Some('x' | 'X')) {
        let mut i = start + 2;
        let mut value: u64 = 0;
        while i < src.len() {
            match src[i].to_digit(16) {
                Some(d) => {
                    value = value.wrapping_mul(16).wrapping_add(u64::from(d));
                    i += 1;
                }
                None => break,
            }
        }
        return Ok((Token::Number(value as i64), i));
    }

    let mut end = start;
    while end < src.len() && src[end].is_ascii_digit() {
        end += 1;
    }
    let text: String = src[start..end].iter().collect();

    if src.get(end) == Some(&'#') {
        return lex_based(src, start, end, &text);
    }

    // A leading zero means octal, exactly as in C — and `$((010))` is 8, not 10.
    let base = if text.len() > 1 && text.starts_with('0') {
        8
    } else {
        10
    };
    let mut value: u64 = 0;
    for c in text.chars() {
        let d = digit_value(c, base).filter(|d| *d < base).ok_or_else(|| {
            err(format!(
                "value too great for base (error token is \"{text}\")"
            ))
        })?;
        value = value
            .wrapping_mul(u64::from(base))
            .wrapping_add(u64::from(d));
    }
    Ok((Token::Number(value as i64), end))
}

/// The `base#digits` form. `hash` indexes the `#`; `text` is the base written before it.
fn lex_based(src: &[char], start: usize, hash: usize, text: &str) -> Result<(Token, usize)> {
    // `parse` rather than the digit loop above: a base is a small decimal number, and one wide
    // enough to overflow `u32` is simply an invalid base.
    let base: u32 = text.parse().unwrap_or(0);
    if !(2..=64).contains(&base) {
        let token: String = src[start..=hash].iter().collect();
        return Err(err(format!(
            "invalid arithmetic base (error token is \"{token}\")"
        )));
    }

    let mut i = hash + 1;
    let mut value: u64 = 0;
    let mut digits = 0usize;
    while i < src.len() {
        let Some(d) = digit_value(src[i], base) else {
            break;
        };
        if d >= base {
            let token: String = src[start..=i].iter().collect();
            return Err(err(format!(
                "value too great for base (error token is \"{token}\")"
            )));
        }
        value = value
            .wrapping_mul(u64::from(base))
            .wrapping_add(u64::from(d));
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        let token: String = src[start..=hash].iter().collect();
        return Err(err(format!(
            "invalid integer constant (error token is \"{token}\")"
        )));
    }
    Ok((Token::Number(value as i64), i))
}

/// Read a whole string as a single signed numeric literal, or `None` if it is anything else.
///
/// This is how a variable's stored text becomes an operand. It goes through the same scanner as
/// the expression itself so that `x=0x10` and `x=010` mean what they mean everywhere else.
pub fn literal_value(text: &str) -> Option<i64> {
    let toks = tokenize(text).ok()?;
    match toks.as_slice() {
        [Token::Number(n)] => Some(*n),
        [Token::Plus, Token::Number(n)] => Some(*n),
        [Token::Minus, Token::Number(n)] => Some(n.wrapping_neg()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(expr: &str) -> i64 {
        match tokenize(expr).expect("literal should scan").as_slice() {
            [Token::Number(n)] => *n,
            other => panic!("{expr}: expected one number, got {other:?}"),
        }
    }

    #[test]
    fn decimal_literals() {
        assert_eq!(num("0"), 0);
        assert_eq!(num("42"), 42);
        assert_eq!(num("9223372036854775807"), i64::MAX);
    }

    /// The chmod case: a leading zero has always meant octal everywhere but here.
    #[test]
    fn leading_zero_is_octal() {
        assert_eq!(num("010"), 8);
        assert_eq!(num("0755"), 0o755);
        assert_eq!(num("00"), 0);
    }

    #[test]
    fn hex_literals_accept_both_prefix_cases() {
        assert_eq!(num("0x1f"), 31);
        assert_eq!(num("0X1F"), 31);
        assert_eq!(num("0xdeadbeef"), 0xdead_beef);
        // bash reads a bare `0x` as zero rather than complaining.
        assert_eq!(num("0x"), 0);
    }

    #[test]
    fn explicit_bases_two_through_sixty_four() {
        assert_eq!(num("2#101"), 5);
        assert_eq!(num("8#777"), 511);
        assert_eq!(num("16#ff"), 255);
        assert_eq!(num("16#FF"), 255);
        assert_eq!(num("36#z"), 35);
        assert_eq!(num("64#@"), 62);
        assert_eq!(num("64#_"), 63);
        // Past base 36 the cases split: lower case stays 10-35, upper case starts at 36.
        assert_eq!(num("37#z"), 35);
        assert_eq!(num("62#Z"), 61);
    }

    #[test]
    fn out_of_range_digit_is_an_error_not_a_wrong_number() {
        for expr in ["08", "09", "2#3", "0999", "8#8"] {
            let e = tokenize(expr).expect_err(expr);
            assert!(e.to_string().contains("value too great for base"), "{e}");
        }
    }

    #[test]
    fn invalid_bases_are_errors() {
        for expr in ["1#0", "0#1", "65#1", "100#1"] {
            let e = tokenize(expr).expect_err(expr);
            assert!(e.to_string().contains("invalid arithmetic base"), "{e}");
        }
        let e = tokenize("2#").expect_err("empty digits");
        assert!(e.to_string().contains("invalid integer constant"), "{e}");
    }

    /// Wide literals wrap like C `intmax_t`; they must never collapse to 0.
    #[test]
    fn oversized_literals_wrap() {
        assert_eq!(num("9223372036854775808"), i64::MIN);
        assert_eq!(num("0xffffffffffffffffff"), -1);
    }

    #[test]
    fn longest_match_wins_over_prefixes() {
        assert_eq!(tokenize("<<=").unwrap(), vec![Token::ShlAssign]);
        assert_eq!(tokenize("<<").unwrap(), vec![Token::Shl]);
        assert_eq!(tokenize("<=").unwrap(), vec![Token::Le]);
        assert_eq!(tokenize("<").unwrap(), vec![Token::Lt]);
        assert_eq!(tokenize("**").unwrap(), vec![Token::Pow]);
        assert_eq!(
            tokenize("a++ + ++b").unwrap(),
            vec![
                Token::Ident("a".into()),
                Token::Inc,
                Token::Plus,
                Token::Inc,
                Token::Ident("b".into()),
            ]
        );
    }

    #[test]
    fn unknown_characters_are_rejected() {
        assert!(tokenize("$x").is_err());
        assert!(tokenize("\"1\"").is_err());
        assert!(tokenize("1 @ 2").is_err());
    }

    #[test]
    fn literal_value_reads_signed_operands() {
        assert_eq!(literal_value(" 12 "), Some(12));
        assert_eq!(literal_value("-5"), Some(-5));
        assert_eq!(literal_value("+5"), Some(5));
        assert_eq!(literal_value("0x10"), Some(16));
        assert_eq!(literal_value("-9223372036854775808"), Some(i64::MIN));
        assert_eq!(literal_value("1+1"), None);
        assert_eq!(literal_value("abc"), None);
        assert_eq!(literal_value(""), None);
    }
}
