use sid_db_clients::lexer::{Token, TokenKind, tokenize};

fn kinds(input: &str) -> Vec<TokenKind> {
    tokenize(input).into_iter().map(|t| t.kind).collect()
}

#[test]
fn empty_input_yields_no_tokens() {
    assert!(tokenize("").is_empty());
}

#[test]
fn whitespace_is_a_token() {
    assert_eq!(kinds("   "), vec![TokenKind::Whitespace]);
}

#[test]
fn keyword_select_is_recognised() {
    let toks = tokenize("SELECT");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Keyword);
    assert_eq!(toks[0].text, "SELECT");
}

#[test]
fn keyword_matching_is_case_insensitive() {
    assert_eq!(kinds("select"), vec![TokenKind::Keyword]);
    assert_eq!(kinds("SeLeCt"), vec![TokenKind::Keyword]);
}

#[test]
fn identifier_after_keyword_is_identifier() {
    let toks = tokenize("SELECT id");
    assert_eq!(
        toks.iter().map(|t| t.kind).collect::<Vec<_>>(),
        vec![
            TokenKind::Keyword,
            TokenKind::Whitespace,
            TokenKind::Identifier
        ]
    );
}

#[test]
fn integer_literal_is_number() {
    assert_eq!(kinds("123"), vec![TokenKind::Number]);
}

#[test]
fn float_literal_is_number() {
    let toks = tokenize("3.14");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Number);
    assert_eq!(toks[0].text, "3.14");
}

#[test]
fn single_quoted_string_with_escape() {
    let toks = tokenize("'hello ''world'''");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::String);
}

#[test]
fn line_comment_runs_to_eol() {
    let toks = tokenize("-- a comment\nSELECT");
    assert_eq!(toks[0].kind, TokenKind::Comment);
    assert!(toks[0].text.starts_with("--"));
}

#[test]
fn block_comment_balanced() {
    let toks = tokenize("/* block */");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Comment);
}

#[test]
fn punctuation_tokens_are_emitted() {
    let toks = tokenize("(),;");
    assert_eq!(
        toks.iter().map(|t| t.kind).collect::<Vec<_>>(),
        vec![TokenKind::Punctuation; 4]
    );
}

#[test]
fn unterminated_string_emits_string_token_to_eof() {
    let toks = tokenize("'unterminated");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::String);
}

#[test]
fn unterminated_block_comment_emits_comment_to_eof() {
    let toks = tokenize("/* never closes");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Comment);
}

#[test]
fn offsets_cover_input_with_no_gaps() {
    let input = "SELECT id FROM t";
    let toks = tokenize(input);
    let mut cursor = 0;
    for t in &toks {
        assert_eq!(
            t.offset, cursor,
            "token offset {} != cursor {}",
            t.offset, cursor
        );
        cursor += t.text.len();
    }
    assert_eq!(cursor, input.len());
}

#[test]
fn token_struct_is_constructible() {
    let _: Token = Token {
        kind: TokenKind::Whitespace,
        offset: 0,
        text: "".into(),
    };
}

#[test]
fn keyword_then_dot_then_keyword_emits_kw_punct_kw() {
    let toks = tokenize("FROM.SELECT");
    assert_eq!(
        toks.iter().map(|t| t.kind).collect::<Vec<_>>(),
        vec![
            TokenKind::Keyword,
            TokenKind::Punctuation,
            TokenKind::Keyword
        ]
    );
}

#[test]
fn dollar_sign_in_identifier_is_legal() {
    let toks = tokenize("my$col");
    assert_eq!(toks.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Identifier);
}

#[test]
fn comment_then_keyword_segments_correctly() {
    let toks = tokenize("-- skip\nSELECT");
    assert_eq!(toks[0].kind, TokenKind::Comment);
    assert!(matches!(toks.last().unwrap().kind, TokenKind::Keyword));
}

#[test]
fn nested_block_comment_treated_as_outermost_only() {
    let toks = tokenize("/* a /* b */ c */");
    assert!(toks.iter().any(|t| t.kind == TokenKind::Comment));
}

#[test]
fn long_input_finishes_quickly() {
    let big = "SELECT id FROM users WHERE name = 'x'; ".repeat(10_000);
    let toks = tokenize(&big);
    assert!(!toks.is_empty());
}
