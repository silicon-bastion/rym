use rym_lexer::Span;
use crate::expr::Expr;
use crate::ty::Ty;

/// A statement — valid in both the definition zone and algorithm zone,
/// though the algorithm zone further restricts to bindings and pipe expressions.
#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    /// `定 name = expr` — immutable binding (Chinese keyword `定`)
    Let {
        name: String,
        ty:   Option<Ty>,
        init: Expr,
    },

    /// `设 name = expr` — mutable binding (Chinese keyword `设`)
    Var {
        name: String,
        ty:   Option<Ty>,
        init: Expr,
    },

    /// `return expr`
    Return(Option<Expr>),

    /// `defer expr`
    Defer(Expr),

    /// A bare expression statement, e.g. a pipeline:
    /// `conn.recv() |> parse(alloc) |> send(conn)`
    Expr(Expr),

    /// `name = expr` — assignment to a mutable binding
    Assign {
        target: Expr,
        value:  Expr,
    },

    /// `for item in collection { body }`
    For {
        binding: String,
        iter:    Expr,
        body:    Vec<Stmt>,
    },

    /// `while cond { body }`
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },

    /// `loop { body }`
    Loop(Vec<Stmt>),

    /// `break`
    Break,

    /// `continue`
    Continue,
}
