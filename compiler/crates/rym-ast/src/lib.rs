pub mod expr;
pub mod item;
pub mod stmt;
pub mod ty;

use rym_lexer::Span;

/// A complete parsed `.rym` source file.
///
/// Every Rym file is physically split into two zones:
///   - definition zone (top): all `fn`, `type`, `enum`, `import` items
///   - algorithm zone (bottom): pipeline expressions and variable bindings
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub ring:     Ring,
    pub def_zone: Vec<item::Item>,
    pub alg_zone: Vec<stmt::Stmt>,
    pub span:     Span,
}

/// Which execution ring this file belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ring {
    /// Default — automatic safety, RAII, standard allocators.
    Safe,
    /// Privileged — raw hardware access, inline LoongArch asm, no runtime checks.
    Base,
}

impl Default for Ring {
    fn default() -> Self {
        Ring::Safe
    }
}
