use std::path::PathBuf;

use serde_json::Value;
use silverscript_lang::ast::{ExprKind, parse_expression_ast};

use crate::compiler::loader::SourceSet;

fn without_spans(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|name, _| !name.ends_with("span"));
            for value in fields.values_mut() {
                without_spans(value);
            }
        }
        Value::Array(items) => {
            for value in items {
                without_spans(value);
            }
        }
        _ => {}
    }
}

fn assert_upstream_expression_shape(expression: &str) {
    let source = format!("const int SAMPLE = {expression};");
    let sources = SourceSet::discover_inline(PathBuf::from("expr.ag"), source).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let mut parsed = serde_json::to_value(&program.modules[0].const_values[0]).expect("Argent AST serializes");
    let mut upstream =
        serde_json::to_value(parse_expression_ast(expression).expect("upstream expression")).expect("Sil AST serializes");
    without_spans(&mut parsed);
    without_spans(&mut upstream);
    assert_eq!(parsed, upstream, "expression: {expression}");
}

#[test]
fn constant_expressions_match_upstream_ast_shapes() {
    for expression in [
        "1 + 2 * 3",
        "1_000",
        "1e3",
        "2 hours",
        "0x01",
        "-42",
        "true && false",
        "Ledger { balance: 3 }",
        "helper(1, 2)",
        "int[2] {1, 2}",
        "value as byte[32]",
        "tx.inputs.length",
        "tx.outputs[index].value",
        "buffer.append(0x01)",
        "buffer.slice(1, 2)",
        "buffer.split(1).1",
        "byte[_](0xaabb)",
        "byte[2]((0x1122))",
        "byte[2](0x1122,)",
        "byte[2](((0x1122)),)",
        "byte[]((0x),)",
        "byte[2]((0x1122 + 0))",
        "byte[2]((0x1122), 1)",
        "date(\"2026-01-02T03:04:05\")",
    ] {
        assert_upstream_expression_shape(expression);
    }
}

#[test]
fn fixed_byte_hex_casts_match_upstream_with_parentheses_and_trailing_commas() {
    for (ty, len) in [("byte[32]", 32), ("pubkey", 32), ("sig", 65), ("datasig", 64)] {
        let hex = format!("0x{}", "11".repeat(len));
        for argument in [hex.clone(), format!("({hex})"), format!("(({hex}))")] {
            for comma in ["", ","] {
                assert_upstream_expression_shape(&format!("{ty}({argument}{comma})"));
            }
        }
    }
}

#[test]
fn qualified_calls_keep_the_authored_path_and_span() {
    let source = "const int SAMPLE = lib /*gap*/ :: helper(1 + 2);";
    let sources = SourceSet::discover_inline(PathBuf::from("qualified.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let expr = &program.modules[0].const_values[0];
    let ExprKind::Call { name, args, .. } = &expr.kind else {
        panic!("qualified call must be a Sil call");
    };
    assert_eq!(name, "lib::helper");
    assert_eq!(args.len(), 1);
    assert_eq!(expr.span.as_str(), "lib /*gap*/ :: helper(1 + 2)");
}

#[test]
fn parenthesized_composites_keep_their_complete_source_spans() {
    for expression in [
        "(left + right) * factor",
        "left + (right * factor)",
        "left + ((right))",
        "(value).field",
        "(values)[index].length",
        "(buffer).slice(1, 2).length",
        "((value)) as byte[4]",
        "-((value))",
        "((value))",
    ] {
        let source = format!("const int SAMPLE = {expression};");
        let sources = SourceSet::discover_inline(PathBuf::from("parenthesized.ag"), source).expect("source discovery");
        let program = sources.parse_modules().expect("source parser");
        let parsed = &program.modules[0].const_values[0];
        let upstream = parse_expression_ast(expression).expect("upstream expression");
        assert_eq!(parsed.span.as_str(), upstream.span.as_str(), "expression: {expression}");
    }
}

#[test]
fn empty_hex_byte_array_literal_has_zero_elements() {
    for (literal, dimension) in [
        ("byte[](0x)", silverscript_lang::ast::ArrayDim::Dynamic),
        ("byte[0](0x)", silverscript_lang::ast::ArrayDim::Fixed(0)),
        ("byte[_](0X)", silverscript_lang::ast::ArrayDim::Fixed(0)),
    ] {
        let source = format!("const int SAMPLE = blake3({literal});");
        let sources = SourceSet::discover_inline(PathBuf::from("empty-hex.ag"), source).expect("source discovery");
        let program = sources.parse_modules().expect("empty hex byte array parses");
        let ExprKind::Call { args, .. } = &program.modules[0].const_values[0].kind else {
            panic!("blake3 must be a call");
        };
        let ExprKind::Array { type_ref, values, .. } = &args[0].kind else {
            panic!("{literal} must be a byte array");
        };
        assert_eq!(type_ref.array_dims, [dimension]);
        assert!(values.is_empty(), "{literal}");
    }

    for (literal, message) in [
        ("byte[1](0x)", "size mismatch"),
        ("byte[1]((0x),)", "size mismatch"),
        ("pubkey((0x),)", "size mismatch"),
        ("0x", "invalid hex literal"),
    ] {
        let sources = SourceSet::discover_inline(PathBuf::from("invalid-empty-hex.ag"), format!("const int SAMPLE = {literal};"))
            .expect("source discovery");
        let error = sources.parse_modules().expect_err("empty hex literal must be invalid here");
        assert!(error.to_string().contains(message), "{literal}: {error}");
    }
}

#[test]
fn malformed_constant_expression_reports_the_authored_file() {
    let source = "const int SAMPLE = 1 + ;";
    let sources = SourceSet::discover_inline(PathBuf::from("invalid.ag"), source.to_string()).expect("source discovery");
    let error = sources.parse_modules().expect_err("missing right operand");
    assert_eq!(error.path.as_deref(), Some(std::path::Path::new("invalid.ag")));
    assert_eq!(error.location.expect("source location").byte_offset, source.find(';').unwrap());
}
