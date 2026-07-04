//! A small dialect-agnostic SQL lexer for syntax highlighting in the query
//! editor. Not a parser — never builds an AST, never validates syntax. It
//! exists to classify each byte range of the source into a token kind so the
//! frontend can colour it.
//!
//! Robustness contract:
//! - Tokenising any byte sequence must terminate.
//! - Tokenising must never panic.
//! - The concatenation of `tok.text` equals the input (no characters dropped).
//! - `tok.offset` is the byte offset where the token begins in the input.
//!
//! These invariants are enforced by the tests in this module.

use std::borrow::Cow;

/// Token classification.
///
/// # Examples
///
/// ```
/// use sid_db::lexer::TokenKind;
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
/// use sid_db::lexer::{Token, TokenKind};
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
/// use sid_db::lexer::{tokenize, TokenKind};
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

/// Strip trailing SQL trivia — a trailing `;` and any whitespace/comments around it —
/// from the end of `input`, using [`tokenize`] rather than a naive `str` scan so a `--`
/// or `;` that's actually inside a string literal (or an identifier, or a block comment)
/// is never mistaken for one of these.
///
/// Repeats: `"SELECT 1 ;  ; "` strips down to `"SELECT 1"` (both semicolons, and the
/// whitespace/comments between/after them, are removed).
///
/// Deliberately does NOT strip a trailing comment that isn't preceded by a `;` — e.g.
/// `"SELECT 1\n-- note"` is returned unchanged. That's not a gap: a caller wrapping this
/// text in its own SQL (e.g. `query_paged`'s `SELECT * FROM ( .. ) AS sid_sub LIMIT ..`
/// subquery) is expected to put its own trailing tail on a fresh line, since a `--`
/// comment only ever runs to the next newline — see `crates/sid-db/src/postgres.rs`'s
/// `query_paged` and `crates/sid-db/src/sqlite.rs`'s `query_paged` for the motivating bug
/// (a trailing line comment in the caller's SQL silently ate the wrapper's own closing
/// `) .. LIMIT .. OFFSET ..` tail when it was appended on the same line).
///
/// # Examples
///
/// ```
/// use sid_db::lexer::strip_trailing_trivia;
/// assert_eq!(strip_trailing_trivia("SELECT 1;"), "SELECT 1");
/// assert_eq!(strip_trailing_trivia("SELECT 1; -- trailing note"), "SELECT 1");
/// assert_eq!(strip_trailing_trivia("SELECT ';'"), "SELECT ';'");
/// ```
pub fn strip_trailing_trivia(input: &str) -> &str {
    let tokens = tokenize(input);
    let mut end = input.len();
    let mut i = tokens.len();
    loop {
        while i > 0
            && matches!(
                tokens[i - 1].kind,
                TokenKind::Whitespace | TokenKind::Comment
            )
        {
            i -= 1;
        }
        if i > 0
            && tokens[i - 1].kind == TokenKind::Punctuation
            && tokens[i - 1].text.as_ref() == ";"
        {
            i -= 1;
            end = tokens[i].offset;
            continue;
        }
        break;
    }
    &input[..end]
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

    #[test]
    fn tokenize_concatenation_equals_input() {
        for input in [
            "SELECT * FROM users WHERE id = 1;",
            "-- comment\nSELECT 1",
            "/* block */ SELECT 'a''b'",
            "",
            "\u{0}\u{1}weird\u{7f}",
            "select \"quoted id\" from t",
        ] {
            let toks = tokenize(input);
            let joined: String = toks.iter().map(|t| t.text.as_ref()).collect();
            assert_eq!(joined, input, "round-trip for {input:?}");
        }
    }

    #[test]
    fn tokenize_never_panics_on_arbitrary_bytes() {
        // Not valid UTF-8 as raw bytes, but we only ever call tokenize on a
        // `&str`, so exercise a range of odd-but-valid strings instead.
        let inputs = [
            "\u{0}",
            "\u{7f}",
            "🦀🦀🦀",
            "'",
            "\"",
            "--",
            "/*",
            "*/",
            ";;;;",
            "𝓢𝓔𝓛𝓔𝓒𝓣",
        ];
        for input in inputs {
            let toks = tokenize(input);
            let joined: String = toks.iter().map(|t| t.text.as_ref()).collect();
            assert_eq!(joined, input);
        }
    }

    #[test]
    fn keyword_matching_is_case_insensitive() {
        for kw in ["select", "SELECT", "Select"] {
            let toks = tokenize(kw);
            assert_eq!(toks.len(), 1);
            assert_eq!(toks[0].kind, TokenKind::Keyword);
        }
    }

    #[test]
    fn identifiers_are_distinguished_from_keywords() {
        let toks = tokenize("my_table");
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind, TokenKind::Identifier);
    }

    #[test]
    fn strip_trailing_trivia_removes_a_trailing_semicolon() {
        assert_eq!(strip_trailing_trivia("SELECT 1;"), "SELECT 1");
    }

    #[test]
    fn strip_trailing_trivia_removes_semicolon_then_line_comment() {
        // Round-D bug repro: `; -- note` used to leave the `;` embedded mid-wrapper.
        assert_eq!(
            strip_trailing_trivia("SELECT 1 AS one; -- trailing comment"),
            "SELECT 1 AS one"
        );
    }

    #[test]
    fn strip_trailing_trivia_removes_a_run_of_semicolons_and_whitespace() {
        assert_eq!(strip_trailing_trivia("SELECT 1 ;  ; "), "SELECT 1 ");
    }

    #[test]
    fn strip_trailing_trivia_removes_semicolon_before_trailing_block_comment() {
        assert_eq!(
            strip_trailing_trivia("SELECT 1; /* trailing */"),
            "SELECT 1"
        );
    }

    #[test]
    fn strip_trailing_trivia_does_not_touch_a_semicolon_inside_a_string_literal() {
        // The trailing `;` is INSIDE the string literal's own token, not a separate
        // Punctuation token, so it must survive untouched.
        assert_eq!(
            strip_trailing_trivia("SELECT 'trailing;'"),
            "SELECT 'trailing;'"
        );
    }

    #[test]
    fn strip_trailing_trivia_does_not_touch_a_line_comment_marker_inside_a_string_literal() {
        assert_eq!(
            strip_trailing_trivia("SELECT '--not a comment'"),
            "SELECT '--not a comment'"
        );
    }

    #[test]
    fn strip_trailing_trivia_leaves_a_bare_trailing_comment_alone_with_no_semicolon() {
        // No trailing `;` before the comment -- deliberately NOT stripped (see the
        // function's doc comment: the newline-separated wrapper tail handles this case
        // instead).
        assert_eq!(
            strip_trailing_trivia("SELECT 1 AS one\n-- trailing comment"),
            "SELECT 1 AS one\n-- trailing comment"
        );
    }

    #[test]
    fn strip_trailing_trivia_leaves_a_trailing_block_comment_alone_with_no_semicolon() {
        assert_eq!(
            strip_trailing_trivia("SELECT 1 AS one /* trailing */"),
            "SELECT 1 AS one /* trailing */"
        );
    }

    #[test]
    fn strip_trailing_trivia_of_empty_input_is_empty() {
        assert_eq!(strip_trailing_trivia(""), "");
    }

    #[test]
    fn strip_trailing_trivia_of_only_a_semicolon_is_empty() {
        assert_eq!(strip_trailing_trivia(";"), "");
    }

    // Property-based: the module doc's robustness contract (terminates, never
    // panics, `tok.text` concatenation equals the input) pinned above with a
    // handful of hand-picked strings — fuzz it over arbitrary Unicode input
    // instead of guessing which strings are adversarial. Also checks the
    // stronger, un-stated invariant the hand-picked tests never verified
    // directly: tokens are *contiguous* (each starts exactly where the last
    // ended, no gaps or overlaps) and every offset lands on a char boundary.
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn tokenize_is_total_and_contiguous_on_arbitrary_input(
            chars in prop::collection::vec(any::<char>(), 0..40)
        ) {
            let input: String = chars.into_iter().collect();
            let toks = tokenize(&input);

            let joined: String = toks.iter().map(|t| t.text.as_ref()).collect();
            prop_assert_eq!(joined, input.clone(), "concatenation must equal the input verbatim");

            let mut cursor = 0usize;
            for t in &toks {
                prop_assert_eq!(t.offset, cursor, "no gap/overlap between consecutive tokens");
                prop_assert!(input.is_char_boundary(t.offset), "offset lands on a char boundary");
                prop_assert!(!t.text.is_empty(), "every token covers at least one byte");
                cursor += t.text.len();
            }
            prop_assert_eq!(cursor, input.len(), "tokens cover the input exactly, to the last byte");
        }
    }
}
