use thiserror::Error;
use rym_lexer::{Span, TokenKind};

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("expected {expected}, found {found:?}")]
    UnexpectedToken {
        expected: &'static str,
        found:    TokenKind,
        span:     Span,
    },

    #[error("unexpected end of file while parsing {context}")]
    UnexpectedEof { context: &'static str },

    #[error("nesting depth exceeds 1 — extract into a named function")]
    IllegalNesting { span: Span },

    #[error("definition found in algorithm zone")]
    DefInAlgZone { span: Span },

    #[error("pipeline expression found in definition zone")]
    PipeInDefZone { span: Span },
}

impl ParseError {
    pub fn span(&self) -> Option<Span> {
        match self {
            ParseError::UnexpectedToken { span, .. } => Some(*span),
            ParseError::IllegalNesting  { span, .. } => Some(*span),
            ParseError::DefInAlgZone    { span, .. } => Some(*span),
            ParseError::PipeInDefZone   { span, .. } => Some(*span),
            ParseError::UnexpectedEof   { .. }       => None,
        }
    }
}
