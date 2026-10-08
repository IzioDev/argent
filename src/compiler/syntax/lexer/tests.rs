use super::*;

#[test]
fn skips_nested_block_comments() {
    let tokens = lex_argent_source("before /* outer /* inner */ outer */ after").expect("nested block comments must lex");
    let identifiers = tokens
        .iter()
        .filter_map(|token| match &token.kind {
            TokenKind::Ident(value) => Some(value.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(identifiers, ["before", "after"]);
}

#[test]
fn reports_unterminated_block_comment_location() {
    let err = lex_argent_source("before\n  /* never closed").expect_err("unterminated block comment must be rejected");

    assert_eq!(err.to_string(), "2:3: unterminated block comment");
}

#[test]
fn rejects_reserved_generated_namespace_identifier() {
    for source in [
        "state gen__state {}",
        "state Gen__State {}",
        "const int gen__module__1__LIMIT = 2;",
        "fn total() -> int { int gen__module__1__LIMIT = 9; return gen__module__1__LIMIT; }",
    ] {
        let err = lex_argent_source(source).expect_err("reserved generated namespace must be rejected");
        assert!(err.to_string().contains("reserved generated namespace"), "unexpected error: {err}");
    }
}

#[test]
fn rejects_legacy_covenant_id_keyword() {
    let err = lex_argent_source("covid value;").expect_err("the legacy covenant id keyword must be rejected");
    assert!(err.to_string().contains("`covid` was renamed to `cov_id`"), "unexpected error: {err}");
}

#[test]
fn source_tokens_preserve_extended_literals_and_utf8() {
    let source = r#"0x00 1_000 2e3 ^ "Caf\u00e9\n""#;
    let tokens = lex_argent_source(source).expect("Argent source tokenization");
    assert_eq!(tokens[0].kind, TokenKind::Number("0x00".to_string()));
    assert_eq!(tokens[1].kind, TokenKind::Number("1_000".to_string()));
    assert_eq!(tokens[2].kind, TokenKind::Number("2e3".to_string()));
    assert_eq!(tokens[3].kind, TokenKind::Symbol('^'));
    assert_eq!(tokens[4].kind, TokenKind::Str("Café\n".to_string()));
    assert_eq!(&source[tokens[4].span.start..tokens[4].span.end], r#""Caf\u00e9\n""#);
}
