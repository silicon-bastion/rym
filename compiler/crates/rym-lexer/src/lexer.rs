use crate::error::LexError;
use crate::token::{Span, Token, TokenKind};

pub struct Lexer<'src> {
    src: &'src str,
    /// Current byte offset into `src`.
    pos: usize,
}

impl<'src> Lexer<'src> {
    pub fn new(src: &'src str) -> Self {
        Self { src, pos: 0 }
    }

    /// Tokenise the entire source, returning a token list or the first error.
    pub fn tokenize(&mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    // ── Helpers ───────────────────────────────────────────────

    fn current(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.current()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    /// Skip spaces, tabs, carriage returns, and `//` line comments.
    /// Newlines are kept so the parser can use them as statement terminators.
    fn skip_whitespace_and_comments(&mut self) {
        loop {
            while matches!(self.current(), Some(' ') | Some('\t') | Some('\r')) {
                self.advance();
            }
            if self.src[self.pos..].starts_with("//") {
                while !matches!(self.current(), Some('\n') | None) {
                    self.advance();
                }
                continue;
            }
            break;
        }
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_whitespace_and_comments();

        let start = self.pos;

        let ch = match self.current() {
            None => return Ok(Token::new(TokenKind::Eof, start, start)),
            Some(c) => c,
        };

        if ch == '\n' {
            self.advance();
            return Ok(Token::new(TokenKind::Newline, start, self.pos));
        }

        if ch.is_ascii_digit() {
            return self.lex_number(start);
        }

        if ch == '"' {
            return self.lex_string(start);
        }

        // Identifiers and keywords (Unicode-aware, supports Chinese)
        if ch.is_alphabetic() || ch == '_' || !ch.is_ascii() {
            return self.lex_ident_or_keyword(start);
        }

        self.advance();
        let kind = match ch {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ';' => TokenKind::Semi,
            '@' => TokenKind::At,
            '%' => TokenKind::Percent,
            '/' => TokenKind::Slash,
            '?' => TokenKind::Question,
            '*' => TokenKind::Star,
            '+' => {
                if self.current() == Some('+') {
                    self.advance();
                    TokenKind::PlusPlus
                } else {
                    TokenKind::Plus
                }
            }
            '-' => {
                if self.current() == Some('>') {
                    self.advance();
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '=' => {
                if self.current() == Some('>') {
                    self.advance();
                    TokenKind::FatArrow
                } else if self.current() == Some('=') {
                    self.advance();
                    TokenKind::Eq
                } else {
                    TokenKind::Assign
                }
            }
            '!' => {
                if self.current() == Some('=') {
                    self.advance();
                    TokenKind::NotEq
                } else {
                    TokenKind::Not
                }
            }
            '<' => {
                if self.current() == Some('=') {
                    self.advance();
                    TokenKind::LtEq
                } else if self.current() == Some('<') {
                    self.advance();
                    TokenKind::Shl
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.current() == Some('=') {
                    self.advance();
                    TokenKind::GtEq
                } else if self.current() == Some('>') {
                    self.advance();
                    TokenKind::Shr
                } else {
                    TokenKind::Gt
                }
            }
            '&' => {
                if self.current() == Some('&') {
                    self.advance();
                    TokenKind::And
                } else {
                    // Single `&` is lexed as Amp; parser context disambiguates
                    // between address-of (prefix) and bitwise AND (infix).
                    TokenKind::Amp
                }
            }
            '^' => TokenKind::Caret,
            '~' => TokenKind::Tilde,
            '|' => {
                if self.current() == Some('>') {
                    self.advance();
                    TokenKind::Pipe
                } else if self.current() == Some('|') {
                    self.advance();
                    TokenKind::Or
                } else {
                    TokenKind::BitOr
                }
            }
            ':' => {
                if self.current() == Some(':') {
                    self.advance();
                    TokenKind::ColonColon
                } else {
                    TokenKind::Colon
                }
            }
            '.' => {
                if self.current() == Some('.') {
                    self.advance();
                    TokenKind::DotDot
                } else {
                    TokenKind::Dot
                }
            }
            other => return Err(LexError::UnknownChar { ch: other, pos: start }),
        };

        Ok(Token::new(kind, start, self.pos))
    }

    fn lex_number(&mut self, start: usize) -> Result<Token, LexError> {
        while matches!(self.current(), Some('0'..='9') | Some('_')) {
            self.advance();
        }

        let is_float = self.current() == Some('.')
            && matches!(self.peek_nth(1), Some('0'..='9'));

        if is_float {
            self.advance(); // consume '.'
            while matches!(self.current(), Some('0'..='9') | Some('_')) {
                self.advance();
            }
            let text = self.src[start..self.pos].replace('_', "");
            let val: f64 = text.parse().map_err(|_| LexError::InvalidNumber {
                text: text.clone(),
                pos: start,
            })?;
            Ok(Token::new(TokenKind::Float(val), start, self.pos))
        } else {
            let text = self.src[start..self.pos].replace('_', "");
            let val: i64 = text.parse().map_err(|_| LexError::InvalidNumber {
                text: text.clone(),
                pos: start,
            })?;
            Ok(Token::new(TokenKind::Int(val), start, self.pos))
        }
    }

    fn lex_string(&mut self, start: usize) -> Result<Token, LexError> {
        self.advance(); // consume opening '"'
        let mut buf = String::new();
        loop {
            match self.advance() {
                None | Some('\n') => {
                    return Err(LexError::UnterminatedString {
                        span: Span::new(start, self.pos),
                    })
                }
                Some('"') => break,
                Some('\\') => match self.advance() {
                    Some('n')  => buf.push('\n'),
                    Some('t')  => buf.push('\t'),
                    Some('\\') => buf.push('\\'),
                    Some('"')  => buf.push('"'),
                    Some(c)    => buf.push(c),
                    None => return Err(LexError::UnterminatedString {
                        span: Span::new(start, self.pos),
                    }),
                },
                Some(c) => buf.push(c),
            }
        }
        Ok(Token::new(TokenKind::Str(buf), start, self.pos))
    }

    fn lex_ident_or_keyword(&mut self, start: usize) -> Result<Token, LexError> {
        while matches!(
            self.current(),
            Some(c) if c.is_alphanumeric() || c == '_' || !c.is_ascii()
        ) {
            self.advance();
        }
        let text = &self.src[start..self.pos];
        // `asm!` — the `!` is part of the macro-call sigil, consumed here.
        if text == "asm" && self.current() == Some('!') {
            self.advance();
            return Ok(Token::new(TokenKind::AsmBang, start, self.pos));
        }
        let kind = Self::keyword(text);
        Ok(Token::new(kind, start, self.pos))
    }

    fn keyword(text: &str) -> TokenKind {
        match text {
            "fn"        => TokenKind::Fn,
            "type"      => TokenKind::Type,
            "enum"      => TokenKind::Enum,
            "struct"    => TokenKind::Struct,
            "match"     => TokenKind::Match,
            "if"        => TokenKind::If,
            "else"      => TokenKind::Else,
            "for"       => TokenKind::For,
            "in"        => TokenKind::In,
            "return"    => TokenKind::Return,
            "defer"     => TokenKind::Defer,
            "while"     => TokenKind::While,
            "loop"      => TokenKind::Loop,
            "break"     => TokenKind::Break,
            "continue"  => TokenKind::Continue,
            "as"        => TokenKind::As,
            "import"    => TokenKind::Import,
            "base"      => TokenKind::Base,
            "safe"      => TokenKind::Safe,
            "read"      => TokenKind::Read,
            "mut"       => TokenKind::Mut,
            "move"      => TokenKind::Move,
            "or_else"   => TokenKind::OrElse,
            "or_return" => TokenKind::OrReturn,
            "or_panic"  => TokenKind::OrPanic,
            "or_zero"   => TokenKind::OrZero,
            "or_nil"    => TokenKind::OrNil,
            "let"       => TokenKind::Let,
            "var"       => TokenKind::Var,
            // Chinese reserved keywords
            "定"        => TokenKind::Ding,
            "设"        => TokenKind::She,
            // Built-in types
            "i8"        => TokenKind::I8,
            "i16"       => TokenKind::I16,
            "i32"       => TokenKind::I32,
            "i64"       => TokenKind::I64,
            "u8"        => TokenKind::U8,
            "u16"       => TokenKind::U16,
            "u32"       => TokenKind::U32,
            "u64"       => TokenKind::U64,
            "f32"       => TokenKind::F32,
            "f64"       => TokenKind::F64,
            "bool"      => TokenKind::BoolTy,
            "usize"     => TokenKind::Usize,
            "str"       => TokenKind::StrTy,
            "void"      => TokenKind::Void,
            "true"      => TokenKind::Bool(true),
            "false"     => TokenKind::Bool(false),
            other       => TokenKind::Ident(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(src: &str) -> Vec<TokenKind> {
        Lexer::new(src)
            .tokenize()
            .unwrap()
            .into_iter()
            .filter(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Eof))
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn keywords() {
        assert_eq!(
            lex("fn type match"),
            vec![TokenKind::Fn, TokenKind::Type, TokenKind::Match]
        );
    }

    #[test]
    fn chinese_keywords() {
        assert_eq!(lex("定 设"), vec![TokenKind::Ding, TokenKind::She]);
    }

    #[test]
    fn pipe_operator() {
        assert_eq!(
            lex("a |> b"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::Pipe,
                TokenKind::Ident("b".into()),
            ]
        );
    }

    #[test]
    fn arrows() {
        assert_eq!(lex("-> =>"), vec![TokenKind::Arrow, TokenKind::FatArrow]);
    }

    #[test]
    fn numbers() {
        assert_eq!(
            lex("42 3.14 1_000"),
            vec![TokenKind::Int(42), TokenKind::Float(3.14), TokenKind::Int(1000)]
        );
    }

    #[test]
    fn string_literal() {
        assert_eq!(lex(r#""hello Rym""#), vec![TokenKind::Str("hello Rym".into())]);
    }

    #[test]
    fn chinese_identifier() {
        let tokens = lex("fn 解析请求(原始: read []u8)");
        assert!(tokens.contains(&TokenKind::Fn));
        assert!(tokens.contains(&TokenKind::Ident("解析请求".into())));
        assert!(tokens.contains(&TokenKind::Read));
    }

    #[test]
    fn error_operators() {
        assert_eq!(
            lex("or_return or_panic or_else or_zero or_nil"),
            vec![
                TokenKind::OrReturn,
                TokenKind::OrPanic,
                TokenKind::OrElse,
                TokenKind::OrZero,
                TokenKind::OrNil,
            ]
        );
    }
}
