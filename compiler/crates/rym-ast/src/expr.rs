use rym_lexer::Span;
use crate::ty::Ty;

/// An expression node.
#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    // ── Literals ─────────────────────────────────────────────
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),

    /// Variable reference: `x`, `解析请求`
    Ident(String),

    // ── Ownership-aware function call ─────────────────────────
    /// `f(arg1, arg2)` — arguments carry explicit ownership mode
    Call {
        callee: Box<Expr>,
        args:   Vec<Arg>,
    },

    /// `recv |> parse(alloc)` — pipe operator
    /// Desugars to: `parse(recv, alloc)`
    Pipe {
        left:  Box<Expr>,
        right: Box<Expr>,
    },

    /// Field access: `conn.fd`, `请求.路径`
    Field {
        base:  Box<Expr>,
        field: String,
    },

    /// Index: `buf[i]`
    Index {
        base:  Box<Expr>,
        index: Box<Expr>,
    },

    // ── Error-handling operators ──────────────────────────────
    /// `expr.or_return?`
    OrReturn(Box<Expr>),
    /// `expr.or_panic("msg")`
    OrPanic(Box<Expr>, String),
    /// `expr.or_else(default)`
    OrElse(Box<Expr>, Box<Expr>),
    /// `expr.or_zero`
    OrZero(Box<Expr>),
    /// `expr.or_nil`
    OrNil(Box<Expr>),

    // ── Arithmetic & logic ────────────────────────────────────
    BinOp {
        op:    BinOp,
        left:  Box<Expr>,
        right: Box<Expr>,
    },
    UnOp {
        op:   UnOp,
        expr: Box<Expr>,
    },

    // ── Struct construction ───────────────────────────────────
    /// `Request{ path=path, method=method }` / `请求{ 路径=路径 }`
    StructLit {
        name:   String,
        fields: Vec<FieldInit>,
    },

    /// `Ok(expr)` / `Err(expr)`
    ResultCtor {
        variant: ResultVariant,
        inner:   Box<Expr>,
    },

    /// Array literal: `[1, 2, 3]`
    ArrayLit(Vec<Expr>),

    /// Matrix literal: `[1,2;3,4]` — `;` separates rows, `,` separates columns.
    /// Stored as flat row-major Vec with explicit row count.
    MatrixLit { elems: Vec<Expr>, rows: usize, cols: usize },

    /// Allocation call: `alloc.alloc(T, n)` — desugared from method syntax.
    AllocCall { allocator: Box<Expr>, elem_ty: Ty, count: Box<Expr> },

    /// Inline assembly: `asm!("template", arg0, arg1, ...)` — base ring only.
    Asm { template: String, args: Vec<Expr> },

    /// Type cast: `x as u64`
    Cast {
        expr: Box<Expr>,
        ty:   Ty,
    },

    /// Address-of: `&x`
    Ref(Box<Expr>),

    /// Dereference: `*x`
    Deref(Box<Expr>),

    /// Block expression used only inside `if`/`match` arms.
    /// The flat-zone rule prohibits stand-alone nested blocks.
    Block(Vec<crate::stmt::Stmt>),

    /// `if cond { then } else { else_ }`
    If {
        cond:  Box<Expr>,
        then:  Box<Expr>,
        else_: Option<Box<Expr>>,
    },

    /// `match expr { pattern => expr, ... }`
    Match {
        subject: Box<Expr>,
        arms:    Vec<MatchArm>,
    },
}

/// An argument in a function call, with its ownership mode.
#[derive(Debug, Clone)]
pub struct Arg {
    /// Optional keyword label: `前缀: "Mr."`
    pub label: Option<String>,
    pub mode:  OwnershipMode,
    pub expr:  Expr,
}

/// Ownership mode for a parameter or argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipMode {
    /// `read` — shared borrow, unlimited aliasing
    Read,
    /// `mut` — exclusive borrow, no aliasing
    Mut,
    /// `move` — ownership transfer, source is consumed
    Move,
    /// Inferred (before sema pass)
    Inferred,
}

#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: String,
    pub expr: Expr,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body:    Expr,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    /// Wildcard `_`
    Wildcard,
    /// Literal value
    Lit(ExprKind),
    /// Enum variant: `IO结果.成功`
    Variant { ty: String, variant: String },
    /// Catch-all `else`
    Else,
}

#[derive(Debug, Clone)]
pub enum ResultVariant {
    Ok,
    Err,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Rem,
    Eq, NotEq, Lt, LtEq, Gt, GtEq,
    And, Or,
    Concat, // `++` string concatenation
    BitAnd, BitOr, BitXor, Shl, Shr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot, // `~`
    Deref,
}
