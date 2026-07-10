use thiserror::Error;
use crate::token::Span;

#[derive(Debug, Error)]
pub enum LexError {
    #[error("unknown character '{ch}' at byte offset {pos}")]
    UnknownChar { ch: char, pos: usize },

    #[error("unterminated string literal starting at {span:?}")]
    UnterminatedString { span: Span },

    #[error("invalid numeric literal '{text}' at byte offset {pos}")]
    InvalidNumber { text: String, pos: usize },
}
