use rym_ast::{
    SourceFile,
    expr::{BinOp, Expr, ExprKind, OwnershipMode, ResultVariant, UnOp},
    item::{FnDef, Item, ItemKind},
    stmt::{Stmt, StmtKind},
    ty::{Ty, TyKind},
};
use rym_lexer::Span;
use crate::{BasicBlock, EnumLayout, Instr, IrFunc, IrMode, IrModule, IrParam, IrTy, Op, StructLayout, Terminator};

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
    /// Enum layouts collected from the AST (variant name → tag index).
    enum_layouts: Vec<EnumLayout>,
    /// Deferred expressions for the current function (LIFO at return/exit).
    deferred: Vec<Expr>,
    /// Variable names whose type is a slice ([]T) — used to pick SliceLen vs StrLen.
    slice_vars: std::collections::HashSet<String>,
    /// Variable names whose type is a function pointer — used to emit CallIndirect.
    fn_ptr_vars: std::collections::HashSet<String>,
}

impl Lowerer {
    pub fn new() -> Self {
        Self {
            next_id:       0,
            next_block:    0,
            instrs:        Vec::new(),
            blocks:        Vec::new(),
            current_label: "entry".into(),
            enum_layouts:  Vec::new(),
            deferred:      Vec::new(),
            slice_vars:    std::collections::HashSet::new(),
            fn_ptr_vars:   std::collections::HashSet::new(),
        }
    }

    /// Lower a full source file.
    pub fn lower_file(&mut self, file: &SourceFile, module_name: &str) -> IrModule {
        let mut funcs = Vec::new();
        let mut structs = Vec::new();

        for item in &file.def_zone {
            match &item.kind {
                ItemKind::Type(type_def) => {
                    structs.push(StructLayout {
                        name:   type_def.name.clone(),
                        fields: type_def.fields.iter().map(|f| f.name.clone()).collect(),
                    });
                }
                ItemKind::Enum(enum_def) => {
                    let layout = EnumLayout {
                        name:     enum_def.name.clone(),
                        variants: enum_def.variants.iter().map(|v| v.name.clone()).collect(),
                    };
                    self.enum_layouts.push(layout.clone());
                    structs.push(StructLayout {
                        name:   enum_def.name.clone(),
                        fields: vec!["__tag".into(), "__payload".into()],
                    });
                }
                _ => {}
            }
            if let Some(f) = self.lower_item(item) {
                funcs.push(f);
            }
        }

        let enums = self.enum_layouts.clone();

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

        IrModule { name: module_name.to_string(), funcs, structs, enums }
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

        let params: Vec<IrParam> = fn_def.params.iter().map(|p| {
            if matches!(p.ty.kind, TyKind::Slice(_)) {
                self.slice_vars.insert(p.name.clone());
            }
            if matches!(p.ty.kind, TyKind::FnPtr { .. }) {
                self.fn_ptr_vars.insert(p.name.clone());
            }
            IrParam {
                name: p.name.clone(),
                ty:   lower_ty(&p.ty),
                mode: lower_mode(&p.mode),
            }
        }).collect();

        let ret = lower_ty(&fn_def.ret);

        for stmt in &fn_def.body {
            self.lower_stmt(stmt);
        }

        // Flush any remaining deferred expressions at implicit function exit.
        let end_span = fn_def.body.last()
            .map(|s| s.span)
            .unwrap_or(rym_lexer::Span::new(0, 0));
        self.flush_deferred(end_span);

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
            StmtKind::Let { name, ty, init } | StmtKind::Var { name, ty, init } => {
                if let Some(t) = ty {
                    if matches!(t.kind, TyKind::Slice(_)) {
                        self.slice_vars.insert(name.clone());
                    }
                    if matches!(t.kind, TyKind::FnPtr { .. }) {
                        self.fn_ptr_vars.insert(name.clone());
                    }
                } else if matches!(init.kind, ExprKind::ArrayLit(_)) {
                    self.slice_vars.insert(name.clone());
                }
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
                // Run deferred expressions (LIFO) before returning.
                self.flush_deferred(stmt.span);
                self.finish_block(Terminator::Return(val));
                // Start a dead block so subsequent stmts don't crash.
                let dead = self.fresh_label();
                self.start_block(dead);
            }

            StmtKind::Defer(expr) => {
                // Queue for execution at function exit (LIFO order).
                self.deferred.push(expr.clone());
            }

            StmtKind::Expr(expr) => {
                self.lower_expr(expr);
            }

            StmtKind::For { binding, iter, body } => {
                // Lower: `for x in slice { body }`
                //
                //   i = 0
                //   len = SliceLen(slice)
                //   jump cond_block
                // cond_block:
                //   in_bounds = i < len
                //   branch in_bounds → body_block, after_block
                // body_block:
                //   x = slice[i]
                //   <body>
                //   i = i + 1
                //   jump cond_block
                // after_block:

                let slice_val   = self.lower_expr(iter);
                let cond_label  = self.fresh_label();
                let body_label  = self.fresh_label();
                let after_label = self.fresh_label();

                // Counter variable — use a unique SSA name scoped to this loop.
                let i_var = self.fresh_name();
                let zero  = self.fresh_name();
                self.emit(Some(zero.clone()),  Op::ConstInt(0), stmt.span);
                self.emit(Some(i_var.clone()), Op::Load(zero),  stmt.span);

                // len = slice.len
                let len_val = self.fresh_name();
                self.emit(Some(len_val.clone()), Op::SliceLen(slice_val.clone()), stmt.span);

                self.finish_block(Terminator::Jump(cond_label.clone()));
                self.start_block(cond_label.clone());

                // Reload i (mutable across iterations).
                let i_cur    = self.fresh_name();
                let len_cur  = self.fresh_name();
                self.emit(Some(i_cur.clone()),   Op::Load(i_var.clone()),  stmt.span);
                self.emit(Some(len_cur.clone()), Op::Load(len_val.clone()), stmt.span);
                let in_bounds = self.fresh_name();
                self.emit(Some(in_bounds.clone()), Op::CmpLt(i_cur.clone(), len_cur), stmt.span);
                self.finish_block(Terminator::Branch {
                    cond:       in_bounds,
                    then_block: body_label.clone(),
                    else_block: after_label.clone(),
                });

                self.start_block(body_label);
                // elem = slice_data_ptr[i]  (dereference through the fat-pointer struct)
                let data_ptr = self.fresh_name();
                self.emit(Some(data_ptr.clone()), Op::SlicePtr(slice_val.clone()), stmt.span);
                let elem = self.fresh_name();
                self.emit(Some(elem.clone()), Op::Index { base: data_ptr, index: i_cur.clone() }, stmt.span);
                self.emit(Some(binding.clone()), Op::Load(elem), stmt.span);

                for s in body { self.lower_stmt(s); }

                // i = i + 1
                let one    = self.fresh_name();
                let i_next = self.fresh_name();
                self.emit(Some(one.clone()),    Op::ConstInt(1),                    stmt.span);
                self.emit(Some(i_next.clone()), Op::Add(i_cur.clone(), one),        stmt.span);
                self.emit(None,                 Op::Store { dest: i_var.clone(), src: i_next }, stmt.span);

                self.finish_block(Terminator::Jump(cond_label));
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
                    ExprKind::Ident(n) => {
                        // If the callee name is a known fn-ptr variable, emit CallIndirect.
                        if self.fn_ptr_vars.contains(n) {
                            let fp_val = self.lower_expr(callee);
                            let arg_vals: Vec<String> = args.iter().map(|a| self.lower_expr(&a.expr)).collect();
                            let dest = self.fresh_name();
                            self.emit(Some(dest.clone()), Op::CallIndirect { fp: fp_val, args: arg_vals }, span);
                            return dest;
                        }
                        n.clone()
                    }
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
                // Check if this is an enum variant expression: `EnumName.Variant`.
                if let ExprKind::Ident(type_name) = &base.kind {
                    if let Some(layout) = self.enum_layouts.iter().find(|e| &e.name == type_name) {
                        if let Some(tag) = layout.tag_of(field) {
                            // No-payload variant — payload is 0.
                            let zero = self.fresh_name();
                            let dest = self.fresh_name();
                            self.emit(Some(zero.clone()), Op::ConstInt(0), span);
                            self.emit(Some(dest.clone()), Op::MakeVariant { tag, payload: zero }, span);
                            return dest;
                        }
                    }
                }
                // Determine if base refers to a slice variable.
                let base_is_slice = if let ExprKind::Ident(name) = &base.kind {
                    self.slice_vars.contains(name)
                } else {
                    // ArrayLit expressions are always slices.
                    matches!(base.kind, ExprKind::ArrayLit(_))
                };
                let base_val = self.lower_expr(base);
                let dest = self.fresh_name();
                match field.as_str() {
                    "len" if base_is_slice => {
                        self.emit(Some(dest.clone()), Op::SliceLen(base_val), span);
                    }
                    "len" => {
                        self.emit(Some(dest.clone()), Op::StrLen(base_val), span);
                    }
                    "ptr" => {
                        self.emit(Some(dest.clone()), Op::SlicePtr(base_val), span);
                    }
                    _ => {
                        self.emit(Some(dest.clone()), Op::Field { base: base_val, field: field.clone(), struct_ty: None }, span);
                    }
                }
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

            // Error operators: or_return — unwrap Ok payload or propagate Err.
            ExprKind::OrReturn(inner) => {
                let val       = self.lower_expr(inner);
                let tag       = self.fresh_name();
                let ok_dest   = self.fresh_name();
                let ok_block  = self.fresh_label();
                let err_block = self.fresh_label();
                let cont      = self.fresh_label();
                self.emit(Some(tag.clone()), Op::GetTag(val.clone()), span);
                // tag == 0 → Ok branch
                let zero = self.fresh_name();
                self.emit(Some(zero.clone()), Op::ConstInt(0), span);
                let is_ok = self.fresh_name();
                self.emit(Some(is_ok.clone()), Op::CmpEq(tag, zero), span);
                self.finish_block(Terminator::Branch { cond: is_ok, then_block: ok_block.clone(), else_block: err_block.clone() });
                self.start_block(ok_block);
                self.emit(Some(ok_dest.clone()), Op::GetPayload(val.clone()), span);
                self.finish_block(Terminator::Jump(cont.clone()));
                self.start_block(err_block);
                // Propagate the whole Result value upward.
                self.finish_block(Terminator::Return(Some(val)));
                self.start_block(cont);
                ok_dest
            }

            ExprKind::OrPanic(inner, _msg) => {
                let val       = self.lower_expr(inner);
                let tag       = self.fresh_name();
                let ok_dest   = self.fresh_name();
                let ok_block  = self.fresh_label();
                let err_block = self.fresh_label();
                let cont      = self.fresh_label();
                self.emit(Some(tag.clone()), Op::GetTag(val.clone()), span);
                let zero = self.fresh_name();
                self.emit(Some(zero.clone()), Op::ConstInt(0), span);
                let is_ok = self.fresh_name();
                self.emit(Some(is_ok.clone()), Op::CmpEq(tag, zero), span);
                self.finish_block(Terminator::Branch { cond: is_ok, then_block: ok_block.clone(), else_block: err_block.clone() });
                self.start_block(ok_block);
                self.emit(Some(ok_dest.clone()), Op::GetPayload(val), span);
                self.finish_block(Terminator::Jump(cont.clone()));
                self.start_block(err_block);
                self.finish_block(Terminator::Unreachable);
                self.start_block(cont);
                ok_dest
            }

            ExprKind::OrElse(inner, default) => {
                let val          = self.lower_expr(inner);
                let tag          = self.fresh_name();
                let ok_dest      = self.fresh_name();
                let ok_block     = self.fresh_label();
                let default_block = self.fresh_label();
                let cont         = self.fresh_label();
                self.emit(Some(tag.clone()), Op::GetTag(val.clone()), span);
                let zero = self.fresh_name();
                self.emit(Some(zero.clone()), Op::ConstInt(0), span);
                let is_ok = self.fresh_name();
                self.emit(Some(is_ok.clone()), Op::CmpEq(tag, zero), span);
                self.finish_block(Terminator::Branch { cond: is_ok, then_block: ok_block.clone(), else_block: default_block.clone() });
                self.start_block(ok_block);
                self.emit(Some(ok_dest.clone()), Op::GetPayload(val), span);
                self.finish_block(Terminator::Jump(cont.clone()));
                self.start_block(default_block);
                let def_val = self.lower_expr(default);
                self.emit(Some(ok_dest.clone()), Op::Load(def_val), span);
                self.finish_block(Terminator::Jump(cont.clone()));
                self.start_block(cont);
                ok_dest
            }

            ExprKind::OrZero(inner) | ExprKind::OrNil(inner) => {
                let val       = self.lower_expr(inner);
                let tag       = self.fresh_name();
                let ok_dest   = self.fresh_name();
                let ok_block  = self.fresh_label();
                let zero_block = self.fresh_label();
                let cont      = self.fresh_label();
                self.emit(Some(tag.clone()), Op::GetTag(val.clone()), span);
                let zero = self.fresh_name();
                self.emit(Some(zero.clone()), Op::ConstInt(0), span);
                let is_ok = self.fresh_name();
                self.emit(Some(is_ok.clone()), Op::CmpEq(tag, zero), span);
                self.finish_block(Terminator::Branch { cond: is_ok, then_block: ok_block.clone(), else_block: zero_block.clone() });
                self.start_block(ok_block);
                self.emit(Some(ok_dest.clone()), Op::GetPayload(val), span);
                self.finish_block(Terminator::Jump(cont.clone()));
                self.start_block(zero_block);
                self.emit(Some(ok_dest.clone()), Op::ConstInt(0), span);
                self.finish_block(Terminator::Jump(cont.clone()));
                self.start_block(cont);
                ok_dest
            }

            ExprKind::ResultCtor { variant, inner } => {
                let v = self.lower_expr(inner);
                let dest = self.fresh_name();
                let tag = match variant { ResultVariant::Ok => 0, ResultVariant::Err => 1 };
                self.emit(Some(dest.clone()), Op::MakeVariant { tag, payload: v }, span);
                dest
            }

            ExprKind::Cast { expr: inner, ty } => {
                let v = self.lower_expr(inner);
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::Cast { val: v, ty: lower_ty(ty) }, span);
                dest
            }

            ExprKind::ArrayLit(elems) => {
                let elem_vals: Vec<String> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::ArrayLit(elem_vals), span);
                dest
            }

            ExprKind::MatrixLit { elems, rows, cols } => {
                let elem_vals: Vec<String> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::MatrixLit { elems: elem_vals, rows: *rows, cols: *cols }, span);
                dest
            }

            ExprKind::AllocCall { allocator, elem_ty, count } => {
                let alloc_val = self.lower_expr(allocator);
                let count_val = self.lower_expr(count);
                let dest = self.fresh_name();
                self.emit(Some(dest.clone()), Op::AllocCall {
                    allocator: alloc_val,
                    elem_ty: lower_ty(elem_ty),
                    count: count_val,
                }, span);
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
                let subj        = self.lower_expr(subject);
                let merge_label = self.fresh_label();
                let result_dest = self.fresh_name();

                // Extract tag once for all arms.
                let tag_val = self.fresh_name();
                self.emit(Some(tag_val.clone()), Op::GetTag(subj.clone()), span);

                // For each arm allocate a separate body block and a separate
                // "next dispatch" block. These must be distinct: the "else" target
                // of arm i's dispatch check is arm i+1's DISPATCH block, not its
                // body block. Using the same block for both causes duplicate labels.
                let body_labels: Vec<String> = (0..arms.len()).map(|_| self.fresh_label()).collect();
                // next_dispatch[i] = where to go when arm i doesn't match.
                // For the last arm this is the merge block.
                let next_dispatch: Vec<String> = (0..arms.len()).map(|i| {
                    if i + 1 < arms.len() { self.fresh_label() } else { merge_label.clone() }
                }).collect();

                for (i, arm) in arms.iter().enumerate() {
                    use rym_ast::expr::Pattern;
                    let body_label = body_labels[i].clone();
                    let else_label = next_dispatch[i].clone();

                    match &arm.pattern {
                        Pattern::Wildcard | Pattern::Else => {
                            // Always matches — jump directly to arm body.
                            self.finish_block(Terminator::Jump(body_label.clone()));
                        }
                        Pattern::Variant { ty: enum_ty, variant } => {
                            let tag_idx = self.enum_layouts.iter()
                                .find(|e| &e.name == enum_ty)
                                .and_then(|e| e.tag_of(variant))
                                .or_else(|| self.enum_layouts.iter().find_map(|e| e.tag_of(variant)))
                                .unwrap_or(0);
                            let expected = self.fresh_name();
                            self.emit(Some(expected.clone()), Op::ConstInt(tag_idx as i64), span);
                            let is_match = self.fresh_name();
                            self.emit(Some(is_match.clone()), Op::CmpEq(tag_val.clone(), expected), span);
                            self.finish_block(Terminator::Branch {
                                cond: is_match,
                                then_block: body_label.clone(),
                                else_block: else_label.clone(),
                            });
                        }
                        Pattern::Lit(lit_kind) => {
                            let lit_expr = rym_ast::expr::Expr { kind: lit_kind.clone(), span };
                            let lit_val  = self.lower_expr(&lit_expr);
                            let is_match = self.fresh_name();
                            self.emit(Some(is_match.clone()), Op::CmpEq(subj.clone(), lit_val), span);
                            self.finish_block(Terminator::Branch {
                                cond: is_match,
                                then_block: body_label.clone(),
                                else_block: else_label.clone(),
                            });
                        }
                    }

                    // Arm body block.
                    self.start_block(body_label);
                    if matches!(&arm.pattern, Pattern::Variant { .. }) {
                        let payload = self.fresh_name();
                        self.emit(Some(payload.clone()), Op::GetPayload(subj.clone()), span);
                        self.emit(Some("__payload".into()), Op::Load(payload), span);
                    }
                    let arm_val = self.lower_expr(&arm.body);
                    self.emit(Some(result_dest.clone()), Op::Load(arm_val), span);
                    self.finish_block(Terminator::Jump(merge_label.clone()));

                    // Start the NEXT arm's dispatch block (different from the body block).
                    if i + 1 < arms.len() {
                        self.start_block(next_dispatch[i].clone());
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
        self.deferred.clear();
        self.slice_vars.clear();
        self.fn_ptr_vars.clear();
        self.current_label = "entry".into();
        let _ = name;
    }

    /// Lower all deferred expressions in LIFO order (last deferred runs first).
    fn flush_deferred(&mut self, _span: Span) {
        let exprs: Vec<Expr> = self.deferred.drain(..).rev().collect();
        for e in exprs {
            self.lower_expr(&e);
        }
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
        TyKind::Array { size, elem } => IrTy::Array { size: *size, elem: Box::new(lower_ty(elem)) },
        TyKind::FnPtr { params, ret } => IrTy::FnPtr {
            params: params.iter().map(lower_ty).collect(),
            ret:    Box::new(lower_ty(ret)),
        },
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
