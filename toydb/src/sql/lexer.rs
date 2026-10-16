//! The lexer: turns SQL text into a flat list of tokens.
//!
//! ```text
//! "SELECT name FROM t WHERE id >= 10;"
//!   => [Keyword(Select), Ident("name"), Keyword(From), Ident("t"),
//!       Keyword(Where), Ident("id"), GtEq, Number("10"), Semicolon]
//! ```
//!
//! Every token records the byte offset where it starts so the parser can
//! point at the exact line and column of a syntax error.

use std::fmt;

use crate::error::{Error, Result};

/// Reserved words. Matched case-insensitively.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    All,
    And,
    As,
    Asc,
    Begin,
    Between,
    Boolean,
    By,
    Commit,
    Create,
    Delete,
    Desc,
    Distinct,
    Drop,
    Exists,
    Explain,
    False,
    From,
    Group,
    Having,
    If,
    In,
    Index,
    Insert,
    Integer,
    Into,
    Is,
    Key,
    Like,
    Limit,
    Not,
    Null,
    Offset,
    On,
    Or,
    Order,
    Primary,
    Real,
    Rollback,
    Select,
    Set,
    Table,
    Text,
    Transaction,
    True,
    Unique,
    Update,
    Values,
    Where,
}

impl Keyword {
    fn from_str(s: &str) -> Option<Keyword> {
        use Keyword::*;
        Some(match s.to_ascii_uppercase().as_str() {
            "ALL" => All,
            "AND" => And,
            "AS" => As,
            "ASC" => Asc,
            "BEGIN" => Begin,
            "BETWEEN" => Between,
            "BOOLEAN" | "BOOL" => Boolean,
            "BY" => By,
            "COMMIT" => Commit,
            "CREATE" => Create,
            "DELETE" => Delete,
            "DESC" => Desc,
            "DISTINCT" => Distinct,
            "DROP" => Drop,
            "EXISTS" => Exists,
            "EXPLAIN" => Explain,
            "FALSE" => False,
            "FROM" => From,
            "GROUP" => Group,
            "HAVING" => Having,
            "IF" => If,
            "IN" => In,
            "INDEX" => Index,
            "INSERT" => Insert,
            "INTEGER" | "INT" | "BIGINT" => Integer,
            "INTO" => Into,
            "IS" => Is,
            "KEY" => Key,
            "LIKE" => Like,
            "LIMIT" => Limit,
            "NOT" => Not,
            "NULL" => Null,
            "OFFSET" => Offset,
            "ON" => On,
            "OR" => Or,
            "ORDER" => Order,
            "PRIMARY" => Primary,
            "REAL" | "FLOAT" | "DOUBLE" => Real,
            "ROLLBACK" => Rollback,
            "SELECT" => Select,
            "SET" => Set,
            "TABLE" => Table,
            "TEXT" | "VARCHAR" | "STRING" => Text,
            "TRANSACTION" => Transaction,
            "TRUE" => True,
            "UNIQUE" => Unique,
            "UPDATE" => Update,
            "VALUES" => Values,
            "WHERE" => Where,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Keyword(Keyword),
    /// An identifier. Unquoted identifiers are lower-cased (SQL identifiers
    /// are case-insensitive); `"Quoted"` ones keep their case.
    Ident(String),
    /// A numeric literal, kept as text until the parser decides whether it
    /// is an INTEGER or a REAL.
    Number(String),
    /// A `'string literal'` with `''` escapes already resolved.
    String(String),
    LParen,
    RParen,
    Comma,
    Semicolon,
    Dot,
    Star,
    Plus,
    Minus,
    Slash,
    Percent,
    Concat,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Keyword(k) => write!(f, "{}", format!("{k:?}").to_uppercase()),
            Token::Ident(s) => write!(f, "identifier \"{s}\""),
            Token::Number(n) => write!(f, "number {n}"),
            Token::String(s) => write!(f, "string '{s}'"),
            t => f.write_str(match t {
                Token::LParen => "(",
                Token::RParen => ")",
                Token::Comma => ",",
                Token::Semicolon => ";",
                Token::Dot => ".",
                Token::Star => "*",
                Token::Plus => "+",
                Token::Minus => "-",
                Token::Slash => "/",
                Token::Percent => "%",
                Token::Concat => "||",
                Token::Eq => "=",
                Token::NotEq => "!=",
                Token::Lt => "<",
                Token::LtEq => "<=",
                Token::Gt => ">",
                Token::GtEq => ">=",
                _ => unreachable!(),
            }),
        }
    }
}

/// A token and the byte offset where it starts.
#[derive(Clone, Debug, PartialEq)]
pub struct Spanned {
    pub token: Token,
    pub pos: usize,
}

/// Convert a byte offset into a 1-based "line L, column C" string.
pub fn describe_pos(src: &str, pos: usize) -> String {
    let before = &src[..pos.min(src.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    format!("line {line}, column {col}")
}

/// Tokenize `src`.
pub fn tokenize(src: &str) -> Result<Vec<Spanned>> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let err = |pos: usize, msg: &str| Error::Parse(format!("{msg} at {}", describe_pos(src, pos)));
    while i < bytes.len() {
        let c = bytes[i];
        let start = i;
        // Whitespace and comments.
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'-' && bytes.get(i + 1) == Some(&b'-') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            match src[i + 2..].find("*/") {
                Some(end) => i += 2 + end + 2,
                None => return Err(err(start, "unterminated comment")),
            }
            continue;
        }
        let token = if c.is_ascii_alphabetic() || c == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            match Keyword::from_str(word) {
                Some(k) => Token::Keyword(k),
                None => Token::Ident(word.to_ascii_lowercase()),
            }
        } else if c.is_ascii_digit() || (c == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                let mut j = i + 1;
                if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j].is_ascii_digit() {
                    i = j;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            if i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                return Err(err(i, "invalid number"));
            }
            Token::Number(src[start..i].to_string())
        } else if c == b'\'' || c == b'"' {
            // 'string' or "quoted identifier"; the quote doubles as escape.
            let quote = c;
            let mut s = String::new();
            i += 1;
            loop {
                match src[i..].find(quote as char) {
                    None => {
                        return Err(err(
                            start,
                            if quote == b'\'' { "unterminated string" } else { "unterminated quoted identifier" },
                        ))
                    }
                    Some(off) => {
                        s.push_str(&src[i..i + off]);
                        i += off + 1;
                        if bytes.get(i) == Some(&quote) {
                            s.push(quote as char);
                            i += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            if quote == b'\'' {
                Token::String(s)
            } else {
                Token::Ident(s)
            }
        } else {
            let two = bytes.get(i + 1).copied();
            let (tok, len) = match (c, two) {
                (b'<', Some(b'=')) => (Token::LtEq, 2),
                (b'>', Some(b'=')) => (Token::GtEq, 2),
                (b'<', Some(b'>')) => (Token::NotEq, 2),
                (b'!', Some(b'=')) => (Token::NotEq, 2),
                (b'|', Some(b'|')) => (Token::Concat, 2),
                (b'=', Some(b'=')) => (Token::Eq, 2),
                (b'(', _) => (Token::LParen, 1),
                (b')', _) => (Token::RParen, 1),
                (b',', _) => (Token::Comma, 1),
                (b';', _) => (Token::Semicolon, 1),
                (b'.', _) => (Token::Dot, 1),
                (b'*', _) => (Token::Star, 1),
                (b'+', _) => (Token::Plus, 1),
                (b'-', _) => (Token::Minus, 1),
                (b'/', _) => (Token::Slash, 1),
                (b'%', _) => (Token::Percent, 1),
                (b'=', _) => (Token::Eq, 1),
                (b'<', _) => (Token::Lt, 1),
                (b'>', _) => (Token::Gt, 1),
                _ => {
                    let ch = src[i..].chars().next().unwrap();
                    return Err(err(start, &format!("unexpected character '{ch}'")));
                }
            };
            i += len;
            tok
        };
        out.push(Spanned { token, pos: start });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Token> {
        tokenize(s).unwrap().into_iter().map(|t| t.token).collect()
    }

    #[test]
    fn basic_statement() {
        use Token::*;
        assert_eq!(
            toks("select Name, 1.5e3 FROM \"T\" where x<>'it''s' -- comment\n;"),
            vec![
                Keyword(self::Keyword::Select),
                Ident("name".into()),
                Comma,
                Number("1.5e3".into()),
                Keyword(self::Keyword::From),
                Ident("T".into()),
                Keyword(self::Keyword::Where),
                Ident("x".into()),
                NotEq,
                String("it's".into()),
                Semicolon,
            ]
        );
    }

    #[test]
    fn operators_and_comments() {
        use Token::*;
        assert_eq!(
            toks("a<=b>=c||d /* skip */ != .5"),
            vec![
                Ident("a".into()),
                LtEq,
                Ident("b".into()),
                GtEq,
                Ident("c".into()),
                Concat,
                Ident("d".into()),
                NotEq,
                Number(".5".into())
            ]
        );
    }

    #[test]
    fn errors_have_positions() {
        let e = tokenize("select\n  'oops").unwrap_err().to_string();
        assert!(e.contains("line 2, column 3"), "{e}");
        assert!(tokenize("select 12abc").is_err());
        assert!(tokenize("select #").is_err());
    }
}
