//! Pratt-style recursive-descent parser for the template-expression subset.
//!
//! Output is a small AST that maps cleanly onto the JS expression grammar.
//! Anything outside the supported subset returns a `ParseError` so the build
//! fails loud — silent partial support is the worst possible UX for a
//! security-driven precompiler.

use crate::lexer::{Spanned, Token};

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    // Literals
    Number(f64),
    Str(String),
    Template(String),
    Bool(bool),
    Null,
    Undefined,

    // Identifier reference. The codegen decides whether to read it via the
    // state proxy or treat it as a global (undefined → undefined, unlike
    // raw JS where bare identifiers would ReferenceError).
    Ident(String),

    // Member access: a.b or a?.b
    Member { object: Box<Expr>, property: String, optional: bool },

    // Computed member: a[expr]
    Index { object: Box<Expr>, index: Box<Expr> },

    // Function call: callee(args...)
    Call { callee: Box<Expr>, args: Vec<Expr> },

    // Unary
    Neg(Box<Expr>),
    Pos(Box<Expr>),
    Not(Box<Expr>),

    // Binary
    Binary { op: BinOp, left: Box<Expr>, right: Box<Expr> },

    // Logical (short-circuiting)
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Coalesce(Box<Expr>, Box<Expr>),

    // Ternary
    Ternary { cond: Box<Expr>, then_branch: Box<Expr>, else_branch: Box<Expr> },

    // Assignment (cv-model write side, `count++`, `flag = !flag`, etc.)
    Assign { target: Box<Expr>, op: AssignOp, value: Box<Expr> },

    // Postfix increment / decrement: count++, count--
    UpdatePostfix { target: Box<Expr>, increment: bool },
    UpdatePrefix  { target: Box<Expr>, increment: bool },

    // Object literal: { a: 1, b }
    Object(Vec<ObjectProp>),

    // Array literal: [1, 2, x]
    Array(Vec<Expr>),

    // Sequence (comma OR semicolon): (a, b) or (a; b) — used inside event
    // handlers like `@click="a = 1; b = 2"`. Both punctuators are treated
    // identically because templates already use them interchangeably.
    Sequence(Vec<Expr>),

    // Arrow function: `t => !t.done`, `(a, b) => a + b`. Block-body (`=>{...}`)
    // is intentionally not supported — it would push toward arbitrary code in
    // templates. Single-expression body only.
    Arrow { params: Vec<String>, body: Box<Expr> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectProp {
    KeyValue(String, Expr),
    Computed(Expr, Expr),     // [expr]: value
    Shorthand(String),        // { x } → { x: x }
    Spread(Expr),             // ...obj
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Lt, Le, Gt, Ge,
    Eq, NotEq, StrictEq, StrictNotEq,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AssignOp { Set, Add, Sub, Mul, Div, Mod }

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub pos: usize,
}

pub struct Parser<'a> {
    tokens: &'a [Spanned],
    cursor: usize,
    src_len: usize,
}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Spanned], src_len: usize) -> Self {
        Self { tokens, cursor: 0, src_len }
    }

    fn peek(&self) -> Option<&Token> { self.tokens.get(self.cursor).map(|s| &s.token) }
    fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.cursor + offset).map(|s| &s.token)
    }
    fn pos(&self) -> usize {
        self.tokens.get(self.cursor).map(|s| s.start).unwrap_or(self.src_len)
    }
    fn advance(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.cursor)?;
        self.cursor += 1;
        Some(tok.token.clone())
    }
    fn eat(&mut self, t: &Token) -> bool {
        if self.peek() == Some(t) { self.cursor += 1; true } else { false }
    }
    fn expect(&mut self, t: &Token, ctx: &str) -> Result<(), ParseError> {
        if self.eat(t) { Ok(()) } else {
            Err(ParseError {
                message: format!("expected `{:?}` {} (got `{}`)", t, ctx, self.peek().map(|p| format!("{}", p)).unwrap_or_else(|| "<eof>".into())),
                pos: self.pos(),
            })
        }
    }

    pub fn parse_full(&mut self) -> Result<Expr, ParseError> {
        // Allow a top-level sequence joined by comma OR semicolon so event
        // handlers like `@click="a = 1, b = 2"` and `@click="a = 1; b = 2"`
        // both parse cleanly. cv-model and {{ ... }} normally have a single
        // expression but pay no cost for this branch when there's no separator.
        let first = self.parse_assignment()?;
        if matches!(self.peek(), Some(Token::Comma) | Some(Token::Semicolon)) {
            self.advance();
            let mut items = vec![first];
            loop {
                // Tolerate trailing separators (`a; b;` and `a, b,`).
                if self.peek().is_none() { break; }
                items.push(self.parse_assignment()?);
                if matches!(self.peek(), Some(Token::Comma) | Some(Token::Semicolon)) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.ensure_eof()?;
            // Single survivor (e.g. `a;`) is just the expression itself —
            // no need to wrap it in a Sequence.
            return Ok(if items.len() == 1 { items.pop().unwrap() } else { Expr::Sequence(items) });
        }
        self.ensure_eof()?;
        Ok(first)
    }

    fn ensure_eof(&self) -> Result<(), ParseError> {
        if self.cursor != self.tokens.len() {
            return Err(ParseError {
                message: format!("unexpected trailing token `{}`",
                    self.peek().map(|p| format!("{}", p)).unwrap_or_else(|| "<eof>".into())),
                pos: self.pos(),
            });
        }
        Ok(())
    }

    // --- assignment (lowest precedence) ---
    fn parse_assignment(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_ternary()?;
        let op = match self.peek() {
            Some(Token::Assign)        => Some(AssignOp::Set),
            Some(Token::PlusAssign)    => Some(AssignOp::Add),
            Some(Token::MinusAssign)   => Some(AssignOp::Sub),
            Some(Token::StarAssign)    => Some(AssignOp::Mul),
            Some(Token::SlashAssign)   => Some(AssignOp::Div),
            Some(Token::PercentAssign) => Some(AssignOp::Mod),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let value = self.parse_assignment()?;
            if !is_assignable(&left) {
                return Err(ParseError {
                    message: "left-hand side of assignment is not assignable".into(),
                    pos: self.pos(),
                });
            }
            return Ok(Expr::Assign { target: Box::new(left), op, value: Box::new(value) });
        }
        Ok(left)
    }

    fn parse_ternary(&mut self) -> Result<Expr, ParseError> {
        let cond = self.parse_coalesce()?;
        if self.eat(&Token::Question) {
            let then_branch = self.parse_assignment()?;
            self.expect(&Token::Colon, "in ternary")?;
            let else_branch = self.parse_assignment()?;
            return Ok(Expr::Ternary {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch: Box::new(else_branch),
            });
        }
        Ok(cond)
    }

    fn parse_coalesce(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_or()?;
        while self.eat(&Token::QuestionQuestion) {
            let right = self.parse_or()?;
            left = Expr::Coalesce(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.eat(&Token::PipePipe) {
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_equality()?;
        while self.eat(&Token::AmpAmp) {
            let right = self.parse_equality()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match self.peek() {
                Some(Token::EqEq)     => BinOp::StrictEq,    // template `==` is treated as ===
                Some(Token::Eq)       => BinOp::StrictEq,
                Some(Token::BangEqEq) => BinOp::StrictNotEq,
                Some(Token::BangEq)   => BinOp::StrictNotEq, // template `!=` is treated as !==
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_addition()?;
        loop {
            let op = match self.peek() {
                Some(Token::Lt)   => BinOp::Lt,
                Some(Token::LtEq) => BinOp::Le,
                Some(Token::Gt)   => BinOp::Gt,
                Some(Token::GtEq) => BinOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.parse_addition()?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_addition(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_multiplication()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus)  => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplication()?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_multiplication(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star)    => BinOp::Mul,
                Some(Token::Slash)   => BinOp::Div,
                Some(Token::Percent) => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Some(Token::Bang) => { self.advance(); Ok(Expr::Not(Box::new(self.parse_unary()?))) }
            Some(Token::Minus) => { self.advance(); Ok(Expr::Neg(Box::new(self.parse_unary()?))) }
            Some(Token::Plus) => { self.advance(); Ok(Expr::Pos(Box::new(self.parse_unary()?))) }
            Some(Token::PlusPlus) => {
                self.advance();
                let target = self.parse_unary()?;
                if !is_assignable(&target) {
                    return Err(ParseError { message: "++ requires an assignable target".into(), pos: self.pos() });
                }
                Ok(Expr::UpdatePrefix { target: Box::new(target), increment: true })
            }
            Some(Token::MinusMinus) => {
                self.advance();
                let target = self.parse_unary()?;
                if !is_assignable(&target) {
                    return Err(ParseError { message: "-- requires an assignable target".into(), pos: self.pos() });
                }
                Ok(Expr::UpdatePrefix { target: Box::new(target), increment: false })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut node = self.parse_member_or_call()?;
        loop {
            match self.peek() {
                Some(Token::PlusPlus) => {
                    self.advance();
                    if !is_assignable(&node) {
                        return Err(ParseError { message: "++ requires an assignable target".into(), pos: self.pos() });
                    }
                    node = Expr::UpdatePostfix { target: Box::new(node), increment: true };
                }
                Some(Token::MinusMinus) => {
                    self.advance();
                    if !is_assignable(&node) {
                        return Err(ParseError { message: "-- requires an assignable target".into(), pos: self.pos() });
                    }
                    node = Expr::UpdatePostfix { target: Box::new(node), increment: false };
                }
                _ => break,
            }
        }
        Ok(node)
    }

    fn parse_member_or_call(&mut self) -> Result<Expr, ParseError> {
        let mut node = self.parse_primary()?;
        loop {
            match self.peek() {
                Some(Token::Dot) => {
                    self.advance();
                    let name = match self.advance() {
                        Some(Token::Ident(s)) => s,
                        other => return Err(ParseError {
                            message: format!("expected property name after `.` (got `{:?}`)", other),
                            pos: self.pos(),
                        }),
                    };
                    node = Expr::Member { object: Box::new(node), property: name, optional: false };
                }
                Some(Token::QuestionDot) => {
                    self.advance();
                    // Could be ?.prop, ?.[expr], or ?.(args). Handle prop and call;
                    // bracket access via ?. is rare and not supported.
                    match self.peek() {
                        Some(Token::Ident(_)) => {
                            let name = if let Some(Token::Ident(s)) = self.advance() { s } else { unreachable!() };
                            node = Expr::Member { object: Box::new(node), property: name, optional: true };
                        }
                        Some(Token::LParen) => {
                            self.advance();
                            let args = self.parse_args()?;
                            // Optional call simplifies to a regular call — runtime null check
                            // handled by the codegen wrapping the callee in `(callee == null ? undefined : callee(...))`.
                            node = Expr::Call { callee: Box::new(Expr::Member {
                                object: Box::new(node),
                                property: "__optional_call__".into(),
                                optional: true,
                            }), args };
                        }
                        other => return Err(ParseError {
                            message: format!("expected property after `?.` (got `{:?}`)", other),
                            pos: self.pos(),
                        }),
                    }
                }
                Some(Token::LBracket) => {
                    self.advance();
                    let index = self.parse_assignment()?;
                    self.expect(&Token::RBracket, "after computed index")?;
                    node = Expr::Index { object: Box::new(node), index: Box::new(index) };
                }
                Some(Token::LParen) => {
                    self.advance();
                    let args = self.parse_args()?;
                    node = Expr::Call { callee: Box::new(node), args };
                }
                _ => break,
            }
        }
        Ok(node)
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if self.eat(&Token::RParen) { return Ok(args); }
        loop {
            args.push(self.parse_assignment()?);
            if self.eat(&Token::RParen) { return Ok(args); }
            self.expect(&Token::Comma, "between call arguments")?;
            // Tolerate trailing comma: `foo(a, b,)`
            if self.eat(&Token::RParen) { return Ok(args); }
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let pos_start = self.pos();

        // Arrow function with bare single parameter: `t => body`.
        // Detect via 2-token lookahead: Ident followed by Arrow.
        if let (Some(Token::Ident(_)), Some(Token::Arrow)) = (self.peek(), self.peek_at(1)) {
            let name = if let Some(Token::Ident(s)) = self.advance() { s } else { unreachable!() };
            self.advance(); // consume `=>`
            let body = self.parse_assignment()?;
            return Ok(Expr::Arrow { params: vec![name], body: Box::new(body) });
        }

        let tok = self.advance().ok_or_else(|| ParseError {
            message: "unexpected end of expression".into(),
            pos: pos_start,
        })?;
        match tok {
            Token::Number(n)    => Ok(Expr::Number(n)),
            Token::String(s)    => Ok(Expr::Str(s)),
            Token::Template(s)  => Ok(Expr::Template(s)),
            Token::True         => Ok(Expr::Bool(true)),
            Token::False        => Ok(Expr::Bool(false)),
            Token::Null         => Ok(Expr::Null),
            Token::Undefined    => Ok(Expr::Undefined),
            Token::Ident(name)  => Ok(Expr::Ident(name)),
            Token::LParen => {
                // Could be:
                //   (expr)                     → parenthesized expression
                //   ()  =>  body               → arrow with no params
                //   (a, b, ...)  =>  body      → arrow with multiple params
                //   (a)  =>  body              → arrow with single param (in parens)
                //
                // We disambiguate by trying the parenthesized-expression path
                // first and reinterpreting after the `)` if `=>` follows.
                if matches!(self.peek(), Some(Token::RParen)) {
                    // ()  =>  body  — no-arg arrow
                    self.advance(); // )
                    if !matches!(self.peek(), Some(Token::Arrow)) {
                        return Err(ParseError {
                            message: "empty `()` is only valid as the parameter list of an arrow function".into(),
                            pos: self.pos(),
                        });
                    }
                    self.advance(); // =>
                    let body = self.parse_assignment()?;
                    return Ok(Expr::Arrow { params: vec![], body: Box::new(body) });
                }
                let saved = self.cursor;
                // Speculatively try to parse a comma-separated identifier list
                // followed by `) =>`. If that pattern holds, it's an arrow.
                if let Some(params) = self.try_parse_arrow_params() {
                    self.advance(); // =>
                    let body = self.parse_assignment()?;
                    return Ok(Expr::Arrow { params, body: Box::new(body) });
                }
                self.cursor = saved;
                let e = self.parse_assignment()?;
                self.expect(&Token::RParen, "after parenthesized expression")?;
                Ok(e)
            }
            Token::LBracket => {
                let mut items = Vec::new();
                if !self.eat(&Token::RBracket) {
                    loop {
                        items.push(self.parse_assignment()?);
                        if self.eat(&Token::RBracket) { break; }
                        self.expect(&Token::Comma, "between array elements")?;
                        if self.eat(&Token::RBracket) { break; }
                    }
                }
                Ok(Expr::Array(items))
            }
            Token::LBrace => {
                let mut props = Vec::new();
                if !self.eat(&Token::RBrace) {
                    loop {
                        // Spread
                        if self.eat(&Token::Spread) {
                            let value = self.parse_assignment()?;
                            props.push(ObjectProp::Spread(value));
                        } else if self.eat(&Token::LBracket) {
                            // Computed key
                            let key = self.parse_assignment()?;
                            self.expect(&Token::RBracket, "after computed key")?;
                            self.expect(&Token::Colon, "after computed key")?;
                            let value = self.parse_assignment()?;
                            props.push(ObjectProp::Computed(key, value));
                        } else {
                            let key = match self.advance() {
                                Some(Token::Ident(s)) => s,
                                Some(Token::String(s)) => s,
                                other => return Err(ParseError {
                                    message: format!("expected property name in object literal (got `{:?}`)", other),
                                    pos: self.pos(),
                                }),
                            };
                            if self.eat(&Token::Colon) {
                                let value = self.parse_assignment()?;
                                props.push(ObjectProp::KeyValue(key, value));
                            } else {
                                props.push(ObjectProp::Shorthand(key));
                            }
                        }
                        if self.eat(&Token::RBrace) { break; }
                        self.expect(&Token::Comma, "between object properties")?;
                        if self.eat(&Token::RBrace) { break; }
                    }
                }
                Ok(Expr::Object(props))
            }
            other => Err(ParseError {
                message: format!("unexpected token `{}` in expression", other),
                pos: pos_start,
            }),
        }
    }
}

fn is_assignable(e: &Expr) -> bool {
    matches!(e, Expr::Ident(_) | Expr::Member { .. } | Expr::Index { .. })
}

impl<'a> Parser<'a> {
    /// Speculative arrow-parameter parser. Used after seeing `(` to decide
    /// whether we're inside a parenthesized expression or an arrow function.
    /// Returns Some(params) only if the lookahead matches the strict shape
    /// `Ident (, Ident)* ) =>`. Anything else returns None and the caller
    /// rewinds the cursor and reparses as a regular expression.
    fn try_parse_arrow_params(&mut self) -> Option<Vec<String>> {
        let mut params = Vec::new();
        loop {
            match self.peek() {
                Some(Token::Ident(s)) => {
                    params.push(s.clone());
                    self.advance();
                }
                _ => return None,
            }
            match self.peek() {
                Some(Token::Comma) => { self.advance(); continue; }
                Some(Token::RParen) => break,
                _ => return None,
            }
        }
        // We're on `)` now; need `) =>`.
        self.advance(); // )
        if matches!(self.peek(), Some(Token::Arrow)) { Some(params) } else { None }
    }
}

pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let tokens = crate::lexer::tokenize(src).map_err(|e| ParseError {
        message: format!("lex error: {}", e.message),
        pos: e.pos,
    })?;
    let mut p = Parser::new(&tokens, src.len());
    p.parse_full()
}
