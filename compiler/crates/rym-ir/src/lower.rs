use rym_ast::{
    SourceFile,
    expr::{BinOp, Expr, ExprKind, OwnershipMode, ResultVariant, UnOp},
    item::{FnDef, Item, ItemKind},
    stmt::{Stmt, StmtKind},
    ty::{Ty, TyKind},
};
use rym_lexer::Span;
use crate::{BasicBlock, Instr, IrFunc, IrMode, IrModule, IrParam, IrTy, Op, Terminator};

/// Lowers an AST `SourceFile` into an `IrModule`.
pub struct Lowerer {
    /// Counter for generating unique SSA names.
    next_id: usize,
    /// Counter for generating unique block labels.
    next_block: usize,
    /// Instructions accumulated for the current basic block.
    instrs: Vec<Instr>,
    /// Completed basic blocks for the current function.
    blocks: Vec<BasicBlock>,
    /// Label of the current basic block.
    current_label: String,
}

impl Lowerer {
    pub fn new() -> Self {
        Self {
            next_id:       0,
            next_block:    0,
            instrs:        Vec::new(),
            blocks:        Vec::new(),
            current_label: "entry".into(),
        }
    }

    /// Lower a full source file. Ignores non-fn items for now.
    pub fn lower_file(&mut self, file: &SourceFile, module_name: &str) -> IrModule {
        let mut funcs = Vec::new();
        for item in &file.def_zone {
            if let Some(f) = self.lower_item(item) {
                funcs.push(f);
            }
        }

        // Algorithm zone: wrap into an implicit `__main` function.
        if !file.alg_zone.is_empty() {
            self.reset_fn("__main");
            for stmt in &file.alg_zone {
                self.lower_stmt(stmt);
            }
            let ret_val = None;
            self.finish_block(Terminator::Return(ret_val));
            funcs.push(IrFunc {
                name:   "__main".into(),
                params: Vec::new(),
                ret:    IrTy::Void,
                blocks: std::mem::take(&mut self.blocks),
            });
        }

        IrModule { name: module_name.to_string(), funcs }
    }

    // ── Items ─────────────────────────────────────────────────

    fn lower_item(&mut self, item: &Item) -> Option<IrFunc> {
        match &item.kind {
            ItemKind::Fn(fn_def) => Some(self.lower_fn(fn_def)),
            _ => None,
        }
    }

    fn lower_fn(&mut self, fn_def: &FnDef) -> IrFunc {
        self.reset_fn(&fn_def.name);

        let params: Vec<IrParam> = fn_def.params.iter().map(|p| IrParam {
            name: p.name.clone(),
            ty:   lower_ty(&p.ty),
            mode: lower_mode(&p.mode),
        }).collect();

        let ret = lower_ty(&fn_def.ret);

        for stmt in &fn_def.body {
            self.lower_stmt(stmt);
        }

        // Ensure every function ends with a terminator.
        if self.instrs.iter().all(|_| true) {
            self.finish_block(Terminator::Return(None));
        }

        IrFunc {
            name: fn_def.name.clone(),
            params,
            ret,
            blocks: std::mem::take(&mut self.blocks),
        }
    }

    // ── Statements ────────────────────────────────────────────

    fn lower_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let { name, init, .. } | StmtKind::Var { name, init, .. } => {
                let val = self.lower_expr(init);
                self.emit(Some(name.clone()), Op::Load(val), stmt.span);
            }

            StmtKind::Assign { target, value } => {
                let src = self.lower_expr(value);
                if let ExprKind::Ident(dest) = &target.kind {
                    self.emit(None, Op::Store { dest: dest.clone(), src }, stmt.span);
                }
            }

            StmtKind::Return(expr) => {
                let val = expr.as_ref().map(|e| self.lower_expr(e));
                // Replace current block terminator.
                self.finish_block(Terminator::Return(val));
                // Start a dead block so subsequent stmts don't crash.
                let dead = self.fresh_label();
                self.start_block(dead);
            }

            StmtKind::Defer(expr) => {
                // Defer is modeled as a no-op at IR level for now;
                // a cleanup pass would hoist it to function exit.
                self.lower_expr(expr);
            }

            StmtKind::Expr(expr) => {
                self.lower_expr(expr);
            }

            StmtKind::For { binding, iter, body } => {
                let iter_val = self.lower_expr(iter);
                let loop_label  = self.fresh_label();
                let body_label  = self.fresh_label();
                let after_label = self.fresh_label();

                self.finish_block(Terminator::Jump(loop_label.clone()));
                self.start_block(loop_label.clone());

                // Simple: emit one body block per loop iteration (linear model).
                let cond = self.fresh_name();
                self.emit(Some(cond.clone()), Op::Load(iter_val.clone()), stmt.span);
                self.finish_block(Terminator::Branch {
                    cond: cond.clone(),
                    then_block: body_label.clone(),
                    else_block: after_label.clone(),
                });

                self.start_block(body_label.clone());
                self.emit(Some(binding.clone()), Op::Load(cond), stmt.span);
                for s in body { self.lower_stmt(s); }
                self.finish_block(Terminator::Jump(loop_label));

                self.start_block(after_label);
            }

            StmtKind::While { cond, body } => {
                let cond_label  = self.fresh_label();
                let body_label  = self.fresh_label();
                let after_label = self.fresh_label();

                self.finish_block(Terminator::Jump(cond_label.clone()));
                self.start_block(cond_label.clone());
                let cond_val = self.lower_expr(cond);
                self.finish_block(Terminator::Branch {
                    cond:       cond_val,
                    then_block: body_label.clone(),
                    else_block: after_label.clone(),
                });

                self.start_block(body_label);
                for s in body { self.lower_stmt(s); }
                self.finish_block(Terminator::Jump(cond_label));

                self.start_block(after_label);
            }

            StmtKind::Loop(body) => {
                let loop_label  = self.fresh_label();
                let after_label = self.fresh_label();

                self.finish_block(Terminator::Jump(loop_label.clone()));
                self.start_block(loop_label.clone());
                for s in body { self.lower_stmt(s); }
                self.finish_block(Terminator::Jump(loop_label));

                self.start_block(after_label);
            }
        }
    }

    // ── Expressions ───────────────────────────────────────────

    /// Lower an expression; returns the SSA name holding its result.
    fn lower_expr(&mut self, expr: &Expr) -> String {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Int(v) => {
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::ConstInt(*v), span);
                dest
            }
            ExprKind::Float(v) => {
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::ConstFloat(*v), span);
                dest
            }
            ExprKind::Bool(v) => {
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::ConstBool(*v), span);
                dest
            }
            ExprKind::Str(s) => {
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::ConstStr(s.clone()), span);
                dest
            }

            ExprKind::Ident(name) => {
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::Load(name.clone()), span);
                dest
            }

            ExprKind::BinOp { op, left, right } => {
                let l = self.lower_expr(left);
                let r = self.lower_expr(right);
                let dest = self.fresh_name();
                let ir_op = match op {
                    BinOp::Add    => Op::Add(l, r),
                    BinOp::Sub    => Op::Sub(l, r),
                    BinOp::Mul    => Op::Mul(l, r),
                    BinOp::Div    => Op::Div(l, r),
                    BinOp::Rem    => Op::Rem(l, r),
                    BinOp::Eq     => Op::CmpEq(l, r),
                    BinOp::NotEq  => Op::CmpNeq(l, r),
                    BinOp::Lt     => Op::CmpLt(l, r),
                    BinOp::LtEq   => Op::CmpLe(l, r),
                    BinOp::Gt     => Op::CmpGt(l, r),
                    BinOp::GtEq   => Op::CmpGe(l, r),
                    BinOp::And    => Op::And(l, r),
                    BinOp::Or     => Op::Or(l, r),
                    BinOp::Concat => Op::Call { func: "__str_concat".into(), args: vec![l, r] },
                };
                self.emit(Some(dest.clone()), ir_op, span);
                dest
            }

            ExprKind::UnOp { op, expr: inner } => {
                let v = self.lower_expr(inner);
                let dest = self.fresh_name();
                let ir_op = match op {
                    UnOp::Not   => Op::Not(v),
                    UnOp::Neg   => Op::Neg(v),
                    UnOp::Deref => Op::Deref(v),
                };
                self.emit(Some(dest.clone()), ir_op, span);
                dest
            }

            ExprKind::Call { callee, args } => {
                let func = match &callee.kind {
                    ExprKind::Ident(n) => n.clone(),
                    ExprKind::Field { base, field } => {
                        // `obj.method(args)` — treat as `method(obj, args)` for now.
                        let base_val = self.lower_expr(base);
                        let mut arg_vals = vec![base_val];
                        for a in args { arg_vals.push(self.lower_expr(&a.expr)); }
                        let dest = self.fresh_name();
                        self.emit(Some(dest.clone()), Op::Call { func: field.clone(), args: arg_vals }, span);
                        return dest;
                    }
                    _ => "__unknown".into(),
                };
                let arg_vals: Vec<String> = args.iter().map(|a| self.lower_expr(&a.expr)).collect();
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::Call { func, args: arg_vals }, span);
                dest
            }

            ExprKind::Pipe { left, right } => {
                // Desugar: `left |> right(args)` → `right(left, args)`
                let left_val = self.lower_expr(left);
                match &right.kind {
                    ExprKind::Call { callee, args } => {
                        let func = match &callee.kind {
                            ExprKind::Ident(n) => n.clone(),
                            _ => "__unknown".into(),
                        };
                        let mut arg_vals = vec![left_val];
                        for a in args { arg_vals.push(self.lower_expr(&a.expr)); }
                        let dest = self.fresh_name();
                        self.emit(Some(dest.clone()), Op::Call { func, args: arg_vals }, span);
                        dest
                    }
                    ExprKind::Ident(name) => {
                        let dest = self.fresh_name();
                        self.emit(Some(dest.clone()), Op::Call { func: name.clone(), args: vec![left_val] }, span);
                        dest
                    }
                    _ => self.lower_expr(right),
                }
            }

            ExprKind::Field { base, field } => {
                let base_val = self.lower_expr(base);
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::Field { base: base_val, field: field.clone() }, span);
                dest
            }

            ExprKind::Index { base, index } => {
                let base_val  = self.lower_expr(base);
                let index_val = self.lower_expr(index);
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::Index { base: base_val, index: index_val }, span);
                dest
            }

            ExprKind::Ref(inner) => {
                let v = self.lower_expr(inner);
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::Ref(v), span);
                dest
            }

            ExprKind::Deref(inner) => {
                let v = self.lower_expr(inner);
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::Deref(v), span);
                dest
            }

            // Error operators: or_return — unwrap or propagate.
            ExprKind::OrReturn(inner) => {
                let val = self.lower_expr(inner);
                let err_block = self.fresh_label();
                let ok_dest   = self.fresh_name();
                self.emit(Some(ok_dest.clone()), Op::UnwrapOk { val, err_block: err_block.clone() }, span);
                // err_block: propagate the error upward.
                let cont_block = self.fresh_label();
                self.finish_block(Terminator::Jump(cont_block.clone()));
                self.start_block(err_block);
                self.finish_block(Terminator::Return(Some("__err".into())));
                self.start_block(cont_block);
                ok_dest
            }

            ExprKind::OrPanic(inner, _msg) => {
                let val = self.lower_expr(inner);
                let err_block = self.fresh_label();
                let ok_dest   = self.fresh_name();
                self.emit(Some(ok_dest.clone()), Op::UnwrapOk { val, err_block: err_block.clone() }, span);
                let cont_block = self.fresh_label();
                self.finish_block(Terminator::Jump(cont_block.clone()));
                self.start_block(err_block);
                self.finish_block(Terminator::Unreachable);
                self.start_block(cont_block);
                ok_dest
            }

            ExprKind::OrElse(inner, default) => {
                let val = self.lower_expr(inner);
                let default_block = self.fresh_label();
                let ok_dest       = self.fresh_name();
                self.emit(Some(ok_dest.clone()), Op::UnwrapOk { val, err_block: default_block.clone() }, span);
                let cont_block = self.fresh_label();
                self.finish_block(Terminator::Jump(cont_block.clone()));
                self.start_block(default_block);
                let def_val = self.lower_expr(default);
                self.emit(Some(ok_dest.clone()), Op::Load(def_val), span);
                self.finish_block(Terminator::Jump(cont_block.clone()));
                self.start_block(cont_block);
                ok_dest
            }

            ExprKind::OrZero(inner) | ExprKind::OrNil(inner) => {
                let val = self.lower_expr(inner);
                let zero_block = self.fresh_label();
                let ok_dest    = self.fresh_name();
                self.emit(Some(ok_dest.clone()), Op::UnwrapOk { val, err_block: zero_block.clone() }, span);
                let cont_block = self.fresh_label();
                self.finish_block(Terminator::Jump(cont_block.clone()));
                self.start_block(zero_block);
                self.emit(Some(ok_dest.clone()), Op::ConstInt(0), span);
                self.finish_block(Terminator::Jump(cont_block.clone()));
                self.start_block(cont_block);
                ok_dest
            }

            ExprKind::ResultCtor { variant, inner } => {
                let v = self.lower_expr(inner);
                let dest = self.fresh_name();
                let op = match variant {
                    ResultVariant::Ok  => Op::WrapOk(v),
                    ResultVariant::Err => Op::WrapErr(v),
                };
                self.emit(Some(dest.clone()), op, span);
                dest
            }

            ExprKind::Cast { expr: inner, ty } => {
                let v = self.lower_expr(inner);
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::Cast { val: v, ty: lower_ty(ty) }, span);
                dest
            }

            ExprKind::StructLit { name, fields } => {
                let field_vals: Vec<(String, String)> = fields.iter().map(|f| {
                    (f.name.clone(), self.lower_expr(&f.expr))
                }).collect();
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::StructLit { ty: name.clone(), fields: field_vals }, span);
                dest
            }

            ExprKind::Block(stmts) => {
                for s in stmts { self.lower_stmt(s); }
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::Nop, span);
                dest
            }

            ExprKind::If { cond, then, else_ } => {
                let cond_val    = self.lower_expr(cond);
                let then_label  = self.fresh_label();
                let else_label  = self.fresh_label();
                let merge_label = self.fresh_label();

                self.finish_block(Terminator::Branch {
                    cond:       cond_val,
                    then_block: then_label.clone(),
                    else_block: else_label.clone(),
                });

                self.start_block(then_label);
                let then_val = self.lower_expr(then);
                self.finish_block(Terminator::Jump(merge_label.clone()));

                self.start_block(else_label);
                let else_val = if let Some(e) = else_ { self.lower_expr(e) } else { then_val.clone() };
                self.finish_block(Terminator::Jump(merge_label.clone()));

                self.start_block(merge_label);
                let dest = self.fresh_name();
                // Phi-like: pick then_val (simplified — a real compiler would emit a phi).
                self.emit(Some(dest.clone()), Op::Load(then_val), span);
                let _ = else_val;
                dest
            }

            ExprKind::Match { subject, arms } => {
                let subj = self.lower_expr(subject);
                let merge_label = self.fresh_label();
                let result_dest = self.fresh_name();

                for (i, arm) in arms.iter().enumerate() {
                    let arm_label  = self.fresh_label();
                    let next_label = if i + 1 < arms.len() { self.fresh_label() } else { merge_label.clone() };

                    let cond = self.fresh_name();
                    // Pattern check: wildcard / else always matches.
                    let always_match = matches!(arm.pattern, rym_ast::expr::Pattern::Wildcard | rym_ast::expr::Pattern::Else);
                    if always_match {
                        self.emit(Some(cond.clone()), Op::ConstBool(true), span);
                    } else {
                        self.emit(Some(cond.clone()), Op::Load(subj.clone()), span);
                    }
                    self.finish_block(Terminator::Branch {
                        cond,
                        then_block: arm_label.clone(),
                        else_block: next_label.clone(),
                    });
                    self.start_block(arm_label);
                    let arm_val = self.lower_expr(&arm.body);
                    self.emit(Some(result_dest.clone()), Op::Load(arm_val), span);
                    self.finish_block(Terminator::Jump(merge_label.clone()));
                    if i + 1 < arms.len() {
                        self.start_block(next_label);
                    }
                }

                self.start_block(merge_label);
                result_dest
            }
        }
    }

    // ── Builder helpers ───────────────────────────────────────

    fn emit(&mut self, dest: Option<String>, op: Op, span: Span) {
        self.instrs.push(Instr { dest, op, span });
    }

    fn finish_block(&mut self, term: Terminator) {
        self.blocks.push(BasicBlock {
            label:  self.current_label.clone(),
            instrs: std::mem::take(&mut self.instrs),
            term,
        });
    }

    fn start_block(&mut self, label: String) {
        self.current_label = label;
    }

    fn fresh_name(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        format!("%{id}")
    }

    fn fresh_label(&mut self) -> String {
        let id = self.next_block;
        self.next_block += 1;
        format!("bb{id}")
    }

    fn reset_fn(&mut self, name: &str) {
        self.next_id    = 0;
        self.next_block = 0;
        self.instrs.clear();
        self.blocks.clear();
        self.current_label = "entry".into();
        let _ = name;
    }
}

impl Default for Lowerer {
    fn default() -> Self {
        Self::new()
    }
}

// ── Type / mode helpers ───────────────────────────────────────

pub fn lower_ty(ty: &Ty) -> IrTy {
    match &ty.kind {
        TyKind::I8    => IrTy::I8,
        TyKind::I16   => IrTy::I16,
        TyKind::I32   => IrTy::I32,
        TyKind::I64   => IrTy::I64,
        TyKind::U8    => IrTy::U8,
        TyKind::U16   => IrTy::U16,
        TyKind::U32   => IrTy::U32,
        TyKind::U64   => IrTy::U64,
        TyKind::F32   => IrTy::F32,
        TyKind::F64   => IrTy::F64,
        TyKind::Bool  => IrTy::Bool,
        TyKind::Usize => IrTy::Usize,
        TyKind::Str   => IrTy::Str,
        TyKind::Void  => IrTy::Void,
        TyKind::Allocator     => IrTy::Allocator,
        TyKind::Named(n)      => IrTy::Named(n.clone()),
        TyKind::Slice(t)      => IrTy::Slice(Box::new(lower_ty(t))),
        TyKind::Ptr(t)        => IrTy::Ptr(Box::new(lower_ty(t))),
        TyKind::PtrMut(t)     => IrTy::PtrMut(Box::new(lower_ty(t))),
        TyKind::Result(ok, e) => IrTy::Result(Box::new(lower_ty(ok)), Box::new(lower_ty(e))),
        TyKind::Option(t)     => IrTy::Option(Box::new(lower_ty(t))),
    }
}

pub fn lower_mode(mode: &OwnershipMode) -> IrMode {
    match mode {
        OwnershipMode::Read     => IrMode::Read,
        OwnershipMode::Mut      => IrMode::Mut,
        OwnershipMode::Move     => IrMode::Move,
        OwnershipMode::Inferred => IrMode::Read,
    }
}
