use std::path::PathBuf;

use silverscript_lang::ast::{Statement, parse_function_ast};

use crate::compiler::loader::SourceSet;

#[test]
fn helper_statements_are_parsed_from_source() {
    let source = "fn helper(int value) -> int { int result = value + 1; require(result > 0); return result; }";
    let sources = SourceSet::discover_inline(PathBuf::from("helper.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let body = program.modules[0].function_bodies.values().next().expect("helper body AST");
    let upstream =
        parse_function_ast("function helper(int value): int { int result = value + 1; require(result > 0); return result; }")
            .expect("upstream helper AST");
    assert_eq!(body.len(), upstream.body.len());
    assert!(matches!(body[0], Statement::VariableDefinition { .. }));
    assert!(matches!(body[1], Statement::Require { .. }));
    assert!(matches!(body[2], Statement::Return { .. }));
    assert_eq!(body[0].span().as_str(), "int result = value + 1;");
}

#[test]
fn lock_requirements_keep_their_sil_variant_and_authored_target() {
    let source = "fn check() { require(tx.time >= 1, \"later\"); }";
    let sources = SourceSet::discover_inline(PathBuf::from("lock.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let body = program.modules[0].function_bodies.values().next().expect("helper body AST");
    let Statement::RequireTxTime { target_span, message, .. } = &body[0] else {
        panic!("transaction time requirement needs the pinned Sil variant");
    };
    assert_eq!(target_span.as_str(), "tx.time");
    assert_eq!(message.as_deref(), Some("later"));
}

#[test]
fn dotted_sil_builtins_keep_their_call_name() {
    let source = "fn check() { g16.verify(proof, key); }";
    let sources = SourceSet::discover_inline(PathBuf::from("builtin.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let body = program.modules[0].function_bodies.values().next().expect("helper body AST");
    let Statement::FunctionCall { name, name_span, .. } = &body[0] else {
        panic!("dotted builtin must be a call");
    };
    assert_eq!(name, "g16.verify");
    assert_eq!(name_span.as_str(), "g16.verify");
}

#[test]
fn nested_control_flow_and_destructuring_use_direct_sil_statements() {
    let source = r#"
        fn flow(int value) -> int {
            int left, int right = pair(value);
            for (i, 0, 2, 2) {
                if (i > 0) { require(value > i); }
            }
            return left;
        }
    "#;
    let sources = SourceSet::discover_inline(PathBuf::from("flow.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let body = program.modules[0].function_bodies.values().next().expect("helper body AST");
    assert!(matches!(&body[0], Statement::TupleAssignment { .. }));
    let Statement::For { body: loop_body, .. } = &body[1] else {
        panic!("loop must remain a Sil statement");
    };
    assert!(matches!(&loop_body[0], Statement::If { then_branch, .. } if matches!(&then_branch[0], Statement::Require { .. })));
    assert!(matches!(&body[2], Statement::Return { .. }));
    assert_eq!(
        body[1].span().as_str().trim(),
        "for (i, 0, 2, 2) {\n                if (i > 0) { require(value > i); }\n            }"
    );
}
