use rym_lexer::Span;
use rym_ast::expr::{Expr, ExprKind, OwnershipMode};
use crate::error::SemaError;
use crate::scope::Scope;

/// Verify ownership rules for a pipe chain.
///
/// Rule: if the left side of a `|>` transfers ownership (`move`), the first
/// parameter of the right side must declare `move`.
pub fn check_pipe(
    left: &Expr,
    right: &Expr,
    scope: &Scope,
    span: Span,
) -> Result<(), SemaError> {
    let left_mode = infer_expr_mode(left, scope);
    let right_first_mode = right_first_param_mode(right);

    if left_mode == OwnershipMode::Move && right_first_mode != OwnershipMode::Move {
        return Err(SemaError::PipeOwnershipConflict { span });
    }
    Ok(())
}

/// Verify that a `move` expression does not use an already-moved binding.
pub fn check_move(name: &str, scope: &Scope, span: Span) -> Result<(), SemaError> {
    if let Some(b) = scope.lookup(name) {
        if b.moved {
            return Err(SemaError::UseAfterMove { name: name.to_string(), span });
        }
        if b.mode == OwnershipMode::Read {
            return Err(SemaError::MoveOfReadBinding { name: name.to_string(), span });
        }
    }
    Ok(())
}

/// Verify that a mutation target is a mutable binding.
pub fn check_mutate(name: &str, scope: &Scope, span: Span) -> Result<(), SemaError> {
    if let Some(b) = scope.lookup(name) {
        if !b.mutable {
            return Err(SemaError::MutateImmutable { name: name.to_string(), span });
        }
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────

fn infer_expr_mode(expr: &Expr, scope: &Scope) -> OwnershipMode {
    match &expr.kind {
        ExprKind::Ident(name) => {
            scope.lookup(name)
                .map(|b| b.mode.clone())
                .unwrap_or(OwnershipMode::Inferred)
        }
        // A call that moves its callee propagates move.
        ExprKind::Call { args, .. } => {
            if args.iter().any(|a| a.mode == OwnershipMode::Move) {
                OwnershipMode::Move
            } else {
                OwnershipMode::Read
            }
        }
        _ => OwnershipMode::Read,
    }
}

fn right_first_param_mode(expr: &Expr) -> OwnershipMode {
    match &expr.kind {
        ExprKind::Call { args, .. } => {
            args.first()
                .map(|a| a.mode.clone())
                .unwrap_or(OwnershipMode::Inferred)
        }
        ExprKind::Ident(_) => OwnershipMode::Inferred,
        _ => OwnershipMode::Inferred,
    }
}
