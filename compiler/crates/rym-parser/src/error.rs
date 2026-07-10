use thiserror::Error;
use rym_lexer::{Span, TokenKind};

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("expected {expected}, found {found:?} at {span:?}")]
    UnexpectedToken {
        expected: &'static str,
        found:    TokenKind,
        span:     Span,
    },

    #[error("unexpected end of file while parsing {context}")]
    UnexpectedEof { context: &'static str },

    #[error("nesting depth exceeds 1 at {span:?} — extract into a named function")]
    IllegalNesting { span: Span },

    #[error("definition found in algorithm zone at {span:?}")]
    DefInAlgZone { span: Span },

    #[error("pipeline expression found in definition zone at {span:?}")]
    PipeInDefZone { span: Span },
}
