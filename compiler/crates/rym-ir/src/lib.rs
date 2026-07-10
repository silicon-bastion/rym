pub mod lower;

use rym_lexer::Span;

/// A complete IR program — one module per source file.
#[derive(Debug, Clone)]
pub struct IrModule {
    pub name:    String,
    pub funcs:   Vec<IrFunc>,
    /// Struct type layouts: type name → ordered field names.
    pub structs: Vec<StructLayout>,
    /// Enum variant layouts: enum name → ordered variant names.
    pub enums:   Vec<EnumLayout>,
}

/// Layout of a struct type used for field-offset calculation.
#[derive(Debug, Clone)]
pub struct StructLayout {
    pub name:   String,
    pub fields: Vec<String>,
}

/// Enum variant layout: variant name → tag index.
#[derive(Debug, Clone)]
pub struct EnumLayout {
    pub name:     String,
    /// Ordered variant names; index == tag value.
    pub variants: Vec<String>,
}

impl EnumLayout {
    pub fn tag_of(&self, variant: &str) -> Option<usize> {
        self.variants.iter().position(|v| v == variant)
    }
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

    // ── Bitwise ──────────────────────────────────────────────
    BitAnd(String, String),
    BitOr(String, String),
    BitXor(String, String),
    Shl(String, String),
    Shr(String, String),
    BitNot(String),

    // ── Memory ───────────────────────────────────────────────
    /// Load a named SSA value.
    Load(String),
    /// Store `src` into mutable slot `dest`.
    Store { dest: String, src: String },
    /// Address-of: `&x`
    Ref(String),
    /// Dereference pointer: `*x`
    Deref(String),
    /// Field access: `base.field` — `struct_ty` is the resolved struct type name.
    Field { base: String, field: String, struct_ty: Option<String> },
    /// Slice index: `base[index]`
    Index { base: String, index: String },
    /// Number of elements in a slice: `base.len`
    SliceLen(String),
    /// Raw data pointer of a slice: `base.ptr`
    SlicePtr(String),
    /// Byte length of a str (via strlen): `base.len`
    StrLen(String),

    // ── Calls ─────────────────────────────────────────────────
    Call { func: String, args: Vec<String> },
    /// Indirect call through a function pointer variable.
    CallIndirect { fp: String, args: Vec<String> },

    // ── Enum / tagged-union construction ─────────────────────
    /// Build an enum value: `{ tag, payload }` where tag is the variant index.
    MakeVariant { tag: usize, payload: String },
    /// Extract the tag index from an enum value.
    GetTag(String),
    /// Extract the payload from an enum value.
    GetPayload(String),

    // ── Result / error handling ───────────────────────────────
    /// Wrap a value as Ok (tag=0).
    WrapOk(String),
    /// Wrap a value as Err (tag=1).
    WrapErr(String),
    /// Extract Ok value or jump to `err_block` on Err.
    UnwrapOk { val: String, err_block: String },

    // ── Type cast ─────────────────────────────────────────────
    Cast { val: String, ty: IrTy },

    // ── Struct construction ────────────────────────────────────
    StructLit { ty: String, fields: Vec<(String, String)> },

    // ── Array / matrix construction ────────────────────────────
    /// Stack array literal: `[a, b, c]` — elems are SSA names.
    ArrayLit(Vec<String>),
    /// Matrix literal (row-major): `[1,2;3,4]`.
    MatrixLit { elems: Vec<String>, rows: usize, cols: usize },

    // ── Allocator ──────────────────────────────────────────────
    /// `alloc.alloc(T, count)` — returns pointer to heap memory.
    AllocCall { allocator: String, elem_ty: IrTy, count: String },

    /// Inline assembly: `asm!("template", arg0, arg1, ...)` — base ring only.
    /// `{0}`, `{1}`, … in the template are replaced by the corresponding arg SSA names.
    Asm { template: String, args: Vec<String> },

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
    Array { size: usize, elem: Box<IrTy> },
    /// Function pointer — represented as uintptr_t at IR level.
    FnPtr { params: Vec<IrTy>, ret: Box<IrTy> },
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
            IrTy::Array { size, elem } => format!("[{}]{}", size, elem.display()),
            IrTy::FnPtr { params, ret } => {
                let ps = params.iter().map(|p| p.display()).collect::<Vec<_>>().join(", ");
                format!("fn({}) -> {}", ps, ret.display())
            }
        }
    }
}
