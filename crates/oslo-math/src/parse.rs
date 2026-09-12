//! The grammar: what an expression is, and how tightly each part binds.
//!
//! # Precedence, loosest first
//!
//! ```text
//! name = expr                     an assignment, and only at the top
//! expr in unit                    conversion, so `1 + 1 m in cm` converts the sum
//! a | b                           bitwise
//! a xor b
//! a & b
//! a << b, a >> b
//! a + b, a - b
//! a * b, a / b, a % b             the written operators
//! a b                             two things side by side, which multiply
//! -a, +a, ~a                      prefix
//! a ^ b                           power, right-associative: 2^3^2 is 2^(3^2)
//! a!, a%                          postfix
//! f(x), (a), 42, 0xff, name       atoms
//! ```
//!
//! # Three rules that are not obvious
//!
//! **`^` binds tighter than unary minus**, so `-2^2` is `-4` — which is what the notation means
//! everywhere outside a spreadsheet, and what every calculator that is not Excel does.
//!
//! **Two things side by side multiply, and bind tighter than `/`.** That second half is what makes
//! a unit readable: `100 m / 9.58 s` is a hundred metres per nine and a half seconds, and plain
//! left-to-right precedence reads it as `(100 × m ÷ 9.58) × s` — metre-seconds, which is not a
//! speed. See the `juxtaposed` rule below.
//!
//! **`in` is the inch as well as the keyword.** `3 ft + 4 in` and `5 km in miles` are both what
//! people write, and what separates them is whether anything follows the `in` to convert into.
//! See the `conversion` rule below.

use crate::lex::{Base, Token};

#[derive(Clone, Debug)]
pub enum Expr {
    Number(f64, Base),
    /// A name: resolved at evaluation into a variable, a constant, a unit or a function.
    Name(String),
    Call(String, Vec<Expr>),
    Unary(Unary, Box<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    /// `expr in unit`, where the right side is a unit expression rather than a value.
    Convert(Box<Expr>, Box<Expr>),
    /// `name = expr`.
    Assign(String, Box<Expr>),
    /// A trailing `%`.
    Percent(Box<Expr>),
    /// A trailing `!`.
    Factorial(Box<Expr>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unary {
    Negate,
    Not,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Binary {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Power,
    And,
    Or,
    Xor,
    Shl,
    Shr,
}

/// How deeply brackets may nest before the expression is refused.
///
/// **A recursive-descent parser recurses, and a stack is finite.** `math "((((…1…))))"` at twenty
/// thousand brackets overflowed the stack and aborted the process — the shell died on an
/// expression somebody typed, where `echo $(( … ))` at the same depth answers `maximum nesting
/// level exceeded` and carries on. The tree this builds is walked again by `eval`, which recurses
/// the same way, so bounding it here bounds both.
///
/// **The number is about the smallest stack this runs on, not about arithmetic.** One bracket
/// costs a frame at every level of the precedence chain above, so the cost per bracket is a dozen
/// frames rather than one: 256 was tried first and still overflowed a 2 MiB thread — which is what
/// `libtest` gives a test, and less than the 16 MiB the shell hands its interpreter. A hundred
/// matches `oslo_base::nesting::MAX_INPUT_NESTING`, the shell's own limit on the same shape, and no
/// expression anybody writes comes close. The constant is duplicated rather than shared because
/// this crate does not depend on `oslo-base`.
const MAX_DEPTH: usize = 100;

/// Parse a whole expression, which must use every token.
pub fn parse(tokens: &[Token]) -> Result<Expr, String> {
    let mut p = Parser {
        tokens,
        at: 0,
        depth: 0,
    };
    let expr = p.assignment()?;
    if p.at < p.tokens.len() {
        return Err(format!("{} is left over", p.describe_here()));
    }
    Ok(expr)
}

struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
    /// Brackets currently open, against [`MAX_DEPTH`].
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            return true;
        }
        false
    }

    /// The word at the cursor, if there is one and it matches.
    fn eat_word(&mut self, word: &str) -> bool {
        if let Some(Token::Word(found)) = self.peek()
            && found.eq_ignore_ascii_case(word)
        {
            self.at += 1;
            return true;
        }
        false
    }

    /// Run `inner` one bracket deeper, refusing past [`MAX_DEPTH`].
    ///
    /// The depth is put back on the way out, including the error path, so a expression that
    /// recovers is not charged for brackets it has already closed.
    fn nested<T>(
        &mut self,
        inner: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.depth >= MAX_DEPTH {
            return Err(format!("brackets nested deeper than {MAX_DEPTH}"));
        }
        self.depth += 1;
        let answer = inner(self);
        self.depth -= 1;
        answer
    }

    fn describe_here(&self) -> String {
        match self.peek() {
            None => "the end of the line".to_string(),
            Some(Token::Word(w)) => format!("{w:?}"),
            Some(Token::Number(n, _)) => format!("{n}"),
            Some(other) => format!("{other:?}"),
        }
    }

    fn assignment(&mut self) -> Result<Expr, String> {
        // `name = expr`, but only that shape: `2 = 3` is not an assignment and `a + b = c` is not
        // one either, so the lookahead is exactly two tokens deep.
        if let (Some(Token::Word(name)), Some(Token::Equals)) =
            (self.tokens.get(self.at), self.tokens.get(self.at + 1))
        {
            let name = name.clone();
            self.at += 2;
            return Ok(Expr::Assign(name, Box::new(self.conversion()?)));
        }
        self.conversion()
    }

    /// `expr in unit`, the loosest operator there is — `1 + 1 m in cm` converts the sum.
    ///
    /// **`in` is also the inch**, which is the one genuine ambiguity in the whole grammar. `3 ft +
    /// 4 in` ends in a unit and `5 km in miles` has a conversion in the middle, and both are what
    /// people write. What separates them is whether anything *follows*: a conversion needs a unit
    /// to convert into, so an `in` with nothing usable after it is the inch. `2 in in cm` reads
    /// correctly for the same reason — the first `in` is followed by a word that cannot begin a
    /// value, so it stays an inch, and the second one converts.
    /// **The left side is everything; the right side is a unit.** `1 + 1 m in cm` converts the
    /// sum, so what comes before binds as loosely as possible — but what comes *after* is a unit
    /// expression and stops there. Read greedily, `100 cm in m + 5 m` converted a metre into
    /// six-metre units and answered `0.167 m`, silently, for a line that plainly means "one metre
    /// plus five". A unit is a name, a product of names, or one divided by another: `m`, `km/h`,
    /// `kg m/s^2` — never a sum.
    fn conversion(&mut self) -> Result<Expr, String> {
        let mut left = self.bit_or()?;
        loop {
            if self.starts_a_conversion() {
                self.at += 1;
                left = Expr::Convert(Box::new(left), Box::new(self.product()?));
                continue;
            }
            // A sum *after* a conversion, which the level below has already stopped reading:
            // `100 cm in m + 5 m` is six metres, and the `+ 5 m` belongs to the converted value
            // rather than to the unit. Without this the `+` was simply left over.
            let op = if self.eat(&Token::Plus) {
                Binary::Add
            } else if self.eat(&Token::Minus) {
                Binary::Subtract
            } else {
                return Ok(left);
            };
            left = Expr::Binary(op, Box::new(left), Box::new(self.bit_or()?));
        }
    }

    /// Whether the cursor is on a conversion keyword that has something to convert into.
    fn starts_a_conversion(&self) -> bool {
        let Some(Token::Word(word)) = self.peek() else {
            return false;
        };
        if !matches!(word.to_ascii_lowercase().as_str(), "in" | "to" | "as") {
            return false;
        }
        self.after().starts_a_value()
    }

    fn bit_or(&mut self) -> Result<Expr, String> {
        let mut left = self.bit_xor()?;
        while self.eat(&Token::Pipe) {
            left = Expr::Binary(Binary::Or, Box::new(left), Box::new(self.bit_xor()?));
        }
        Ok(left)
    }

    /// `xor` as a word: `^` is already the power operator, and changing that to match C would
    /// surprise everyone who has used a calculator.
    fn bit_xor(&mut self) -> Result<Expr, String> {
        let mut left = self.bit_and()?;
        while self.eat_word("xor") {
            left = Expr::Binary(Binary::Xor, Box::new(left), Box::new(self.bit_and()?));
        }
        Ok(left)
    }

    fn bit_and(&mut self) -> Result<Expr, String> {
        let mut left = self.shift()?;
        while self.eat(&Token::Amp) {
            left = Expr::Binary(Binary::And, Box::new(left), Box::new(self.shift()?));
        }
        Ok(left)
    }

    fn shift(&mut self) -> Result<Expr, String> {
        let mut left = self.sum()?;
        loop {
            let op = if self.eat(&Token::Shl) {
                Binary::Shl
            } else if self.eat(&Token::Shr) {
                Binary::Shr
            } else {
                return Ok(left);
            };
            left = Expr::Binary(op, Box::new(left), Box::new(self.sum()?));
        }
    }

    fn sum(&mut self) -> Result<Expr, String> {
        let mut left = self.product()?;
        loop {
            let op = if self.eat(&Token::Plus) {
                Binary::Add
            } else if self.eat(&Token::Minus) {
                Binary::Subtract
            } else {
                return Ok(left);
            };
            left = Expr::Binary(op, Box::new(left), Box::new(self.product()?));
        }
    }

    /// The written operators: `*`, `/`, `%`, and the words for them.
    fn product(&mut self) -> Result<Expr, String> {
        let mut left = self.juxtaposed()?;
        loop {
            let op = if self.eat(&Token::Star) {
                Binary::Multiply
            } else if self.eat(&Token::Slash) || self.eat_word("per") {
                Binary::Divide
            } else if self.eat_word("of") {
                // `20% of 50`, and `half of 8` if somebody has defined `half`. A multiplication
                // with a word for it, which is how the question is usually said out loud.
                Binary::Multiply
            } else if self.eat(&Token::Percent) && self.starts_a_value() {
                // `a % b` is a remainder; a trailing `%` was already taken by `postfix`.
                Binary::Modulo
            } else {
                return Ok(left);
            };
            left = Expr::Binary(op, Box::new(left), Box::new(self.juxtaposed()?));
        }
    }

    /// Two things side by side, which multiply — and bind **tighter than `/` does**.
    ///
    /// That is the rule that makes a unit readable. `100 m / 9.58 s` means a hundred metres per
    /// nine and a half seconds, and left-to-right precedence would read it as
    /// `(100 × m ÷ 9.58) × s` — metre-seconds, which is not a speed and not what anybody wrote.
    /// Binding juxtaposition first gives `(100 m) / (9.58 s)`, and `9.8 m/s^2` comes out as
    /// `(9.8 m) / (s²)` for the same reason.
    ///
    /// It is the convention every calculator that takes units seriously uses, and the one every
    /// physics textbook is typeset in.
    fn juxtaposed(&mut self) -> Result<Expr, String> {
        let mut left = self.prefix()?;
        while self.starts_a_value() {
            left = Expr::Binary(Binary::Multiply, Box::new(left), Box::new(self.prefix()?));
        }
        Ok(left)
    }

    /// Whether what comes next could begin a value, which is what makes juxtaposition work.
    ///
    /// A word here might be `in`, which ends the expression rather than multiplying it — so the
    /// conversion keywords are excluded, and a variable really called `in` is not expressible.
    fn starts_a_value(&self) -> bool {
        match self.peek() {
            Some(Token::Number(..)) | Some(Token::LParen) => true,
            Some(Token::Word(w)) => {
                let lower = w.to_ascii_lowercase();
                // `in` is the inch as well as the keyword, and which one it is depends on whether
                // anything follows it to convert into. `4 in` is four inches; `4 in cm` is a
                // conversion. See `conversion`, which asks the same question from the other side.
                if lower == "in" {
                    return !self.after().starts_a_value();
                }
                !matches!(lower.as_str(), "to" | "as" | "per" | "xor" | "of")
            }
            _ => false,
        }
    }

    /// The same parser, one token further on. Terminates because `at` only ever grows.
    fn after(&self) -> Parser<'_> {
        Parser {
            tokens: self.tokens,
            at: self.at + 1,
            // The lookahead inherits the depth it is being asked about, so a bracket already open
            // still counts against the limit inside it.
            depth: self.depth,
        }
    }

    fn prefix(&mut self) -> Result<Expr, String> {
        if self.eat(&Token::Minus) {
            return Ok(Expr::Unary(Unary::Negate, Box::new(self.prefix()?)));
        }
        if self.eat(&Token::Plus) {
            return self.prefix();
        }
        if self.eat(&Token::Tilde) {
            return Ok(Expr::Unary(Unary::Not, Box::new(self.prefix()?)));
        }
        self.power()
    }

    /// Right-associative, and tighter than the unary minus above it: `-2^2` is `-4`.
    fn power(&mut self) -> Result<Expr, String> {
        let left = self.postfix()?;
        if self.eat(&Token::Caret) {
            // The right side goes through `prefix` so `2^-1` reads.
            return Ok(Expr::Binary(
                Binary::Power,
                Box::new(left),
                Box::new(self.prefix()?),
            ));
        }
        Ok(left)
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut left = self.atom()?;
        loop {
            // A `%` with nothing to its right is a percentage; with a value after it, `product`
            // reads it as a remainder instead.
            if self.peek() == Some(&Token::Percent) && !self.next_starts_a_value() {
                self.at += 1;
                left = Expr::Percent(Box::new(left));
                continue;
            }
            if self.eat(&Token::Bang) {
                left = Expr::Factorial(Box::new(left));
                continue;
            }
            return Ok(left);
        }
    }

    fn next_starts_a_value(&self) -> bool {
        self.after().starts_a_value()
    }

    fn atom(&mut self) -> Result<Expr, String> {
        match self.peek().cloned() {
            Some(Token::Number(value, base)) => {
                self.at += 1;
                Ok(Expr::Number(value, base))
            }
            Some(Token::LParen) => {
                self.at += 1;
                let inner = self.nested(|p| p.conversion())?;
                if !self.eat(&Token::RParen) {
                    return Err("a bracket was opened and not closed".to_string());
                }
                Ok(inner)
            }
            Some(Token::Word(name)) => {
                self.at += 1;
                // `f(x)`, but only when the bracket is *immediately* there — `2 (1+1)` is a
                // multiplication and `sin (x)` is a call, and the difference is whether the name
                // is a function, which evaluation decides.
                if self.peek() == Some(&Token::LParen) {
                    self.at += 1;
                    let mut args = vec![self.conversion()?];
                    while self.eat(&Token::Comma) {
                        args.push(self.conversion()?);
                    }
                    if !self.eat(&Token::RParen) {
                        return Err(format!("{name}( was opened and not closed"));
                    }
                    return Ok(Expr::Call(name, args));
                }
                Ok(Expr::Name(name))
            }
            _ => Err(format!("expected a value, found {}", self.describe_here())),
        }
    }
}

#[cfg(test)]
#[path = "parse/tests.rs"]
mod tests;
