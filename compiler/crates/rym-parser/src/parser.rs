use rym_lexer::{Span, Token, TokenKind};
use rym_ast::{
    Ring, SourceFile,
    expr::{Arg, BinOp, Expr, ExprKind, FieldInit, MatchArm, OwnershipMode, Pattern, ResultVariant, UnOp},
    item::{Attr, AttrArg, CompilerExt, EnumDef, EnumVariant, ExtPhase, FnDef, Item, ItemKind, Param, TypeDef, FieldDef},
    stmt::{Stmt, StmtKind},
    ty::{Ty, TyKind},
};
use crate::error::ParseError;

pub struct Parser {
    tokens: Vec<Token>,
    /// Current position in `tokens`.
    pos: usize,
    /// Current control-flow nesting depth inside a function body.
    /// Rym's absolute flatness rule: max depth = 1.
    /// Entering if/for/while/loop increments this; exiting decrements it.
    control_depth: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        // Strip newlines — we use them only for zone detection, handled separately.
        let tokens = tokens
            .into_iter()
            .filter(|t| t.kind != TokenKind::Newline)
            .collect();
        Self { tokens, pos: 0, control_depth: 0 }
    }

    /// Parse a complete source file.
    pub fn parse_file(&mut self) -> Result<SourceFile, ParseError> {
        let start = self.span();

        // Optional ring declaration at the very top.
        let ring = if self.eat(TokenKind::Base) {
            Ring::Base
        } else {
            self.eat(TokenKind::Safe);
            Ring::Safe
        };

        let mut def_zone: Vec<Item> = Vec::new();
        let mut alg_zone: Vec<Stmt> = Vec::new();
        let mut in_alg = false;

        while !self.is_eof() {
            // `import` is always a definition.
            if self.peek_is(TokenKind::Import) {
                if in_alg {
                    return Err(ParseError::DefInAlgZone { span: self.span() });
                }
                def_zone.push(self.parse_import()?);
                continue;
            }

            // Compiler extension attribute `@name`.
            if self.peek_is(TokenKind::At) {
                if in_alg {
                    return Err(ParseError::DefInAlgZone { span: self.span() });
                }
                def_zone.push(self.parse_compiler_ext_item()?);
                continue;
            }

            // `fn` / `type` / `enum` → definition zone.
            if matches!(self.peek(), TokenKind::Fn | TokenKind::Type | TokenKind::Enum) {
                if in_alg {
                    return Err(ParseError::DefInAlgZone { span: self.span() });
                }
                def_zone.push(self.parse_item()?);
                continue;
            }

            // Anything else starts the algorithm zone.
            in_alg = true;
            alg_zone.push(self.parse_stmt()?);
        }

        let end = self.span();
        Ok(SourceFile {
            ring,
            def_zone,
            alg_zone,
            span: Span::new(start.start, end.end),
        })
    }

    // ── Items (definition zone) ───────────────────────────────

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        let span = self.span();
        match self.peek().clone() {
            TokenKind::Fn   => Ok(Item { kind: ItemKind::Fn(self.parse_fn()?), span }),
            TokenKind::Type => Ok(Item { kind: ItemKind::Type(self.parse_type_def()?), span }),
            TokenKind::Enum => Ok(Item { kind: ItemKind::Enum(self.parse_enum_def()?), span }),
            other => Err(ParseError::UnexpectedToken {
                expected: "fn, type, or enum",
                found: other,
                span,
            }),
        }
    }

    fn parse_import(&mut self) -> Result<Item, ParseError> {
        let span = self.span();
        self.expect(TokenKind::Import)?;
        let path = self.expect_str()?;
        Ok(Item { kind: ItemKind::Import(path), span })
    }

    fn parse_compiler_ext_item(&mut self) -> Result<Item, ParseError> {
        let span = self.span();
        self.expect(TokenKind::At)?;
        let phase_name = self.expect_ident()?;
        let phase = match phase_name.as_str() {
            "parser_rule" => ExtPhase::ParserRule,
            "ast_node"    => ExtPhase::AstNode,
            "type_rule"   => ExtPhase::TypeRule,
            "lower_pass"  => ExtPhase::LowerPass,
            "opt_pass"    => ExtPhase::OptPass,
            "backend"     => ExtPhase::Backend,
            _ => return Err(ParseError::UnexpectedToken {
                expected: "known compiler extension phase",
                found: TokenKind::Ident(phase_name),
                span,
            }),
        };
        // `(key = "value", ...)` — consume but ignore for now.
        if self.peek_is(TokenKind::LParen) {
            self.parse_attr_args()?;
        }
        // The next item must be a fn.
        let fn_name = if self.peek_is(TokenKind::Fn) {
            let f = self.parse_fn()?;
            f.name
        } else {
            return Err(ParseError::UnexpectedToken {
                expected: "fn after compiler extension attribute",
                found: self.peek().clone(),
                span: self.span(),
            });
        };
        Ok(Item {
            kind: ItemKind::CompilerExt(CompilerExt { phase, fn_name }),
            span,
        })
    }

    // ── fn definition ─────────────────────────────────────────

    fn parse_fn(&mut self) -> Result<FnDef, ParseError> {
        self.expect(TokenKind::Fn)?;
        let name = self.expect_ident()?;

        let attrs: Vec<Attr> = Vec::new();

        self.expect(TokenKind::LParen)?;
        let mut params = Vec::new();
        while !self.peek_is(TokenKind::RParen) {
            params.push(self.parse_param()?);
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen)?;

        self.expect(TokenKind::Arrow)?;
        let ret = self.parse_ty()?;

        // Reset nesting depth for each new function body.
        let saved_depth = self.control_depth;
        self.control_depth = 0;

        self.expect(TokenKind::LBrace)?;
        let body = self.parse_block()?;
        self.expect(TokenKind::RBrace)?;

        self.control_depth = saved_depth;

        Ok(FnDef { name, params, ret, body, attrs })
    }

    fn parse_param(&mut self) -> Result<Param, ParseError> {
        let mode = self.parse_ownership_mode();
        let name = self.expect_ident()?;
        self.expect(TokenKind::Colon)?;
        // Mode may also appear after the colon: `x: read i32`
        let mode = if mode == OwnershipMode::Inferred {
            self.parse_ownership_mode()
        } else {
            mode
        };
        let ty = self.parse_ty()?;
        Ok(Param { name, mode, ty })
    }

    // ── type / enum definitions ───────────────────────────────

    fn parse_type_def(&mut self) -> Result<TypeDef, ParseError> {
        self.expect(TokenKind::Type)?;
        let name = self.expect_ident()?;
        self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.peek_is(TokenKind::RBrace) {
            let fname = self.expect_ident()?;
            self.expect(TokenKind::Colon)?;
            let fty = self.parse_ty()?;
            fields.push(FieldDef { name: fname, ty: fty });
        }
        self.expect(TokenKind::RBrace)?;
        Ok(TypeDef { name, fields })
    }

    fn parse_enum_def(&mut self) -> Result<EnumDef, ParseError> {
        self.expect(TokenKind::Enum)?;
        let name = self.expect_ident()?;
        self.expect(TokenKind::LBrace)?;
        let mut variants = Vec::new();
        while !self.peek_is(TokenKind::RBrace) {
            let vname = self.expect_ident()?;
            let payload = if self.peek_is(TokenKind::Colon) {
                self.advance();
                Some(self.parse_ty()?)
            } else {
                None
            };
            variants.push(EnumVariant { name: vname, payload });
            self.eat(TokenKind::Comma);
        }
        self.expect(TokenKind::RBrace)?;
        Ok(EnumDef { name, variants })
    }

    // ── Statements ────────────────────────────────────────────

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        let span = self.span();
        match self.peek().clone() {
            // 定 / let — immutable binding
            TokenKind::Ding | TokenKind::Let => {
                self.advance();
                let name = self.expect_ident()?;
                let ty = if self.eat(TokenKind::Colon) { Some(self.parse_ty()?) } else { None };
                self.expect(TokenKind::Assign)?;
                let init = self.parse_expr()?;
                Ok(Stmt { kind: StmtKind::Let { name, ty, init }, span })
            }
            // 设 / var — mutable binding
            TokenKind::She | TokenKind::Var => {
                self.advance();
                let name = self.expect_ident()?;
                let ty = if self.eat(TokenKind::Colon) { Some(self.parse_ty()?) } else { None };
                self.expect(TokenKind::Assign)?;
                let init = self.parse_expr()?;
                Ok(Stmt { kind: StmtKind::Var { name, ty, init }, span })
            }
            TokenKind::Return => {
                self.advance();
                let expr = if self.is_eof() || self.peek_is(TokenKind::RBrace) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                Ok(Stmt { kind: StmtKind::Return(expr), span })
            }
            TokenKind::Defer => {
                self.advance();
                let expr = self.parse_expr()?;
                Ok(Stmt { kind: StmtKind::Defer(expr), span })
            }
            TokenKind::For => {
                self.advance();
                if self.control_depth >= 1 {
                    return Err(ParseError::IllegalNesting { span });
                }
                let binding = self.expect_ident()?;
                self.expect(TokenKind::In)?;
                let iter = self.parse_expr()?;
                self.control_depth += 1;
                self.expect(TokenKind::LBrace)?;
                let body = self.parse_block()?;
                self.expect(TokenKind::RBrace)?;
                self.control_depth -= 1;
                Ok(Stmt { kind: StmtKind::For { binding, iter, body }, span })
            }
            _ => {
                // Expression statement (including pipelines and assignments).
                let expr = self.parse_expr()?;
                // Check for assignment: `name = value`
                if self.eat(TokenKind::Assign) {
                    let value = self.parse_expr()?;
                    return Ok(Stmt { kind: StmtKind::Assign { target: expr, value }, span });
                }
                Ok(Stmt { kind: StmtKind::Expr(expr), span })
            }
        }
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        while !self.peek_is(TokenKind::RBrace) && !self.is_eof() {
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    // ── Expressions ───────────────────────────────────────────

    /// Entry point for expression parsing — handles pipe `|>` at lowest precedence.
    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_pipe()
    }

    fn parse_pipe(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_or()?;
        while self.eat(TokenKind::Pipe) {
            let right = self.parse_or()?;
            let span = Span::new(left.span.start, right.span.end);
            left = Expr { kind: ExprKind::Pipe { left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.eat(TokenKind::Or) {
            let right = self.parse_and()?;
            let span = Span::new(left.span.start, right.span.end);
            left = Expr { kind: ExprKind::BinOp { op: BinOp::Or, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_cmp()?;
        while self.eat(TokenKind::And) {
            let right = self.parse_cmp()?;
            let span = Span::new(left.span.start, right.span.end);
            left = Expr { kind: ExprKind::BinOp { op: BinOp::And, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_cmp(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_add()?;
        loop {
            let op = match self.peek().clone() {
                TokenKind::Eq    => BinOp::Eq,
                TokenKind::NotEq => BinOp::NotEq,
                TokenKind::Lt    => BinOp::Lt,
                TokenKind::LtEq  => BinOp::LtEq,
                TokenKind::Gt    => BinOp::Gt,
                TokenKind::GtEq  => BinOp::GtEq,
                _                => break,
            };
            self.advance();
            let right = self.parse_add()?;
            let span = Span::new(left.span.start, right.span.end);
            left = Expr { kind: ExprKind::BinOp { op, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek().clone() {
                TokenKind::Plus     => BinOp::Add,
                TokenKind::Minus    => BinOp::Sub,
                TokenKind::PlusPlus => BinOp::Concat,
                _                   => break,
            };
            self.advance();
            let right = self.parse_mul()?;
            let span = Span::new(left.span.start, right.span.end);
            left = Expr { kind: ExprKind::BinOp { op, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek().clone() {
                TokenKind::Star    => BinOp::Mul,
                TokenKind::Slash   => BinOp::Div,
                TokenKind::Percent => BinOp::Rem,
                _                  => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            let span = Span::new(left.span.start, right.span.end);
            left = Expr { kind: ExprKind::BinOp { op, left: Box::new(left), right: Box::new(right) }, span };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        if self.eat(TokenKind::Not) {
            let expr = self.parse_unary()?;
            let end = expr.span.end;
            return Ok(Expr { kind: ExprKind::UnOp { op: UnOp::Not, expr: Box::new(expr) }, span: Span::new(span.start, end) });
        }
        if self.eat(TokenKind::Minus) {
            let expr = self.parse_unary()?;
            let end = expr.span.end;
            return Ok(Expr { kind: ExprKind::UnOp { op: UnOp::Neg, expr: Box::new(expr) }, span: Span::new(span.start, end) });
        }
        if self.eat(TokenKind::Star) {
            let expr = self.parse_unary()?;
            let end = expr.span.end;
            return Ok(Expr { kind: ExprKind::Deref(Box::new(expr)), span: Span::new(span.start, end) });
        }
        if self.eat(TokenKind::Amp) {
            let expr = self.parse_unary()?;
            let end = expr.span.end;
            return Ok(Expr { kind: ExprKind::Ref(Box::new(expr)), span: Span::new(span.start, end) });
        }
        self.parse_postfix()
    }

    /// Handles `.field`, `[index]`, `.or_return`, `.or_panic(msg)`,
    /// `.or_else(default)`, `.or_zero`, `.or_nil`, `as Type`, and calls.
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            let span_start = expr.span.start;
            if self.eat(TokenKind::Dot) {
                match self.peek().clone() {
                    // `expr.alloc(T, count)` — allocator call
                    TokenKind::Ident(ref s) if s == "alloc" => {
                        self.advance(); // consume 'alloc'
                        self.expect(TokenKind::LParen)?;
                        let elem_ty = self.parse_ty()?;
                        self.expect(TokenKind::Comma)?;
                        let count = self.parse_expr()?;
                        self.expect(TokenKind::RParen)?;
                        let end = self.span().start;
                        expr = Expr {
                            kind: ExprKind::AllocCall {
                                allocator: Box::new(expr),
                                elem_ty,
                                count: Box::new(count),
                            },
                            span: Span::new(span_start, end),
                        };
                    }
                    TokenKind::OrReturn => {
                        self.advance();
                        self.eat(TokenKind::Question);
                        let end = self.span().start;
                        expr = Expr { kind: ExprKind::OrReturn(Box::new(expr)), span: Span::new(span_start, end) };
                    }
                    TokenKind::OrPanic => {
                        self.advance();
                        self.expect(TokenKind::LParen)?;
                        let msg = self.expect_str()?;
                        self.expect(TokenKind::RParen)?;
                        let end = self.span().start;
                        expr = Expr { kind: ExprKind::OrPanic(Box::new(expr), msg), span: Span::new(span_start, end) };
                    }
                    TokenKind::OrElse => {
                        self.advance();
                        self.expect(TokenKind::LParen)?;
                        let default = self.parse_expr()?;
                        self.expect(TokenKind::RParen)?;
                        let end = self.span().start;
                        expr = Expr { kind: ExprKind::OrElse(Box::new(expr), Box::new(default)), span: Span::new(span_start, end) };
                    }
                    TokenKind::OrZero => {
                        self.advance();
                        let end = self.span().start;
                        expr = Expr { kind: ExprKind::OrZero(Box::new(expr)), span: Span::new(span_start, end) };
                    }
                    TokenKind::OrNil => {
                        self.advance();
                        let end = self.span().start;
                        expr = Expr { kind: ExprKind::OrNil(Box::new(expr)), span: Span::new(span_start, end) };
                    }
                    _ => {
                        let field = self.expect_ident()?;
                        let end = self.span().start;
                        expr = Expr { kind: ExprKind::Field { base: Box::new(expr), field }, span: Span::new(span_start, end) };
                    }
                }
            } else if self.eat(TokenKind::LBracket) {
                let index = self.parse_expr()?;
                self.expect(TokenKind::RBracket)?;
                let end = self.span().start;
                expr = Expr { kind: ExprKind::Index { base: Box::new(expr), index: Box::new(index) }, span: Span::new(span_start, end) };
            } else if self.peek_is(TokenKind::LParen) {
                // Trailing call: `f(a, b)`
                let args = self.parse_call_args()?;
                let end = self.span().start;
                expr = Expr { kind: ExprKind::Call { callee: Box::new(expr), args }, span: Span::new(span_start, end) };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        match self.peek().clone() {
            TokenKind::Int(v) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Int(v), span })
            }
            TokenKind::Float(v) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Float(v), span })
            }
            TokenKind::Bool(v) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Bool(v), span })
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Str(s), span })
            }
            // `Ok(expr)` / `Err(expr)`
            TokenKind::Ident(ref s) if s == "Ok" => {
                self.advance();
                self.expect(TokenKind::LParen)?;
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                let end = self.span().start;
                Ok(Expr { kind: ExprKind::ResultCtor { variant: ResultVariant::Ok, inner: Box::new(inner) }, span: Span::new(span.start, end) })
            }
            TokenKind::Ident(ref s) if s == "Err" => {
                self.advance();
                self.expect(TokenKind::LParen)?;
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                let end = self.span().start;
                Ok(Expr { kind: ExprKind::ResultCtor { variant: ResultVariant::Err, inner: Box::new(inner) }, span: Span::new(span.start, end) })
            }
            TokenKind::Ident(name) => {
                self.advance();
                // Struct literal: `Name { field = expr, ... }`
                // Only treat `{` as a struct literal if the interior starts with `}` (empty)
                // or `Ident =` (field initializer). This prevents match/if subjects from
                // being greedily consumed as struct literals.
                if self.peek_is(TokenKind::LBrace) && self.is_struct_lit_opening() {
                    self.advance();
                    let mut fields = Vec::new();
                    while !self.peek_is(TokenKind::RBrace) {
                        let fname = self.expect_ident()?;
                        self.expect(TokenKind::Assign)?;
                        let fexpr = self.parse_expr()?;
                        fields.push(FieldInit { name: fname, expr: fexpr });
                        self.eat(TokenKind::Comma);
                    }
                    self.expect(TokenKind::RBrace)?;
                    let end = self.span().start;
                    return Ok(Expr { kind: ExprKind::StructLit { name, fields }, span: Span::new(span.start, end) });
                }
                Ok(Expr { kind: ExprKind::Ident(name), span })
            }
            TokenKind::AsmBang => {
                self.advance();
                self.expect(TokenKind::LParen)?;
                let template = self.expect_str()?;
                let mut args = Vec::new();
                while self.eat(TokenKind::Comma) {
                    args.push(self.parse_expr()?);
                }
                self.expect(TokenKind::RParen)?;
                let end = self.span().start;
                Ok(Expr { kind: ExprKind::Asm { template, args }, span: Span::new(span.start, end) })
            }
            TokenKind::LBracket => self.parse_array_or_matrix(),
            TokenKind::If => self.parse_if(),
            TokenKind::Match => self.parse_match(),
            TokenKind::LParen => {
                self.advance();
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                Ok(inner)
            }
            other => Err(ParseError::UnexpectedToken {
                expected: "expression",
                found: other,
                span,
            }),
        }
    }

    /// Parse `[e1, e2; e3, e4]` (matrix) or `[e1, e2, e3]` (array).
    /// `;` in the bracket signals a new row.
    fn parse_array_or_matrix(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        self.expect(TokenKind::LBracket)?;

        let mut rows: Vec<Vec<Expr>> = Vec::new();
        let mut current_row: Vec<Expr> = Vec::new();
        let mut has_semi = false;

        while !self.peek_is(TokenKind::RBracket) && !self.is_eof() {
            if self.eat(TokenKind::Semi) {
                has_semi = true;
                rows.push(std::mem::take(&mut current_row));
                continue;
            }
            current_row.push(self.parse_expr()?);
            if !self.peek_is(TokenKind::Semi) && !self.peek_is(TokenKind::RBracket) {
                self.eat(TokenKind::Comma);
            }
        }
        self.expect(TokenKind::RBracket)?;
        let end = self.span().start;

        if has_semi {
            // Matrix literal — push the last row.
            rows.push(current_row);
            let cols = rows.first().map(|r| r.len()).unwrap_or(0);
            let num_rows = rows.len();
            let elems: Vec<Expr> = rows.into_iter().flatten().collect();
            Ok(Expr {
                kind: ExprKind::MatrixLit { elems, rows: num_rows, cols },
                span: Span::new(span.start, end),
            })
        } else {
            Ok(Expr {
                kind: ExprKind::ArrayLit(current_row),
                span: Span::new(span.start, end),
            })
        }
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        self.expect(TokenKind::If)?;
        if self.control_depth >= 1 {
            return Err(ParseError::IllegalNesting { span });
        }
        let cond = self.parse_expr()?;
        self.control_depth += 1;
        self.expect(TokenKind::LBrace)?;
        let then_stmts = self.parse_block()?;
        self.expect(TokenKind::RBrace)?;
        let then_expr = Expr {
            span,
            kind: ExprKind::Block(then_stmts),
        };
        let else_expr = if self.eat(TokenKind::Else) {
            self.expect(TokenKind::LBrace)?;
            let else_stmts = self.parse_block()?;
            self.expect(TokenKind::RBrace)?;
            Some(Box::new(Expr { span, kind: ExprKind::Block(else_stmts) }))
        } else {
            None
        };
        self.control_depth -= 1;
        let end = self.span().start;
        Ok(Expr {
            kind: ExprKind::If { cond: Box::new(cond), then: Box::new(then_expr), else_ : else_expr },
            span: Span::new(span.start, end),
        })
    }

    fn parse_match(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        self.expect(TokenKind::Match)?;
        if self.control_depth >= 1 {
            return Err(ParseError::IllegalNesting { span });
        }
        let subject = self.parse_expr()?;
        self.control_depth += 1;
        self.expect(TokenKind::LBrace)?;
        let mut arms = Vec::new();
        while !self.peek_is(TokenKind::RBrace) {
            let pattern = self.parse_pattern()?;
            self.expect(TokenKind::FatArrow)?;
            let body = self.parse_expr()?;
            arms.push(MatchArm { pattern, body });
            self.eat(TokenKind::Comma);
        }
        self.expect(TokenKind::RBrace)?;
        self.control_depth -= 1;
        let end = self.span().start;
        Ok(Expr {
            kind: ExprKind::Match { subject: Box::new(subject), arms },
            span: Span::new(span.start, end),
        })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        match self.peek().clone() {
            TokenKind::Ident(ref s) if s == "_" => {
                self.advance();
                Ok(Pattern::Wildcard)
            }
            TokenKind::Else => {
                self.advance();
                Ok(Pattern::Else)
            }
            TokenKind::Ident(ty) => {
                self.advance();
                if self.eat(TokenKind::Dot) {
                    let variant = self.expect_ident()?;
                    Ok(Pattern::Variant { ty, variant })
                } else {
                    Ok(Pattern::Lit(ExprKind::Ident(ty)))
                }
            }
            TokenKind::Int(v) => {
                self.advance();
                Ok(Pattern::Lit(ExprKind::Int(v)))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Pattern::Lit(ExprKind::Str(s)))
            }
            other => Err(ParseError::UnexpectedToken {
                expected: "pattern",
                found: other,
                span: self.span(),
            }),
        }
    }

    fn parse_call_args(&mut self) -> Result<Vec<Arg>, ParseError> {
        self.expect(TokenKind::LParen)?;
        let mut args = Vec::new();
        while !self.peek_is(TokenKind::RParen) {
            let mode = self.parse_ownership_mode();
            // Optional label: `前缀: expr`
            let label = if let TokenKind::Ident(name) = self.peek().clone() {
                if self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::Colon) {
                    self.advance(); // name
                    self.advance(); // ':'
                    Some(name)
                } else {
                    None
                }
            } else {
                None
            };
            let expr = self.parse_expr()?;
            args.push(Arg { label, mode, expr });
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen)?;
        Ok(args)
    }

    fn parse_attr_args(&mut self) -> Result<Vec<AttrArg>, ParseError> {
        self.expect(TokenKind::LParen)?;
        let mut args = Vec::new();
        while !self.peek_is(TokenKind::RParen) {
            let key = self.expect_ident()?;
            self.expect(TokenKind::Assign)?;
            let value = self.expect_str()?;
            args.push(AttrArg { key, value });
            self.eat(TokenKind::Comma);
        }
        self.expect(TokenKind::RParen)?;
        Ok(args)
    }

    // ── Types ─────────────────────────────────────────────────

    fn parse_ty(&mut self) -> Result<Ty, ParseError> {
        let span = self.span();
        // `[N]T` — fixed array  OR  `[]T` — slice
        if self.eat(TokenKind::LBracket) {
            if let TokenKind::Int(n) = self.peek().clone() {
                let size = n as usize;
                self.advance();
                self.expect(TokenKind::RBracket)?;
                let elem = self.parse_ty()?;
                let end = elem.span.end;
                return Ok(Ty { kind: TyKind::Array { size, elem: Box::new(elem) }, span: Span::new(span.start, end) });
            }
            self.expect(TokenKind::RBracket)?;
            let inner = self.parse_ty()?;
            let end = inner.span.end;
            return Ok(Ty { kind: TyKind::Slice(Box::new(inner)), span: Span::new(span.start, end) });
        }
        // `fn(T, U) -> R` — function pointer type
        if self.eat(TokenKind::Fn) {
            self.expect(TokenKind::LParen)?;
            let mut params = Vec::new();
            while !self.peek_is(TokenKind::RParen) {
                params.push(self.parse_ty()?);
                if !self.eat(TokenKind::Comma) { break; }
            }
            self.expect(TokenKind::RParen)?;
            self.expect(TokenKind::Arrow)?;
            let ret = self.parse_ty()?;
            let end = ret.span.end;
            return Ok(Ty { kind: TyKind::FnPtr { params, ret: Box::new(ret) }, span: Span::new(span.start, end) });
        }
        // `*mut T` / `*T`
        if self.eat(TokenKind::Star) {
            if self.eat(TokenKind::Mut) {
                let inner = self.parse_ty()?;
                let end = inner.span.end;
                return Ok(Ty { kind: TyKind::PtrMut(Box::new(inner)), span: Span::new(span.start, end) });
            }
            let inner = self.parse_ty()?;
            let end = inner.span.end;
            return Ok(Ty { kind: TyKind::Ptr(Box::new(inner)), span: Span::new(span.start, end) });
        }
        self.advance();
        let kind = match self.tokens[self.pos - 1].kind.clone() {
            TokenKind::I8    => TyKind::I8,
            TokenKind::I16   => TyKind::I16,
            TokenKind::I32   => TyKind::I32,
            TokenKind::I64   => TyKind::I64,
            TokenKind::U8    => TyKind::U8,
            TokenKind::U16   => TyKind::U16,
            TokenKind::U32   => TyKind::U32,
            TokenKind::U64   => TyKind::U64,
            TokenKind::F32   => TyKind::F32,
            TokenKind::F64   => TyKind::F64,
            TokenKind::BoolTy  => TyKind::Bool,
            TokenKind::Usize => TyKind::Usize,
            TokenKind::StrTy   => TyKind::Str,
            TokenKind::Void  => TyKind::Void,
            TokenKind::Ident(name) => {
                // `Result(T, E)` / `Option(T)`
                if name == "Result" && self.peek_is(TokenKind::LParen) {
                    self.advance();
                    let ok_ty = self.parse_ty()?;
                    self.expect(TokenKind::Comma)?;
                    let err_ty = self.parse_ty()?;
                    self.expect(TokenKind::RParen)?;
                    TyKind::Result(Box::new(ok_ty), Box::new(err_ty))
                } else if name == "Option" && self.peek_is(TokenKind::LParen) {
                    self.advance();
                    let inner = self.parse_ty()?;
                    self.expect(TokenKind::RParen)?;
                    TyKind::Option(Box::new(inner))
                } else if name == "Allocator" {
                    TyKind::Allocator
                } else {
                    TyKind::Named(name)
                }
            }
            other => return Err(ParseError::UnexpectedToken {
                expected: "type",
                found: other,
                span,
            }),
        };
        Ok(Ty { kind, span: Span::new(span.start, self.span().start) })
    }

    // ── Ownership mode ────────────────────────────────────────

    fn parse_ownership_mode(&mut self) -> OwnershipMode {
        match self.peek() {
            TokenKind::Read => { self.advance(); OwnershipMode::Read }
            TokenKind::Mut  => { self.advance(); OwnershipMode::Mut  }
            TokenKind::Move => { self.advance(); OwnershipMode::Move }
            _               => OwnershipMode::Inferred,
        }
    }

    // ── Token stream helpers ──────────────────────────────────

    fn peek(&self) -> &TokenKind {
        self.tokens
            .get(self.pos)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    fn peek_is(&self, kind: TokenKind) -> bool {
        self.peek() == &kind
    }

    fn span(&self) -> Span {
        self.tokens
            .get(self.pos)
            .map(|t| t.span)
            .unwrap_or(Span::new(0, 0))
    }

    fn advance(&mut self) -> &TokenKind {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        &self.tokens[self.pos - 1].kind
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.peek_is(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: TokenKind) -> Result<(), ParseError> {
        if self.eat(kind.clone()) {
            Ok(())
        } else {
            Err(ParseError::UnexpectedToken {
                expected: "expected token",
                found: self.peek().clone(),
                span: self.span(),
            })
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Ident(s) => { self.advance(); Ok(s) }
            other => Err(ParseError::UnexpectedToken {
                expected: "identifier",
                found: other,
                span: self.span(),
            }),
        }
    }

    fn expect_str(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Str(s) => { self.advance(); Ok(s) }
            other => Err(ParseError::UnexpectedToken {
                expected: "string literal",
                found: other,
                span: self.span(),
            }),
        }
    }

    fn is_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    /// Returns true if the current token is `{` and the content looks like a
    /// struct literal: either `{}` or `{ ident = ...`. This disambiguates
    /// `Name { field = val }` from `match Name { arm => ... }`.
    fn is_struct_lit_opening(&self) -> bool {
        // pos is currently at `{`
        let after_brace = self.tokens.get(self.pos + 1).map(|t| &t.kind);
        let after_ident = self.tokens.get(self.pos + 2).map(|t| &t.kind);
        match (after_brace, after_ident) {
            (Some(TokenKind::RBrace), _) => true,
            (Some(TokenKind::Ident(_)), Some(TokenKind::Assign)) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rym_lexer::Lexer;

    fn parse(src: &str) -> SourceFile {
        let tokens = Lexer::new(src).tokenize().unwrap();
        Parser::new(tokens).parse_file().unwrap()
    }

    #[test]
    fn empty_file() {
        let f = parse("");
        assert!(f.def_zone.is_empty());
        assert!(f.alg_zone.is_empty());
    }

    #[test]
    fn base_ring_declaration() {
        let f = parse("base\nfn foo() -> void {}");
        assert_eq!(f.ring, Ring::Base);
        assert_eq!(f.def_zone.len(), 1);
    }

    #[test]
    fn simple_fn() {
        let f = parse("fn add(a: read i32, b: read i32) -> i32 { return a }");
        assert_eq!(f.def_zone.len(), 1);
        match &f.def_zone[0].kind {
            ItemKind::Fn(fn_def) => {
                assert_eq!(fn_def.name, "add");
                assert_eq!(fn_def.params.len(), 2);
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn chinese_fn() {
        let f = parse("fn 解析(原始: read []u8) -> void {}");
        match &f.def_zone[0].kind {
            ItemKind::Fn(fn_def) => assert_eq!(fn_def.name, "解析"),
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn let_binding() {
        let f = parse("定 x = 42");
        assert_eq!(f.alg_zone.len(), 1);
        assert!(matches!(f.alg_zone[0].kind, StmtKind::Let { .. }));
    }

    #[test]
    fn var_binding() {
        let f = parse("设 x = 42");
        assert!(matches!(f.alg_zone[0].kind, StmtKind::Var { .. }));
    }

    #[test]
    fn pipeline_expr() {
        let f = parse("a |> b() |> c()");
        assert_eq!(f.alg_zone.len(), 1);
        match &f.alg_zone[0].kind {
            StmtKind::Expr(e) => assert!(matches!(e.kind, ExprKind::Pipe { .. })),
            _ => panic!("expected expr stmt"),
        }
    }

    #[test]
    fn import() {
        let f = parse(r#"import "safe/fs.rym""#);
        assert!(matches!(f.def_zone[0].kind, ItemKind::Import(_)));
    }

    #[test]
    fn type_def() {
        let f = parse("type 请求 { 路径: str }");
        match &f.def_zone[0].kind {
            ItemKind::Type(t) => {
                assert_eq!(t.name, "请求");
                assert_eq!(t.fields.len(), 1);
            }
            _ => panic!("expected type"),
        }
    }

    #[test]
    fn or_return() {
        let f = parse("定 x = foo().or_return?");
        match &f.alg_zone[0].kind {
            StmtKind::Let { init, .. } => {
                assert!(matches!(init.kind, ExprKind::OrReturn(_)));
            }
            _ => panic!(),
        }
    }
}
