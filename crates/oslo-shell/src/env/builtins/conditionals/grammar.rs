//! The POSIX `test` grammar, evaluated recursively.
//!
//! The old implementation was a `match` on operand count that handled 0, 1, 2 and 3 and then
//! `Ok(0)` — so *every* expression with four or more words was true, silently. `[ -f /nope -a -f
//! /nope2 ]` succeeded; so did `[ ! a = a ]`; so did outright garbage. A shell that answers "yes"
//! to a question it did not parse is worse than one that refuses, because the caller has no way
//! to tell the difference.
//!
//! The grammar (bash's `test.c`, which is the de-facto specification, since POSIX leaves anything
//! past four operands unspecified):
//!
//! ```text
//! expr  ::= or
//! or    ::= and ( '-o' and )*
//! and   ::= term ( '-a' term )*
//! term  ::= '!'+ term | '(' expr ')' | <string> <binop> <string> | <unop> <string> | <string>
//! ```
//!
//! Two details that are easy to get wrong and are load-bearing:
//!
//! * **Short operand counts are not parsed with the grammar.** `[ ! ]` is the one-character string
//!   `!`, which is true; `[ = ]` is the string `=`, which is true; `[ -z ]` is the string `-z`.
//!   Only at four operands and above does the recursive parser take over, which is why the arity
//!   cases below exist rather than being folded into `term`.
//! * **`-a`/`-o` do not short-circuit.** `[ 1 -eq 1 -o abc -eq 1 ]` is an error in bash even
//!   though the left operand already decided the answer, because both sides are evaluated.

use super::operators::{self, Mode, TestError, TestResult};
use crate::env::scope::Environment;

/// Evaluate a `test` expression: the argument list with `[`/`]` already stripped.
pub(super) fn evaluate(env: &Environment, args: &[String]) -> TestResult<bool> {
    let mut parser = Parser { env, args, pos: 0 };
    let value = parser.dispatch_by_arity()?;
    if parser.pos != args.len() {
        // Words left over: the expression parsed, but not all of it. bash's wording.
        return Err(TestError::new("too many arguments"));
    }
    Ok(value)
}

struct Parser<'a> {
    env: &'a Environment,
    args: &'a [String],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn arg(&self, index: usize) -> Option<&'a str> {
        self.args.get(index).map(String::as_str)
    }

    fn current(&self) -> Option<&'a str> {
        self.arg(self.pos)
    }

    fn remaining(&self) -> usize {
        self.args.len() - self.pos
    }

    fn need(&self, index: usize) -> TestResult<&'a str> {
        self.arg(index)
            .ok_or_else(|| TestError::new("argument expected"))
    }

    /// The top level. Short forms have fixed meanings that the grammar would get wrong.
    fn dispatch_by_arity(&mut self) -> TestResult<bool> {
        match self.remaining() {
            0 => Ok(false),
            1 => self.one_argument(),
            2 => self.two_arguments(),
            3 => self.three_arguments(),
            4 => {
                if self.current() == Some("!") {
                    self.pos += 1;
                    return Ok(!self.three_arguments()?);
                }
                if self.current() == Some("(") && self.arg(self.pos + 3) == Some(")") {
                    self.pos += 1;
                    let value = self.two_arguments()?;
                    self.pos += 1; // the closing paren
                    return Ok(value);
                }
                self.expr()
            }
            _ => self.expr(),
        }
    }

    /// A bare string: true when non-empty.
    fn one_argument(&mut self) -> TestResult<bool> {
        let value = !self.need(self.pos)?.is_empty();
        self.pos += 1;
        Ok(value)
    }

    /// `! string` or `<unop> string`. Anything else is a syntax error — notably a bare
    /// two-word list like `[ a b ]`, which used to be silently false.
    fn two_arguments(&mut self) -> TestResult<bool> {
        let first = self.need(self.pos)?;
        let second = self.need(self.pos + 1)?;

        let value = if first == "!" {
            second.is_empty()
        } else if operators::is_unary_op(first) {
            operators::eval_unary(self.env, first, second)?
        } else {
            return Err(TestError::new(format!(
                "{}: unary operator expected",
                first
            )));
        };

        self.pos += 2;
        Ok(value)
    }

    /// `a <binop> b`, `! <two-word form>`, `( string )`, or a connective, in that order.
    fn three_arguments(&mut self) -> TestResult<bool> {
        let second = self.need(self.pos + 1)?;

        if operators::is_binary_op(second) {
            self.binary_operator()
        } else if second == "-a" || second == "-o" {
            // `[ x -a y ]` — two bare strings joined by a connective, whatever they spell:
            // `[ ! -a / ]` and `[ -z -a -z ]` are both true.
            let first = !self.need(self.pos)?.is_empty();
            let third = !self.need(self.pos + 2)?.is_empty();
            self.pos += 3;
            Ok(if second == "-a" {
                first && third
            } else {
                first || third
            })
        } else if self.current() == Some("!") {
            self.pos += 1;
            Ok(!self.two_arguments()?)
        } else if self.current() == Some("(") && self.arg(self.pos + 2) == Some(")") {
            self.pos += 1;
            let value = self.one_argument()?;
            self.pos += 1; // the closing paren
            Ok(value)
        } else {
            Err(TestError::new(format!(
                "{}: binary operator expected",
                second
            )))
        }
    }

    fn expr(&mut self) -> TestResult<bool> {
        if self.pos >= self.args.len() {
            return Err(TestError::new("argument expected"));
        }
        self.or_expr()
    }

    fn or_expr(&mut self) -> TestResult<bool> {
        let mut value = self.and_expr()?;
        while self.current() == Some("-o") {
            self.pos += 1;
            // Deliberately not `||`: the right-hand side is evaluated even when the left already
            // settled it, so its syntax errors still surface.
            let rhs = self.and_expr()?;
            value = value || rhs;
        }
        Ok(value)
    }

    fn and_expr(&mut self) -> TestResult<bool> {
        let mut value = self.term()?;
        while self.current() == Some("-a") {
            self.pos += 1;
            let rhs = self.term()?;
            value = value && rhs;
        }
        Ok(value)
    }

    fn term(&mut self) -> TestResult<bool> {
        if self.pos >= self.args.len() {
            return Err(TestError::new("argument expected"));
        }

        // A run of `!`s: each flips the sense of everything that follows, so `[ ! ! a = a ]` is
        // true. This is the arity-independent negation R5.2 asks for — the old code only looked
        // at `!` in the two-operand form.
        if self.current() == Some("!") {
            let mut negate = false;
            while self.current() == Some("!") {
                self.pos += 1;
                negate = !negate;
            }
            let value = self.term()?;
            return Ok(if negate { !value } else { value });
        }

        if self.current() == Some("(") {
            self.pos += 1;
            let value = self.expr()?;
            match self.current() {
                Some(")") => self.pos += 1,
                Some(found) => {
                    return Err(TestError::new(format!("`)' expected, found {}", found)));
                }
                None => return Err(TestError::new("`)' expected")),
            }
            return Ok(value);
        }

        // Dyadic wins over monadic: with room for three words and an operator in the middle,
        // `[ -f = -f ]` compares two strings rather than testing a file named `=`.
        if self.pos + 3 <= self.args.len() && operators::is_binary_op(self.need(self.pos + 1)?) {
            return self.binary_operator();
        }

        if self.pos + 2 <= self.args.len() && operators::is_unary_op(self.need(self.pos)?) {
            let op = self.need(self.pos)?;
            let target = self.need(self.pos + 1)?;
            let value = operators::eval_unary(self.env, op, target)?;
            self.pos += 2;
            return Ok(value);
        }

        self.one_argument()
    }

    fn binary_operator(&mut self) -> TestResult<bool> {
        let left = self.need(self.pos)?;
        let op = self.need(self.pos + 1)?;
        let right = self.need(self.pos + 2)?;
        let value = operators::eval_binary(Mode::Posix, left, op, right)?;
        self.pos += 3;
        Ok(value)
    }
}
