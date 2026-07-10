use rym_ast::{
    SourceFile,
    expr::{Expr, ExprKind, OwnershipMode, ResultVariant, BinOp},
    item::{FnDef, Item, ItemKind},
    stmt::{Stmt, StmtKind},
    ty::{Ty, TyKind},
};
use rym_lexer::Span;
use crate::error::SemaError;
use crate::scope::{Binding, FnSig, ResolvedTy, Scope};
use crate::ownership;

pub struct TyChecker {
    scope: Scope,
    errors: Vec<SemaError>,
}

impl TyChecker {
    pub fn new() -> Self {
        Self { scope: Scope::new(), errors: Vec::new() }
    }

    /// Run semantic analysis on a parsed source file.
    /// Returns all collected errors (empty = success).
    pub fn check(&mut self, file: &SourceFile) -> Vec<SemaError> {
        // Pass 1: register all top-level fn signatures so forward calls work.
        for item in &file.def_zone {
            self.register_item(item);
        }

        // Pass 2: check all fn bodies.
        for item in &file.def_zone {
            self.check_item(item);
        }

        // Pass 3: check algorithm zone statements.
        for stmt in &file.alg_zone {
            self.check_stmt(stmt);
        }

        std::mem::take(&mut self.errors)
    }

    // ── Registration pass ─────────────────────────────────────

    fn register_item(&mut self, item: &Item) {
        match &item.kind {
            ItemKind::Fn(fn_def) => {
                let sig = FnSig {
                    params: fn_def.params.iter().map(|p| {
                        (p.name.clone(), p.mode.clone(), self.resolve_ty(&p.ty))
                    }).collect(),
                    ret: self.resolve_ty(&fn_def.ret),
                };
                self.scope.fns.insert(fn_def.name.clone(), sig);
            }
            ItemKind::Type(type_def) => {
                // Register struct field layout so Field access can be typed.
                let fields: Vec<(String, ResolvedTy)> = type_def.fields.iter()
                    .map(|f| (f.name.clone(), self.resolve_ty(&f.ty)))
                    .collect();
                self.scope.types.insert(type_def.name.clone(), fields);
            }
            ItemKind::Enum(enum_def) => {
                self.scope.enums.insert(enum_def.name.clone());
                // Register each variant as a zero-arg or one-arg constructor.
                for variant in &enum_def.variants {
                    let ret_ty = ResolvedTy::Named(enum_def.name.clone());
                    let params = if let Some(payload_ty) = &variant.payload {
                        vec![("value".into(), OwnershipMode::Move, self.resolve_ty(payload_ty))]
                    } else {
                        vec![]
                    };
                    let qualified = format!("{}.{}", enum_def.name, variant.name);
                    self.scope.fns.insert(qualified, FnSig { params, ret: ret_ty });
                }
            }
            _ => {}
        }
    }

    // ── Item checking ─────────────────────────────────────────

    fn check_item(&mut self, item: &Item) {
        match &item.kind {
            ItemKind::Fn(fn_def) => self.check_fn(fn_def),
            ItemKind::Type(_) | ItemKind::Enum(_) | ItemKind::Import(_) => {}
            ItemKind::CompilerExt(_) => {}
        }
    }

    fn check_fn(&mut self, fn_def: &FnDef) {
        self.scope.push();

        // Bind parameters into scope.
        for param in &fn_def.params {
            let ty = self.resolve_ty(&param.ty);
            self.scope.define(param.name.clone(), Binding {
                ty,
                mode:    param.mode.clone(),
                mutable: param.mode == OwnershipMode::Mut,
                moved:   false,
                span:    Span::new(0, 0),
            });
        }

        let ret_ty = self.resolve_ty(&fn_def.ret);

        for stmt in &fn_def.body {
            self.check_stmt(stmt);
        }

        // Verify return type on last return stmt (basic check).
        if let Some(last) = fn_def.body.last() {
            if let StmtKind::Return(Some(expr)) = &last.kind {
                let expr_ty = self.infer_expr(expr);
                if !self.ty_compatible(&expr_ty, &ret_ty) {
                    self.errors.push(SemaError::TypeMismatch {
                        expected: ret_ty.display(),
                        found:    expr_ty.display(),
                        span:     expr.span,
                    });
                }
            }
        }

        self.scope.pop();
    }

    // ── Statement checking ────────────────────────────────────

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let { name, ty, init } => {
                let init_ty = self.infer_expr(init);
                let declared_ty = ty.as_ref().map(|t| self.resolve_ty(t)).unwrap_or(init_ty.clone());
                if !self.ty_compatible(&init_ty, &declared_ty) {
                    self.errors.push(SemaError::TypeMismatch {
                        expected: declared_ty.display(),
                        found:    init_ty.display(),
                        span:     stmt.span,
                    });
                }
                self.scope.define(name.clone(), Binding {
                    ty:      declared_ty,
                    mode:    OwnershipMode::Read,
                    mutable: false,
                    moved:   false,
                    span:    stmt.span,
                });
            }

            StmtKind::Var { name, ty, init } => {
                let init_ty = self.infer_expr(init);
                let declared_ty = ty.as_ref().map(|t| self.resolve_ty(t)).unwrap_or(init_ty.clone());
                if !self.ty_compatible(&init_ty, &declared_ty) {
                    self.errors.push(SemaError::TypeMismatch {
                        expected: declared_ty.display(),
                        found:    init_ty.display(),
                        span:     stmt.span,
                    });
                }
                self.scope.define(name.clone(), Binding {
                    ty:      declared_ty,
                    mode:    OwnershipMode::Mut,
                    mutable: true,
                    moved:   false,
                    span:    stmt.span,
                });
            }

            StmtKind::Return(expr) => {
                if let Some(e) = expr {
                    self.infer_expr(e);
                }
            }

            StmtKind::Defer(expr) => { self.infer_expr(expr); }

            StmtKind::Assign { target, value } => {
                // Check that the target is mutable.
                if let ExprKind::Ident(name) = &target.kind {
                    if let Err(e) = ownership::check_mutate(name, &self.scope, target.span) {
                        self.errors.push(e);
                    }
                }
                let _val_ty = self.infer_expr(value);
            }

            StmtKind::For { binding, iter, body } => {
                let iter_ty = self.infer_expr(iter);
                let elem_ty = match &iter_ty {
                    ResolvedTy::Slice(inner) => *inner.clone(),
                    _ => ResolvedTy::Unknown,
                };
                self.scope.push();
                self.scope.define(binding.clone(), Binding {
                    ty:      elem_ty,
                    mode:    OwnershipMode::Read,
                    mutable: false,
                    moved:   false,
                    span:    stmt.span,
                });
                for s in body { self.check_stmt(s); }
                self.scope.pop();
            }

            StmtKind::While { cond, body } => {
                self.infer_expr(cond);
                self.scope.push();
                for s in body { self.check_stmt(s); }
                self.scope.pop();
            }

            StmtKind::Loop(body) => {
                self.scope.push();
                for s in body { self.check_stmt(s); }
                self.scope.pop();
            }

            StmtKind::Expr(expr) => { self.infer_expr(expr); }
        }
    }

    // ── Expression type inference ─────────────────────────────

    fn infer_expr(&mut self, expr: &Expr) -> ResolvedTy {
        match &expr.kind {
            ExprKind::Int(_)   => ResolvedTy::I64,
            ExprKind::Float(_) => ResolvedTy::F64,
            ExprKind::Bool(_)  => ResolvedTy::Bool,
            ExprKind::Str(_)   => ResolvedTy::Str,

            ExprKind::Ident(name) => {
                // Enum type names used as namespace prefix (e.g. `Color` in `Color.Red`)
                // are handled by the Field branch — suppress "undefined" here.
                if self.scope.enums.contains(name) {
                    return ResolvedTy::Named(name.clone());
                }
                if let Some(b) = self.scope.lookup(name) {
                    if b.moved {
                        self.errors.push(SemaError::UseAfterMove {
                            name: name.clone(),
                            span: expr.span,
                        });
                    }
                    b.ty.clone()
                } else {
                    self.errors.push(SemaError::Undefined {
                        name: name.clone(),
                        span: expr.span,
                    });
                    ResolvedTy::Unknown
                }
            }

            ExprKind::Call { callee, args } => {
                // Resolve callee name.
                let fn_name = match &callee.kind {
                    ExprKind::Ident(n) => n.clone(),
                    ExprKind::Field { field, .. } => field.clone(),
                    _ => String::new(),
                };

                let ret = if let Some(sig) = self.scope.fns.get(&fn_name).cloned() {
                    // Arity check.
                    if args.len() != sig.params.len() {
                        self.errors.push(SemaError::ArgCountMismatch {
                            name:     fn_name.clone(),
                            expected: sig.params.len(),
                            found:    args.len(),
                            span:     expr.span,
                        });
                    }
                    // Ownership-mode check per argument.
                    for (arg, (pname, pmode, _pty)) in args.iter().zip(sig.params.iter()) {
                        if arg.mode != OwnershipMode::Inferred && arg.mode != *pmode {
                            self.errors.push(SemaError::OwnershipModeMismatch {
                                param:    pname.clone(),
                                expected: format!("{:?}", pmode),
                                found:    format!("{:?}", arg.mode),
                                span:     expr.span,
                            });
                        }
                        // If argument is moved, mark the source binding.
                        if arg.mode == OwnershipMode::Move {
                            if let ExprKind::Ident(n) = &arg.expr.kind {
                                if let Err(e) = ownership::check_move(n, &self.scope, arg.expr.span) {
                                    self.errors.push(e);
                                }
                                self.scope.mark_moved(n);
                            }
                        }
                    }
                    sig.ret.clone()
                } else {
                    // Unknown function — still infer arg types.
                    for arg in args { self.infer_expr(&arg.expr); }
                    ResolvedTy::Unknown
                };
                ret
            }

            ExprKind::Pipe { left, right } => {
                // Ownership check on pipe.
                if let Err(e) = ownership::check_pipe(left, right, &self.scope, expr.span) {
                    self.errors.push(e);
                }
                let left_ty = self.infer_expr(left);
                // Right side: `left` is implicitly the first argument.
                self.infer_piped_call(right, &left_ty, expr.span)
            }

            ExprKind::Field { base, field } => {
                // Check for `EnumName.Variant` before treating as struct field access.
                if let ExprKind::Ident(type_name) = &base.kind {
                    let qualified = format!("{}.{}", type_name, field);
                    if let Some(sig) = self.scope.fns.get(&qualified).cloned() {
                        return sig.ret.clone();
                    }
                }
                let base_ty = self.infer_expr(base);
                // Look up the field in the type table.
                let struct_name = match &base_ty {
                    ResolvedTy::Named(n) => Some(n.clone()),
                    _ => None,
                };
                if let Some(name) = struct_name {
                    if let Some(fields) = self.scope.types.get(&name).cloned() {
                        if let Some((_, ty)) = fields.iter().find(|(n, _)| n == field) {
                            return ty.clone();
                        } else {
                            self.errors.push(SemaError::Undefined {
                                name: format!("{name}.{field}"),
                                span: expr.span,
                            });
                        }
                    }
                }
                ResolvedTy::Unknown
            }

            ExprKind::Index { base, index } => {
                let base_ty = self.infer_expr(base);
                self.infer_expr(index);
                match base_ty {
                    ResolvedTy::Slice(inner) => *inner,
                    _ => ResolvedTy::Unknown,
                }
            }

            // Error-handling operators — inner must be Result.
            ExprKind::OrReturn(inner) => {
                let ty = self.infer_expr(inner);
                if !ty.is_result() && ty != ResolvedTy::Unknown {
                    self.errors.push(SemaError::OrReturnNonResult { span: expr.span });
                }
                match ty {
                    ResolvedTy::Result(ok, _) => *ok,
                    _ => ResolvedTy::Unknown,
                }
            }
            ExprKind::OrPanic(inner, _) => {
                let ty = self.infer_expr(inner);
                match ty {
                    ResolvedTy::Result(ok, _) | ResolvedTy::Option(ok) => *ok,
                    _ => ty,
                }
            }
            ExprKind::OrElse(inner, default) => {
                let ty = self.infer_expr(inner);
                self.infer_expr(default);
                match ty {
                    ResolvedTy::Result(ok, _) | ResolvedTy::Option(ok) => *ok,
                    _ => ty,
                }
            }
            ExprKind::OrZero(inner) | ExprKind::OrNil(inner) => {
                let ty = self.infer_expr(inner);
                match ty {
                    ResolvedTy::Result(ok, _) | ResolvedTy::Option(ok) => *ok,
                    _ => ty,
                }
            }

            ExprKind::ArrayLit(elems) => {
                let elem_ty = elems.first()
                    .map(|e| self.infer_expr(e))
                    .unwrap_or(ResolvedTy::Unknown);
                for e in elems.iter().skip(1) { self.infer_expr(e); }
                let len = elems.len();
                ResolvedTy::Array { size: len, elem: Box::new(elem_ty) }
            }

            ExprKind::MatrixLit { elems, rows, cols } => {
                for e in elems { self.infer_expr(e); }
                ResolvedTy::Array {
                    size: rows * cols,
                    elem: Box::new(ResolvedTy::I64),
                }
            }

            ExprKind::AllocCall { allocator, elem_ty, count } => {
                self.infer_expr(allocator);
                self.infer_expr(count);
                let inner = self.resolve_ty(elem_ty);
                ResolvedTy::Ptr(Box::new(inner))
            }

            ExprKind::BinOp { op, left, right } => {
                let lt = self.infer_expr(left);
                let rt = self.infer_expr(right);
                match op {
                    BinOp::Concat => {
                        if lt != ResolvedTy::Str && lt != ResolvedTy::Unknown {
                            self.errors.push(SemaError::TypeMismatch {
                                expected: "str".into(),
                                found:    lt.display(),
                                span:     left.span,
                            });
                        }
                        let _ = rt;
                        ResolvedTy::Str
                    }
                    _ => lt,
                }
            }

            ExprKind::UnOp { expr: inner, .. } => self.infer_expr(inner),

            ExprKind::StructLit { name, fields } => {
                if let Some(layout) = self.scope.types.get(name).cloned() {
                    for f in fields {
                        let val_ty = self.infer_expr(&f.expr);
                        if let Some((_, expected_ty)) = layout.iter().find(|(n, _)| n == &f.name) {
                            if !self.ty_compatible(&val_ty, expected_ty) {
                                self.errors.push(SemaError::TypeMismatch {
                                    expected: expected_ty.display(),
                                    found:    val_ty.display(),
                                    span:     expr.span,
                                });
                            }
                        } else {
                            self.errors.push(SemaError::Undefined {
                                name: format!("{name}.{}", f.name),
                                span: expr.span,
                            });
                        }
                    }
                } else {
                    // Unknown type — still infer field exprs.
                    for f in fields { self.infer_expr(&f.expr); }
                }
                ResolvedTy::Named(name.clone())
            }

            ExprKind::ResultCtor { variant, inner } => {
                let inner_ty = self.infer_expr(inner);
                match variant {
                    ResultVariant::Ok  =>
                        ResolvedTy::Result(Box::new(inner_ty), Box::new(ResolvedTy::Unknown)),
                    ResultVariant::Err =>
                        ResolvedTy::Result(Box::new(ResolvedTy::Unknown), Box::new(inner_ty)),
                }
            }

            ExprKind::Cast { expr: inner, ty } => {
                self.infer_expr(inner);
                self.resolve_ty(ty)
            }

            ExprKind::Ref(inner)   => {
                let ty = self.infer_expr(inner);
                ResolvedTy::Ptr(Box::new(ty))
            }
            ExprKind::Deref(inner) => {
                let ty = self.infer_expr(inner);
                match ty {
                    ResolvedTy::Ptr(inner) | ResolvedTy::PtrMut(inner) => *inner,
                    _ => ResolvedTy::Unknown,
                }
            }

            ExprKind::Block(stmts) => {
                self.scope.push();
                for s in stmts { self.check_stmt(s); }
                self.scope.pop();
                ResolvedTy::Void
            }

            ExprKind::If { cond, then, else_ } => {
                let cond_ty = self.infer_expr(cond);
                if cond_ty != ResolvedTy::Bool && cond_ty != ResolvedTy::Unknown {
                    self.errors.push(SemaError::TypeMismatch {
                        expected: "bool".into(),
                        found:    cond_ty.display(),
                        span:     cond.span,
                    });
                }
                let then_ty = self.infer_expr(then);
                if let Some(e) = else_ { self.infer_expr(e); }
                then_ty
            }

            ExprKind::Match { subject, arms } => {
                self.infer_expr(subject);
                let mut arm_ty = ResolvedTy::Unknown;
                for arm in arms {
                    arm_ty = self.infer_expr(&arm.body);
                }
                arm_ty
            }
        }
    }

    /// Infer the return type of a pipe's right side, treating `left_ty` as
    /// the implicit first argument (so arity is params.len() - 1 remaining).
    fn infer_piped_call(&mut self, right: &Expr, _left_ty: &ResolvedTy, span: Span) -> ResolvedTy {
        match &right.kind {
            ExprKind::Call { callee, args } => {
                let fn_name = match &callee.kind {
                    ExprKind::Ident(n) => n.clone(),
                    ExprKind::Field { field, .. } => field.clone(),
                    _ => String::new(),
                };
                if let Some(sig) = self.scope.fns.get(&fn_name).cloned() {
                    // Pipe passes left as first arg — so explicit args count must be params - 1.
                    let expected_explicit = sig.params.len().saturating_sub(1);
                    if args.len() != expected_explicit {
                        self.errors.push(SemaError::ArgCountMismatch {
                            name:     fn_name.clone(),
                            expected: expected_explicit,
                            found:    args.len(),
                            span,
                        });
                    }
                    for arg in args { self.infer_expr(&arg.expr); }
                    sig.ret.clone()
                } else {
                    for arg in args { self.infer_expr(&arg.expr); }
                    ResolvedTy::Unknown
                }
            }
            _ => self.infer_expr(right),
        }
    }

    // ── Type resolution ───────────────────────────────────────

    fn resolve_ty(&self, ty: &Ty) -> ResolvedTy {
        match &ty.kind {
            TyKind::I8    => ResolvedTy::I8,
            TyKind::I16   => ResolvedTy::I16,
            TyKind::I32   => ResolvedTy::I32,
            TyKind::I64   => ResolvedTy::I64,
            TyKind::U8    => ResolvedTy::U8,
            TyKind::U16   => ResolvedTy::U16,
            TyKind::U32   => ResolvedTy::U32,
            TyKind::U64   => ResolvedTy::U64,
            TyKind::F32   => ResolvedTy::F32,
            TyKind::F64   => ResolvedTy::F64,
            TyKind::Bool  => ResolvedTy::Bool,
            TyKind::Usize => ResolvedTy::Usize,
            TyKind::Str   => ResolvedTy::Str,
            TyKind::Void  => ResolvedTy::Void,
            TyKind::Allocator => ResolvedTy::Allocator,
            TyKind::Named(n)  => ResolvedTy::Named(n.clone()),
            TyKind::Slice(t)  => ResolvedTy::Slice(Box::new(self.resolve_ty(t))),
            TyKind::Ptr(t)    => ResolvedTy::Ptr(Box::new(self.resolve_ty(t))),
            TyKind::PtrMut(t) => ResolvedTy::PtrMut(Box::new(self.resolve_ty(t))),
            TyKind::Result(ok, err) =>
                ResolvedTy::Result(Box::new(self.resolve_ty(ok)), Box::new(self.resolve_ty(err))),
            TyKind::Option(t) => ResolvedTy::Option(Box::new(self.resolve_ty(t))),
            TyKind::Array { size, elem } =>
                ResolvedTy::Array { size: *size, elem: Box::new(self.resolve_ty(elem)) },
            TyKind::FnPtr { params, ret } =>
                ResolvedTy::FnPtr {
                    params: params.iter().map(|p| self.resolve_ty(p)).collect(),
                    ret:    Box::new(self.resolve_ty(ret)),
                },
        }
    }

    /// Permissive compatibility: Unknown matches anything, and structural types
    /// are checked recursively so partial inference (e.g. Result(i64, Unknown))
    /// is accepted where a concrete wrapper type is expected.
    fn ty_compatible(&self, found: &ResolvedTy, expected: &ResolvedTy) -> bool {
        if *found == ResolvedTy::Unknown || *expected == ResolvedTy::Unknown {
            return true;
        }
        match (found, expected) {
            (ResolvedTy::Result(fo, fe), ResolvedTy::Result(eo, ee)) =>
                self.ty_compatible(fo, eo) && self.ty_compatible(fe, ee),
            (ResolvedTy::Option(f), ResolvedTy::Option(e)) =>
                self.ty_compatible(f, e),
            (ResolvedTy::Slice(f), ResolvedTy::Slice(e)) =>
                self.ty_compatible(f, e),
            (ResolvedTy::Ptr(f), ResolvedTy::Ptr(e)) =>
                self.ty_compatible(f, e),
            (ResolvedTy::PtrMut(f), ResolvedTy::PtrMut(e)) =>
                self.ty_compatible(f, e),
            (ResolvedTy::Array { size: fs, elem: fe }, ResolvedTy::Array { size: es, elem: ee }) =>
                fs == es && self.ty_compatible(fe, ee),
            (ResolvedTy::FnPtr { params: fp, ret: fr }, ResolvedTy::FnPtr { params: ep, ret: er }) =>
                fp.len() == ep.len()
                && fp.iter().zip(ep.iter()).all(|(a, b)| self.ty_compatible(a, b))
                && self.ty_compatible(fr, er),
            _ => found == expected,
        }
    }
}

impl Default for TyChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rym_lexer::Lexer;
    use rym_parser::Parser;

    fn check(src: &str) -> Vec<SemaError> {
        let tokens = Lexer::new(src).tokenize().unwrap();
        let file   = Parser::new(tokens).parse_file().unwrap();
        TyChecker::new().check(&file)
    }

    fn assert_ok(src: &str) {
        let errs = check(src);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
    }

    fn assert_err(src: &str) {
        let errs = check(src);
        assert!(!errs.is_empty(), "expected errors but got none");
    }

    #[test]
    fn let_binding_ok() {
        assert_ok("定 x = 42");
    }

    #[test]
    fn var_binding_ok() {
        assert_ok("设 x = 42");
    }

    #[test]
    fn mutate_immutable_binding() {
        assert_err("定 x = 42\nx = 10");
    }

    #[test]
    fn simple_fn_ok() {
        assert_ok("fn add(a: read i32, b: read i32) -> i32 { return a }");
    }

    #[test]
    fn undefined_variable() {
        assert_err("定 x = y");
    }

    #[test]
    fn if_non_bool_cond() {
        assert_err("if 42 { return 1 }");
    }

    #[test]
    fn or_return_on_result_ok() {
        assert_ok(
            "fn foo() -> Result(i64, str) { return Ok(1) }\n\
             定 x = foo().or_return?"
        );
    }

    #[test]
    fn pipeline_ok() {
        assert_ok(
            "fn double(n: read i32) -> i32 { return n }\n\
             定 x = 1\n\
             x |> double()"
        );
    }

    #[test]
    fn chinese_fn_ok() {
        assert_ok("fn 解析(原始: read []u8) -> void {}");
    }

    #[test]
    fn struct_lit_ok() {
        assert_ok(
            "type 请求 { 路径: str }\n\
             定 r = 请求{ 路径=\"/\" }"
        );
    }

    #[test]
    fn struct_field_type_ok() {
        assert_ok(
            "type Point { x: i64\ny: i64 }\n\
             fn get_x(p: read Point) -> i64 { return p.x }\n\
             定 p = Point{ x=1, y=2 }\n\
             定 x = get_x(p)"
        );
    }

    #[test]
    fn struct_unknown_field_err() {
        assert_err(
            "type Point { x: i64 }\n\
             定 p = Point{ x=1 }\n\
             定 v = p.z"
        );
    }

    #[test]
    fn struct_wrong_field_type_err() {
        assert_err(
            "type Point { x: i64 }\n\
             定 p = Point{ x=true }"
        );
    }
}
