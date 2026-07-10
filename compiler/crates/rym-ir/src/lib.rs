pub mod lower;

use rym_lexer::Span;

/// A complete IR program — one module per source file.
#[derive(Debug, Clone)]
pub struct IrModule {
    pub name:  String,
    pub funcs: Vec<IrFunc>,
}

/// A function in IR form.
#[derive(Debug, Clone)]
pub struct IrFunc {
    pub name:   String,
    pub params: Vec<IrParam>,
    pub ret:    IrTy,
    pub blocks: Vec<BasicBlock>,
}

/// A function parameter.
#[derive(Debug, Clone)]
pub struct IrParam {
    pub name: String,
    pub ty:   IrTy,
    pub mode: IrMode,
}

/// Ownership mode in IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrMode {
    Read,
    Mut,
    Move,
}

/// A basic block: linear sequence of instructions with a terminator.
#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub label:  String,
    pub instrs: Vec<Instr>,
    pub term:   Terminator,
}

/// An IR instruction (three-address code style).
#[derive(Debug, Clone)]
pub struct Instr {
    /// Optional SSA destination name.
    pub dest: Option<String>,
    pub op:   Op,
    pub span: Span,
}

/// IR operations.
#[derive(Debug, Clone)]
pub enum Op {
    // ── Literals ─────────────────────────────────────────────
    ConstInt(i64),
    ConstFloat(f64),
    ConstBool(bool),
    ConstStr(String),

    // ── Arithmetic ───────────────────────────────────────────
    Add(String, String),
    Sub(String, String),
    Mul(String, String),
    Div(String, String),
    Rem(String, String),

    // ── Comparison ───────────────────────────────────────────
    CmpEq(String, String),
    CmpNeq(String, String),
    CmpLt(String, String),
    CmpLe(String, String),
    CmpGt(String, String),
    CmpGe(String, String),

    // ── Logical ──────────────────────────────────────────────
    And(String, String),
    Or(String, String),
    Not(String),
    Neg(String),

    // ── Memory ───────────────────────────────────────────────
    /// Load a named SSA value.
    Load(String),
    /// Store `src` into mutable slot `dest`.
    Store { dest: String, src: String },
    /// Address-of: `&x`
    Ref(String),
    /// Dereference pointer: `*x`
    Deref(String),
    /// Field access: `base.field`
    Field { base: String, field: String },
    /// Slice index: `base[index]`
    Index { base: String, index: String },

    // ── Calls ─────────────────────────────────────────────────
    Call { func: String, args: Vec<String> },

    // ── Result / error handling ───────────────────────────────
    WrapOk(String),
    WrapErr(String),
    /// Extract Ok value or jump to `err_block` on Err.
    UnwrapOk { val: String, err_block: String },

    // ── Type cast ─────────────────────────────────────────────
    Cast { val: String, ty: IrTy },

    // ── Struct construction ────────────────────────────────────
    StructLit { ty: String, fields: Vec<(String, String)> },

    Nop,
}

/// Terminator — exactly one per basic block.
#[derive(Debug, Clone)]
pub enum Terminator {
    Jump(String),
    Branch { cond: String, then_block: String, else_block: String },
    Return(Option<String>),
    Unreachable,
}

/// IR type system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrTy {
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    F32, F64,
    Bool,
    Usize,
    Str,
    Void,
    Ptr(Box<IrTy>),
    PtrMut(Box<IrTy>),
    Slice(Box<IrTy>),
    Result(Box<IrTy>, Box<IrTy>),
    Option(Box<IrTy>),
    Named(String),
    Allocator,
}

impl IrTy {
    pub fn display(&self) -> String {
        match self {
            IrTy::I8    => "i8".into(),
            IrTy::I16   => "i16".into(),
            IrTy::I32   => "i32".into(),
            IrTy::I64   => "i64".into(),
            IrTy::U8    => "u8".into(),
            IrTy::U16   => "u16".into(),
            IrTy::U32   => "u32".into(),
            IrTy::U64   => "u64".into(),
            IrTy::F32   => "f32".into(),
            IrTy::F64   => "f64".into(),
            IrTy::Bool  => "bool".into(),
            IrTy::Usize => "usize".into(),
            IrTy::Str   => "str".into(),
            IrTy::Void  => "void".into(),
            IrTy::Allocator     => "Allocator".into(),
            IrTy::Named(n)      => n.clone(),
            IrTy::Ptr(t)        => format!("*{}", t.display()),
            IrTy::PtrMut(t)     => format!("*mut {}", t.display()),
            IrTy::Slice(t)      => format!("[]{}", t.display()),
            IrTy::Result(ok, e) => format!("Result({}, {})", ok.display(), e.display()),
            IrTy::Option(t)     => format!("Option({})", t.display()),
        }
    }
}
