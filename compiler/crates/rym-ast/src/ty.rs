use rym_lexer::Span;

/// A type expression.
#[derive(Debug, Clone)]
pub struct Ty {
    pub kind: TyKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TyKind {
    // ── Primitives ────────────────────────────────────────────
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    F32, F64,
    Bool,
    Usize,
    Str,
    Void,

    /// Named type: `Request`, `请求`, etc.
    Named(String),

    /// Slice: `[]u8`, `[]Request`
    Slice(Box<Ty>),

    /// Raw pointer: `*u8`
    Ptr(Box<Ty>),

    /// Mutable raw pointer: `*mut u8`
    PtrMut(Box<Ty>),

    /// `Result(T, E)`
    Result(Box<Ty>, Box<Ty>),

    /// `Option(T)`
    Option(Box<Ty>),

    /// `Allocator` interface
    Allocator,

    /// Fixed-size stack array: `[4]i32`
    Array { size: usize, elem: Box<Ty> },

    /// Function pointer: `fn(T, U) -> R`
    FnPtr { params: Vec<Ty>, ret: Box<Ty> },
}
