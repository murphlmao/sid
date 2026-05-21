//! A small dialect-agnostic SQL lexer for syntax highlighting in the query
//! editor. Not a parser — never builds an AST, never validates syntax. It
//! exists to classify each byte range of the source into a token kind so the
//! TUI can colour it.
//!
//! Robustness contract:
//! - Tokenising any byte sequence must terminate.
//! - Tokenising must never panic.
//! - The concatenation of `tok.text` equals the input (no characters dropped).
//! - `tok.offset` is the byte offset where the token begins in the input.
//!
//! These invariants are enforced by tests and by the proptest in
//! `tests/lexer_proptest.rs`.

use std::borrow::Cow;

/// Token classification.
///
/// # Examples
///
/// ```
/// use sid_db_clients::lexer::TokenKind;
/// let _ = TokenKind::Keyword;
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    /// A reserved SQL word (case-insensitive match against [`KEYWORDS`]).
    Keyword,
    /// Identifier (table/column name, etc.).
    Identifier,
    /// String literal (single-quoted; SQL escape is `''`).
    String,
    /// Numeric literal (integer or float).
    Number,
    /// Line comment (`-- ...`) or block comment (`/* ... */`).
    Comment,
    /// Operator or punctuation character.
    Punctuation,
    /// Run of whitespace characters.
    Whitespace,
    /// Any byte we don't recognise. Renderer falls back to the foreground colour.
    Unknown,
}

/// One token: kind + byte offset + owned text.
///
/// # Examples
///
/// ```
/// use sid_db_clients::lexer::{Token, TokenKind};
/// let t = Token { kind: TokenKind::Whitespace, offset: 0, text: " ".into() };
/// assert_eq!(t.offset, 0);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    /// Token kind.
    pub kind: TokenKind,
    /// Byte offset within the input where this token begins.
    pub offset: usize,
    /// Verbatim text covered by this token.
    pub text: Cow<'static, str>,
}

/// Tokenise the input. Returns a vector whose `text` concatenation equals
/// `input`.
///
/// # Examples
///
/// ```
/// use sid_db_clients::lexer::{tokenize, TokenKind};
/// let toks = tokenize("SELECT 1");
/// assert_eq!(toks[0].kind, TokenKind::Keyword);
/// ```
pub fn tokenize(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let b = bytes[i];
        let (kind, end) = match b {
            // Whitespace
            b' ' | b'\t' | b'\n' | b'\r' => {
                let mut j = i;
                while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                    j += 1;
                }
                (TokenKind::Whitespace, j)
            }
            // Line comment "-- ..."
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                let mut j = i + 2;
                while j < bytes.len() && bytes[j] != b'\n' {
                    j += 1;
                }
                (TokenKind::Comment, j)
            }
            // Block comment "/* ... */"
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                let mut j = i + 2;
                let mut closed = false;
                while j + 1 < bytes.len() {
                    if bytes[j] == b'*' && bytes[j + 1] == b'/' {
                        j += 2;
                        closed = true;
                        break;
                    }
                    j += 1;
                }
                if !closed {
                    j = bytes.len();
                }
                (TokenKind::Comment, j)
            }
            // Single-quoted string (SQL escape is doubled quote: '')
            b'\'' => {
                let mut j = i + 1;
                while j < bytes.len() {
                    if bytes[j] == b'\'' {
                        if j + 1 < bytes.len() && bytes[j + 1] == b'\'' {
                            j += 2;
                            continue;
                        }
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                (TokenKind::String, j)
            }
            // Double-quoted identifier (also delim-string in some dialects)
            b'"' => {
                let mut j = i + 1;
                while j < bytes.len() {
                    if bytes[j] == b'"' {
                        if j + 1 < bytes.len() && bytes[j + 1] == b'"' {
                            j += 2;
                            continue;
                        }
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                (TokenKind::Identifier, j)
            }
            // Punctuation
            b'(' | b')' | b',' | b';' | b'*' | b'+' | b'-' | b'/' | b'%' | b'<' | b'>' | b'='
            | b'!' | b'.' | b':' | b'[' | b']' | b'{' | b'}' => (TokenKind::Punctuation, i + 1),
            // Number (integer or float)
            b'0'..=b'9' => {
                let mut j = i;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
                    j += 1;
                }
                (TokenKind::Number, j)
            }
            // Identifier or keyword (starts with letter or underscore)
            _ if is_ident_start(b) => {
                let mut j = i;
                while j < bytes.len() && is_ident_continue(bytes[j]) {
                    j += 1;
                }
                let slice = &input[i..j];
                let kind = if is_keyword(slice) {
                    TokenKind::Keyword
                } else {
                    TokenKind::Identifier
                };
                (kind, j)
            }
            _ => (TokenKind::Unknown, i + 1),
        };
        // Safety: end must always advance.
        let end = end.max(start + 1).min(bytes.len());
        // For multi-byte UTF-8 chars in Unknown/identifier paths, snap to the next char boundary.
        let end = snap_to_char_boundary(input, end);
        out.push(Token {
            kind,
            offset: start,
            text: Cow::Owned(input[start..end].to_string()),
        });
        i = end;
    }
    out
}

fn is_ident_start(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'_') || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit() || b == b'$'
}

fn snap_to_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

fn is_keyword(ident: &str) -> bool {
    let upper: String = ident.chars().map(|c| c.to_ascii_uppercase()).collect();
    KEYWORDS.binary_search(&upper.as_str()).is_ok()
}

/// Sorted set of SQL reserved words recognised by [`tokenize`]. Lexicographic
/// order is required for `binary_search`.
pub const KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "AND",
    "AS",
    "ASC",
    "BEGIN",
    "BETWEEN",
    "BY",
    "CASCADE",
    "CASE",
    "CAST",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "COMMIT",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "DATABASE",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DROP",
    "ELSE",
    "END",
    "EXCEPT",
    "EXISTS",
    "EXPLAIN",
    "FALSE",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "GRANT",
    "GROUP",
    "HAVING",
    "IF",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INTERSECT",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "LEFT",
    "LIKE",
    "LIMIT",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "PRIMARY",
    "REFERENCES",
    "RETURNING",
    "REVOKE",
    "RIGHT",
    "ROLLBACK",
    "SELECT",
    "SET",
    "TABLE",
    "THEN",
    "TO",
    "TRANSACTION",
    "TRIGGER",
    "TRUE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USING",
    "VALUES",
    "VIEW",
    "WHEN",
    "WHERE",
    "WITH",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_sorted() {
        let mut sorted = KEYWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, KEYWORDS);
    }
}
