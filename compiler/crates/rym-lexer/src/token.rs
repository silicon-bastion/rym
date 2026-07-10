/// Byte-offset range within the source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }
}

/// A token together with its source location.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, start: usize, end: usize) -> Self {
        Self { kind, span: Span::new(start, end) }
    }
}

/// Every token kind in the Rym language.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ── Literals ─────────────────────────────────────────────
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),

    // ── Identifier ───────────────────────────────────────────
    Ident(String),

    // ── English keywords ─────────────────────────────────────
    Fn,
    Type,
    Enum,
    Struct,
    Match,
    If,
    Else,
    For,
    In,
    Return,
    Defer,
    /// `asm!` — inline assembly (base ring only)
    AsmBang,
    While,
    Loop,
    Break,
    Continue,
    As,
    Import,
    /// Declares a file as part of the privileged `base` ring.
    Base,
    /// Declares a file as part of the safe `safe` ring.
    Safe,

    // ── Parameter-mode keywords ──────────────────────────────
    Read,
    Mut,
    Move,

    // ── Error-handling operator keywords ─────────────────────
    OrElse,
    OrReturn,
    OrPanic,
    OrZero,
    OrNil,

    // ── English binding keywords (aliases for 定/设) ─────────
    /// `let` — immutable binding (English alias for 定)
    Let,
    /// `var` — mutable binding (English alias for 设)
    Var,

    // ── Reserved Chinese keywords ────────────────────────────
    /// 定 — immutable variable binding
    Ding,
    /// 设 — mutable variable binding
    She,

    // ── Built-in types ───────────────────────────────────────
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    F32, F64,
    BoolTy,
    Usize,
    StrTy,
    Void,

    // ── Punctuation & operators ──────────────────────────────
    /// `|>` — pipe operator (the only topology connector in the algorithm zone)
    Pipe,
    /// `->` — return-type arrow
    Arrow,
    /// `=>` — match-arm arrow
    FatArrow,
    /// `?`
    Question,
    Colon,
    /// `::`
    ColonColon,
    Comma,
    Dot,
    /// `..`
    DotDot,
    Semi,
    At,
    Star,
    Amp,
    /// `++` — string concatenation
    PlusPlus,

    // ── Arithmetic ───────────────────────────────────────────
    Plus, Minus, Slash, Percent,

    // ── Bitwise ──────────────────────────────────────────────
    /// `^` — bitwise XOR
    Caret,
    /// `~` — bitwise NOT
    Tilde,
    /// `<<` — left shift
    Shl,
    /// `>>` — right shift
    Shr,

    // ── Comparison ───────────────────────────────────────────
    Eq, NotEq, Lt, LtEq, Gt, GtEq,

    // ── Assignment ───────────────────────────────────────────
    Assign,
    PlusAssign,   // `+=`
    MinusAssign,  // `-=`
    StarAssign,   // `*=`
    SlashAssign,  // `/=`
    PercentAssign,// `%=`
    AmpAssign,    // `&=`
    BitOrAssign,  // `|=`
    CaretAssign,  // `^=`
    ShlAssign,    // `<<=`
    ShrAssign,    // `>>=`

    // ── Logical ──────────────────────────────────────────────
    And, Or, Not,

    // ── Bitwise AND / OR (single `&` / `|` in non-prefix position) ──
    BitAnd,
    BitOr,

    // ── Delimiters ───────────────────────────────────────────
    LParen, RParen,
    LBrace, RBrace,
    LBracket, RBracket,

    // ── Special ──────────────────────────────────────────────
    Eof,
    Newline,
}
