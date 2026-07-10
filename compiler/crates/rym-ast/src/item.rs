use rym_lexer::Span;
use crate::expr::OwnershipMode;
use crate::stmt::Stmt;
use crate::ty::Ty;

/// A top-level definition — valid only in the definition zone.
#[derive(Debug, Clone)]
pub struct Item {
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ItemKind {
    /// `fn name(params) -> ret { body }`
    Fn(FnDef),

    /// `type Name { fields }`
    Type(TypeDef),

    /// `enum Name { Variant1, Variant2(T), ... }`
    Enum(EnumDef),

    /// `import "path/to/file.rym"`
    Import(String),

    /// Compiler extension point: `@parser_rule(...)`, `@type_rule(...)`, etc.
    CompilerExt(CompilerExt),
}

// ── Function definition ───────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FnDef {
    pub name:   String,
    pub params: Vec<Param>,
    pub ret:    Ty,
    pub body:   Vec<Stmt>,
    /// Compiler extension attributes on this function.
    pub attrs:  Vec<Attr>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub mode: OwnershipMode,
    pub ty:   Ty,
}

// ── Type definition ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name:   String,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub ty:   Ty,
}

// ── Enum definition ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name:     String,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name:    String,
    /// Some if the variant carries a payload: `IOError(str)`
    pub payload: Option<Ty>,
}

// ── Compiler extension ────────────────────────────────────────

/// `@lower_pass(node = "Syscall")` style attribute on a function.
#[derive(Debug, Clone)]
pub struct Attr {
    pub name: String,
    pub args: Vec<AttrArg>,
}

#[derive(Debug, Clone)]
pub struct AttrArg {
    pub key:   String,
    pub value: String,
}

/// A compiler extension declaration in the `base` ring.
/// These map to Phase 1–6 extension points.
#[derive(Debug, Clone)]
pub struct CompilerExt {
    pub phase:   ExtPhase,
    pub fn_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtPhase {
    /// Phase 1 — `@parser_rule`
    ParserRule,
    /// Phase 2 — `@ast_node`
    AstNode,
    /// Phase 3 — `@type_rule`
    TypeRule,
    /// Phase 4 — `@lower_pass`
    LowerPass,
    /// Phase 5 — `@opt_pass`
    OptPass,
    /// Phase 6 — `@backend`
    Backend,
}
