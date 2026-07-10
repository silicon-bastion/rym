use thiserror::Error;
use rym_lexer::Span;

#[derive(Debug, Error)]
pub enum SemaError {
    #[error("undefined name '{name}'")]
    Undefined { name: String, span: Span },

    #[error("type mismatch: expected '{expected}', found '{found}'")]
    TypeMismatch { expected: String, found: String, span: Span },

    #[error("ownership conflict: {msg}")]
    OwnershipConflict { msg: String, span: Span },

    #[error("cannot move '{name}' — it was declared with 'read'")]
    MoveOfReadBinding { name: String, span: Span },

    #[error("cannot mutate '{name}' — it was declared with '定' (immutable)")]
    MutateImmutable { name: String, span: Span },

    #[error("use of moved value '{name}'")]
    UseAfterMove { name: String, span: Span },

    #[error("function '{name}' expects {expected} arguments, got {found}")]
    ArgCountMismatch { name: String, expected: usize, found: usize, span: Span },

    #[error("parameter '{param}' expects ownership mode '{expected}', got '{found}'")]
    OwnershipModeMismatch { param: String, expected: String, found: String, span: Span },

    #[error("pipe left side transfers ownership but right side does not declare 'move'")]
    PipeOwnershipConflict { span: Span },

    #[error("Result type required for 'or_return'")]
    OrReturnNonResult { span: Span },

    #[error("allocator parameter missing in function '{name}' that allocates")]
    MissingAllocator { name: String, span: Span },
}

impl SemaError {
    pub fn span(&self) -> Span {
        match self {
            SemaError::Undefined              { span, .. } => *span,
            SemaError::TypeMismatch           { span, .. } => *span,
            SemaError::OwnershipConflict      { span, .. } => *span,
            SemaError::MoveOfReadBinding      { span, .. } => *span,
            SemaError::MutateImmutable        { span, .. } => *span,
            SemaError::UseAfterMove           { span, .. } => *span,
            SemaError::ArgCountMismatch       { span, .. } => *span,
            SemaError::OwnershipModeMismatch  { span, .. } => *span,
            SemaError::PipeOwnershipConflict  { span, .. } => *span,
            SemaError::OrReturnNonResult      { span, .. } => *span,
            SemaError::MissingAllocator       { span, .. } => *span,
        }
    }
}
