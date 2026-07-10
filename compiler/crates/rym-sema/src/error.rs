use thiserror::Error;
use rym_lexer::Span;

#[derive(Debug, Error)]
pub enum SemaError {
    #[error("undefined name '{name}' at {span:?}")]
    Undefined { name: String, span: Span },

    #[error("type mismatch: expected '{expected}', found '{found}' at {span:?}")]
    TypeMismatch { expected: String, found: String, span: Span },

    #[error("ownership conflict at {span:?}: {msg}")]
    OwnershipConflict { msg: String, span: Span },

    #[error("cannot move '{name}' — it was declared with 'read' at {span:?}")]
    MoveOfReadBinding { name: String, span: Span },

    #[error("cannot mutate '{name}' — it was declared with '定' (immutable) at {span:?}")]
    MutateImmutable { name: String, span: Span },

    #[error("use of moved value '{name}' at {span:?}")]
    UseAfterMove { name: String, span: Span },

    #[error("function '{name}' expects {expected} arguments, got {found} at {span:?}")]
    ArgCountMismatch { name: String, expected: usize, found: usize, span: Span },

    #[error("parameter '{param}' expects ownership mode '{expected}', got '{found}' at {span:?}")]
    OwnershipModeMismatch { param: String, expected: String, found: String, span: Span },

    #[error("pipe left side transfers ownership but right side does not declare 'move' at {span:?}")]
    PipeOwnershipConflict { span: Span },

    #[error("Result type required for 'or_return' at {span:?}")]
    OrReturnNonResult { span: Span },

    #[error("allocator parameter missing in function '{name}' that allocates at {span:?}")]
    MissingAllocator { name: String, span: Span },
}
