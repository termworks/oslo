//! Recursive-descent parser for arithmetic expressions.
//!
//! One function per precedence level, lowest first. The old parser had two levels and `break`d out
//! of both on any operator it did not know, discarding the rest of the expression *without an
//! error* — `$((5>3))` evaluated to 5. Here every level either consumes its operator or hands the
//! token back, and `parse` refuses to return unless the token stream is exhausted.

use crate::expand::arithmetic::lexer::{CompoundOp, Token};
use oslo_base::error::{Result, ShellError};

/// Deepest parenthesis (or unary-operator) nesting an arithmetic expression may use.
///
/// The grammar is recursive descent, so `$(( ((((1)))) ))` costs a stack frame per parenthesis and
/// `$(( ----1 ))` one per sign; 50 000 of either overflowed the stack and aborted the shell.
///
/// The number is measured, not guessed. One nesting level does not cost one frame — it descends
/// the whole precedence chain — which works out to roughly 20 KiB of stack per level in a debug
/// build. The first limit here was 100, which needs over 2 MiB and so overflowed a spawned
/// thread's default stack: the guard against crashing crashed. 32 leaves margin on the smallest
/// stack oslo is likely to get and is still far past anything a human writes;
/// `nesting_at_the_limit_fits_a_small_stack` is what keeps that true.
const MAX_DEPTH: usize = 32;

fn too_deep() -> ShellError {
    ShellError::ExecutionError("maximum nesting level exceeded".to_string())
}

fn syntax(msg: &str) -> ShellError {
    ShellError::ExpansionError(msg.to_string())
}

/// A binary operator with no evaluation-order subtlety: both sides always run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl From<CompoundOp> for BinOp {
    fn from(op: CompoundOp) -> Self {
        match op {
            CompoundOp::Add => BinOp::Add,
            CompoundOp::Sub => BinOp::Sub,
            CompoundOp::Mul => BinOp::Mul,
            CompoundOp::Div => BinOp::Div,
            CompoundOp::Rem => BinOp::Rem,
            CompoundOp::Shl => BinOp::Shl,
            CompoundOp::Shr => BinOp::Shr,
            CompoundOp::BitAnd => BinOp::BitAnd,
            CompoundOp::BitOr => BinOp::BitOr,
            CompoundOp::BitXor => BinOp::BitXor,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Pos,
    /// Logical `!`: 1 when the operand is zero.
    Not,
    /// Bitwise `~`.
    BitNot,
}

/// Parsed arithmetic. Short-circuiting and assignment are separate variants rather than `Binary`
/// cases because they must not evaluate both sides, or must write back.
/// What an expression names: a variable, or one element of an array.
///
/// A struct rather than two `Expr` variants because every operation that touches a variable —
/// reading it, assigning to it, stepping it — applies equally to an element. `((a[i]++))` is not a
/// special case of `((i++))`, it is the same case with a subscript, and splitting them meant four
/// pairs of near-identical arms that could drift apart.
///
/// The subscript is an [`Expr`] because it is arithmetic in its own right: `a[i+1]` and
/// `a[n%4]` are both ordinary, and `BASH_VERSINFO[0]` — the one that started this — is the
/// simplest member of the family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub index: Option<Box<Expr>>,
}

impl Ref {
    /// A plain variable, with no subscript.
    pub fn plain(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            index: None,
        }
    }
}

impl From<&str> for Ref {
    fn from(name: &str) -> Self {
        Ref::plain(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Number(i64),
    Var(Ref),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    LogicalAnd(Box<Expr>, Box<Expr>),
    LogicalOr(Box<Expr>, Box<Expr>),
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>),
    Comma(Box<Expr>, Box<Expr>),
    /// `name = rhs`, or `name op= rhs` when the operator is present.
    Assign(Ref, Option<BinOp>, Box<Expr>),
    /// `++name` / `--name`; the value is the variable *after* the change.
    PreStep(Ref, i64),
    /// `name++` / `name--`; the value is the variable *before* the change.
    PostStep(Ref, i64),
}

/// Parse a complete expression. An empty token stream is 0, matching `$(( ))`.
pub fn parse(tokens: &[Token]) -> Result<Expr> {
    if tokens.is_empty() {
        return Ok(Expr::Number(0));
    }
    let mut p = Parser { tokens, pos: 0 };
    let expr = p.comma(0)?;
    if p.pos < tokens.len() {
        // The whole point of R3.1: a trailing `)` or a stray operator is a diagnosed error, never
        // a partial answer.
        return Err(ShellError::ExpansionError(format!(
            "arithmetic syntax error near token {:?}",
            tokens[p.pos]
        )));
    }
    Ok(expr)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.pos)
    }

    fn eat(&mut self, tok: &Token) -> bool {
        if self.peek() == Some(tok) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// `expr (, expr)*`, the lowest precedence level.
    fn comma(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.assignment(depth)?;
        while self.eat(&Token::Comma) {
            let right = self.assignment(depth)?;
            left = Expr::Comma(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// Assignment is right-associative, and its left side has to turn out to be a variable.
    ///
    /// The conditional level is parsed first and *then* inspected: that is the standard way to get
    /// `x == 1` and `x = 1` to disagree without lookahead, and it gives `1 = 2` the same
    /// "assignment to non-variable" diagnosis bash produces.
    fn assignment(&mut self, depth: usize) -> Result<Expr> {
        let left = self.conditional(depth)?;
        let Some(op) = self.peek().and_then(Token::assign_op) else {
            return Ok(left);
        };
        let Expr::Var(name) = left else {
            return Err(syntax("attempted assignment to non-variable"));
        };
        self.pos += 1;
        let value = self.assignment(depth)?;
        Ok(Expr::Assign(name, op.map(BinOp::from), Box::new(value)))
    }

    fn conditional(&mut self, depth: usize) -> Result<Expr> {
        let cond = self.logical_or(depth)?;
        if !self.eat(&Token::Question) {
            return Ok(cond);
        }
        // The middle arm is delimited by `:`, so a comma inside it is unambiguous — bash allows it.
        let then = self.comma(depth)?;
        if !self.eat(&Token::Colon) {
            return Err(syntax("expected `:` in conditional expression"));
        }
        // The else arm stays at this level so `a ? b : c ? d : e` nests to the right.
        let other = self.assignment(depth)?;
        Ok(Expr::Conditional(
            Box::new(cond),
            Box::new(then),
            Box::new(other),
        ))
    }

    fn logical_or(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.logical_and(depth)?;
        while self.eat(&Token::OrOr) {
            let right = self.logical_and(depth)?;
            left = Expr::LogicalOr(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn logical_and(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.bit_or(depth)?;
        while self.eat(&Token::AndAnd) {
            let right = self.bit_or(depth)?;
            left = Expr::LogicalAnd(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn bit_or(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.bit_xor(depth)?;
        while self.eat(&Token::Pipe) {
            let right = self.bit_xor(depth)?;
            left = Expr::Binary(BinOp::BitOr, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn bit_xor(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.bit_and(depth)?;
        while self.eat(&Token::Caret) {
            let right = self.bit_and(depth)?;
            left = Expr::Binary(BinOp::BitXor, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn bit_and(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.equality(depth)?;
        while self.eat(&Token::Amp) {
            let right = self.equality(depth)?;
            left = Expr::Binary(BinOp::BitAnd, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn equality(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.relational(depth)?;
        loop {
            let op = match self.peek() {
                Some(Token::EqEq) => BinOp::Eq,
                Some(Token::Ne) => BinOp::Ne,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.relational(depth)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn relational(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.shift(depth)?;
        loop {
            let op = match self.peek() {
                Some(Token::Lt) => BinOp::Lt,
                Some(Token::Le) => BinOp::Le,
                Some(Token::Gt) => BinOp::Gt,
                Some(Token::Ge) => BinOp::Ge,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.shift(depth)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn shift(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.additive(depth)?;
        loop {
            let op = match self.peek() {
                Some(Token::Shl) => BinOp::Shl,
                Some(Token::Shr) => BinOp::Shr,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.additive(depth)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn additive(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.multiplicative(depth)?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.multiplicative(depth)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn multiplicative(&mut self, depth: usize) -> Result<Expr> {
        let mut left = self.power(depth)?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                Some(Token::Percent) => BinOp::Rem,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.power(depth)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    /// `**` is right-associative and binds *looser* than unary minus: bash gives `-2 ** 2` as 4,
    /// because its left operand is a full unary expression.
    fn power(&mut self, depth: usize) -> Result<Expr> {
        let left = self.unary(depth)?;
        if !self.eat(&Token::Pow) {
            return Ok(left);
        }
        let right = self.power(depth)?;
        Ok(Expr::Binary(BinOp::Pow, Box::new(left), Box::new(right)))
    }

    fn unary(&mut self, depth: usize) -> Result<Expr> {
        if depth > MAX_DEPTH {
            return Err(too_deep());
        }
        let op = match self.peek() {
            Some(Token::Minus) => UnOp::Neg,
            Some(Token::Plus) => UnOp::Pos,
            Some(Token::Not) => UnOp::Not,
            Some(Token::Tilde) => UnOp::BitNot,
            Some(tok @ (Token::Inc | Token::Dec)) => {
                let delta = if *tok == Token::Inc { 1 } else { -1 };
                self.pos += 1;
                let target = self.unary(depth + 1)?;
                let Expr::Var(name) = target else {
                    return Err(syntax("attempted assignment to non-variable"));
                };
                return Ok(Expr::PreStep(name, delta));
            }
            _ => return self.postfix(depth),
        };
        self.pos += 1;
        Ok(Expr::Unary(op, Box::new(self.unary(depth + 1)?)))
    }

    fn postfix(&mut self, depth: usize) -> Result<Expr> {
        let value = self.primary(depth)?;
        let delta = match self.peek() {
            Some(Token::Inc) => 1,
            Some(Token::Dec) => -1,
            _ => return Ok(value),
        };
        // `(x)++` is not an lvalue; leave the token for `parse` to reject rather than inventing a
        // meaning for it.
        let Expr::Var(name) = &value else {
            return Ok(value);
        };
        let name = name.clone();
        self.pos += 1;
        Ok(Expr::PostStep(name, delta))
    }

    /// The `[expr]` after a name, if there is one.
    ///
    /// Only immediately after the name: `a [0]` is not a subscript in bash either, and the space
    /// is not something the tokeniser preserves — so `[` here is always the subscript's.
    fn subscript(&mut self, name: String, depth: usize) -> Result<Ref> {
        if !self.eat(&Token::LBracket) {
            return Ok(Ref::plain(name));
        }
        let index = self.comma(depth + 1)?;
        if !self.eat(&Token::RBracket) {
            return Err(syntax("missing `]` in arithmetic subscript"));
        }
        Ok(Ref {
            name,
            index: Some(Box::new(index)),
        })
    }

    fn primary(&mut self, depth: usize) -> Result<Expr> {
        let tok = self
            .peek()
            .ok_or_else(|| syntax("Unexpected end of arithmetic expression"))?;
        match tok {
            Token::Number(n) => {
                self.pos += 1;
                Ok(Expr::Number(*n))
            }
            Token::Ident(name) => {
                let name = name.clone();
                self.pos += 1;
                Ok(Expr::Var(self.subscript(name, depth)?))
            }
            Token::LParen => {
                self.pos += 1;
                let inner = self.comma(depth + 1)?;
                if !self.eat(&Token::RParen) {
                    return Err(syntax("missing `)` in arithmetic expression"));
                }
                Ok(inner)
            }
            other => Err(ShellError::ExpansionError(format!(
                "arithmetic syntax error near token {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expand::arithmetic::lexer::tokenize;

    fn ast(expr: &str) -> Expr {
        parse(&tokenize(expr).expect("scan")).expect("parse")
    }

    fn n(v: i64) -> Box<Expr> {
        Box::new(Expr::Number(v))
    }

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        assert_eq!(
            ast("1 + 2 * 3"),
            Expr::Binary(
                BinOp::Add,
                n(1),
                Box::new(Expr::Binary(BinOp::Mul, n(2), n(3)))
            )
        );
    }

    #[test]
    fn subtraction_is_left_associative() {
        assert_eq!(
            ast("10 - 2 - 3"),
            Expr::Binary(
                BinOp::Sub,
                Box::new(Expr::Binary(BinOp::Sub, n(10), n(2))),
                n(3)
            )
        );
    }

    #[test]
    fn power_is_right_associative() {
        assert_eq!(
            ast("2 ** 3 ** 2"),
            Expr::Binary(
                BinOp::Pow,
                n(2),
                Box::new(Expr::Binary(BinOp::Pow, n(3), n(2)))
            )
        );
    }

    /// bash reads `-2 ** 2` as `(-2) ** 2`, not `-(2 ** 2)`.
    #[test]
    fn unary_minus_binds_tighter_than_power() {
        assert_eq!(
            ast("-2 ** 2"),
            Expr::Binary(BinOp::Pow, Box::new(Expr::Unary(UnOp::Neg, n(2))), n(2))
        );
    }

    #[test]
    fn assignment_is_right_associative() {
        assert_eq!(
            ast("x = y = 3"),
            Expr::Assign(
                "x".into(),
                None,
                Box::new(Expr::Assign("y".into(), None, n(3)))
            )
        );
    }

    #[test]
    fn compound_assignment_carries_its_operator() {
        assert_eq!(
            ast("x <<= 2"),
            Expr::Assign("x".into(), Some(BinOp::Shl), n(2))
        );
    }

    #[test]
    fn conditional_nests_to_the_right() {
        assert_eq!(
            ast("1 ? 2 : 3 ? 4 : 5"),
            Expr::Conditional(n(1), n(2), Box::new(Expr::Conditional(n(3), n(4), n(5))))
        );
    }

    /// Comma is looser than `?:`, so it takes the conditional as its left operand.
    #[test]
    fn comma_is_the_loosest_level() {
        assert_eq!(
            ast("1 ? 2 : 3 , 4"),
            Expr::Comma(Box::new(Expr::Conditional(n(1), n(2), n(3))), n(4))
        );
    }

    #[test]
    fn unconsumed_input_is_an_error() {
        for expr in ["1 2", "1 +", "(1", "1)", "1 ? 2", "* 3"] {
            assert!(
                parse(&tokenize(expr).expect("scan")).is_err(),
                "{expr} should not parse"
            );
        }
    }

    #[test]
    fn assignment_to_a_non_variable_is_rejected() {
        for expr in ["1 = 2", "x = 2 = 3", "1 += 2"] {
            let e = parse(&tokenize(expr).expect("scan")).expect_err(expr);
            assert!(e.to_string().contains("non-variable"), "{expr}: {e}");
        }
    }

    #[test]
    fn empty_expression_is_zero() {
        assert_eq!(parse(&[]).expect("parse"), Expr::Number(0));
    }
}
