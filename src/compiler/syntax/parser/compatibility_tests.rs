//! Differential syntax corpus against the pinned Sil parser, without name resolution.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use silverscript_lang::ast::visit::{AstVisitorMut, NameKind};
use silverscript_lang::ast::{Expr, Statement, parse_expression_ast, parse_function_ast};

use crate::compiler::loader::SourceSet;
use crate::compiler::syntax::AuthoredEntryStatement;

#[derive(Clone, Copy, Debug)]
enum Context {
    Entry,
    GlobalHelper,
    ActorHelper,
}

const CONTEXTS: [Context; 3] = [Context::Entry, Context::GlobalHelper, Context::ActorHelper];

#[derive(Default)]
struct NameSpans(Vec<(String, String, String)>);

impl<'src> AstVisitorMut<'src> for NameSpans {
    fn visit_name(&mut self, name: &mut String, kind: NameKind, span: silverscript_lang::span::Span<'src>) {
        self.0.push((format!("{kind:?}"), name.clone(), span.as_str().to_string()));
    }
}

fn expression_name_spans(expression: &Expr<'_>) -> NameSpans {
    let mut spans = NameSpans::default();
    spans.visit_expr(&mut expression.clone());
    spans
}

fn statement_name_spans(statements: &[Statement<'_>]) -> NameSpans {
    let mut spans = NameSpans::default();
    for statement in statements {
        spans.visit_statement(&mut statement.clone());
    }
    spans
}

impl Context {
    fn source(self, body: &str) -> String {
        match self {
            Self::Entry => format!("actor Probe owns S {{ entry probe() emits none {{ {body} }} }}"),
            Self::GlobalHelper => format!("fn probe() {{ {body} }}"),
            Self::ActorHelper => format!("actor Probe owns S {{ fn probe() {{ {body} }} }}"),
        }
    }
}

fn shape(value: &impl Serialize) -> Value {
    fn strip_spans(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                fields.retain(|name, _| !name.ends_with("span") && !name.ends_with("spans"));
                fields.values_mut().for_each(strip_spans);
            }
            Value::Array(items) => items.iter_mut().for_each(strip_spans),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(value).expect("AST serializes");
    strip_spans(&mut value);
    value
}

// Entry control flow uses Argent nodes. Project only that wrapper structure;
// expressions and ordinary statements remain the parser's original Sil nodes.
fn entry_statement<'src>(statement: &AuthoredEntryStatement<'src>) -> Statement<'src> {
    fn branch<'src>(statement: &AuthoredEntryStatement<'src>) -> Vec<Statement<'src>> {
        match statement {
            AuthoredEntryStatement::Block { statements, .. } => statements.iter().map(entry_statement).collect(),
            _ => vec![entry_statement(statement)],
        }
    }
    fn span<'src>(statement: &AuthoredEntryStatement<'src>) -> silverscript_lang::span::Span<'src> {
        match statement {
            AuthoredEntryStatement::Sil(statement) => statement.span(),
            AuthoredEntryStatement::Block { span, .. } | AuthoredEntryStatement::If { span, .. } => *span,
            _ => panic!("syntax corpus contains an Argent transition"),
        }
    }
    match statement {
        AuthoredEntryStatement::Sil(statement) => statement.as_ref().clone(),
        AuthoredEntryStatement::Block { statements, span } => {
            Statement::Block { body: statements.iter().map(entry_statement).collect(), span: *span }
        }
        AuthoredEntryStatement::If { condition, then_branch, else_branch, span: statement_span } => Statement::If {
            condition: condition.clone(),
            then_branch: branch(then_branch),
            else_branch: else_branch.as_deref().map(branch),
            span: *statement_span,
            then_span: span(then_branch),
            else_span: else_branch.as_deref().map(span),
        },
        _ => panic!("syntax corpus contains an Argent transition"),
    }
}

fn with_body(body: &str, context: Context, check: impl FnOnce(&[Statement<'_>])) {
    let source = context.source(body);
    let sources = SourceSet::discover_inline(PathBuf::from("sil-syntax-corpus.ag"), source)
        .unwrap_or_else(|error| panic!("{context:?}: {body}\n{error}"));
    let program = sources.parse_modules().unwrap_or_else(|error| panic!("{context:?}: {body}\n{error}"));
    let module = &program.modules[0];
    let statements = match context {
        Context::Entry => module.entry_bodies.values().next().expect("entry body").iter().map(entry_statement).collect(),
        Context::GlobalHelper | Context::ActorHelper => module.function_bodies.values().next().expect("helper body").clone(),
    };
    check(&statements);
}

fn assert_expression(expression: &str) {
    let expected = parse_expression_ast(expression).unwrap_or_else(|error| panic!("upstream: {expression}\n{error}"));
    let check = |actual: &Expr<'_>, context: &str| {
        assert_eq!(shape(actual), shape(&expected), "{context}: {expression}");
        assert_eq!(actual.span.as_str(), expected.span.as_str(), "expression span in {context}: {expression}");
        assert_eq!(expression_name_spans(actual).0, expression_name_spans(&expected).0, "name spans in {context}: {expression}");
    };
    for context in CONTEXTS {
        with_body(&format!("require({expression});"), context, |body| {
            let [Statement::Require { expr, .. }] = body else { panic!("expected one require for {expression}") };
            check(expr, &format!("{context:?}"));
        });
    }
    let sources = SourceSet::discover_inline(PathBuf::from("sil-constant-corpus.ag"), format!("const int SAMPLE = {expression};"))
        .expect("constant source discovery");
    let program = sources.parse_modules().unwrap_or_else(|error| panic!("constant: {expression}\n{error}"));
    check(&program.modules[0].const_values[0], "constant");
}

fn assert_statements(body: &str) {
    fn statement_source<'a>(statement: &'a Statement<'_>) -> &'a str {
        let source = statement.span().as_str().trim();
        // Sil's Block span covers its contents; Argent also records the braces.
        if matches!(statement, Statement::Block { .. }) {
            source.strip_prefix('{').and_then(|source| source.strip_suffix('}')).unwrap_or(source).trim()
        } else {
            source
        }
    }
    let reference = format!("function probe() {{ {body} }}");
    let expected = parse_function_ast(&reference).unwrap_or_else(|error| panic!("upstream: {body}\n{error}"));
    for context in CONTEXTS {
        with_body(body, context, |actual| {
            assert_eq!(shape(&actual), shape(&expected.body), "{context:?}: {body}");
            assert_eq!(statement_name_spans(actual).0, statement_name_spans(&expected.body).0, "name spans in {context:?}: {body}");
            for (actual, expected) in actual.iter().zip(&expected.body) {
                assert_eq!(statement_source(actual), statement_source(expected), "statement span in {context:?}: {body}");
            }
        });
    }
}

macro_rules! expressions {
    ($name:ident, [$($expression:expr),+ $(,)?]) => {
        #[test]
        fn $name() {
            for expression in [$($expression),+] {
                assert_expression(expression);
            }
        }
    };
}

expressions!(
    operator_precedence_and_parentheses,
    [
        "left + right * factor",
        "(left + right) * factor",
        "left - right - factor",
        "left / right % factor",
        "-left * right + !flag as int",
        "left == right || left != factor && flag",
        "left < right == flag",
        "left <= right && left >= factor",
        "left & right ^ factor | last",
        "(left | right) & factor",
        "!!flag || !other",
        "--value",
        "left--right",
        "left < -right",
        "((value))",
        "left + ((right))",
    ]
);

expressions!(
    compact_negative_comparisons,
    ["left<-right", "left<-right * factor + last", "left<-(right + factor)", "left<-1", "left<--1"]
);

expressions!(
    numeric_temporal_and_string_literals,
    [
        "0",
        "9223372036854775807",
        "-9223372036854775807",
        "1_000_000",
        "2e3",
        "2E1_0",
        "0x7f",
        "0X0123",
        "5 litras + 2 grains + 1 kas",
        "2 seconds + 3 minutes",
        "4 hours + 2 days + 1 weeks",
        "date(\"2026-01-02T03:04:05\")",
        r#""a\nb\t\"c\\d\u0041""#,
        r#""שלום""#,
        "true && false",
    ]
);

expressions!(
    calls_casts_and_byte_sequences,
    [
        "helper()",
        "helper(value, other,)",
        "helper(other(value), value + 1)",
        "value as byte[8]",
        "(flag as int) + 1",
        "((value)) as byte[4]",
        "byte(128)",
        "int(value)",
        "signed(value)",
        "unsigned(value)",
        "byte[](0x)",
        "byte[_](0xaabb)",
        "byte[2]((0x1122))",
        "byte[2](0x1122,)",
        "byte[2](((0x1122)),)",
        "byte[]((0x),)",
        "byte[2]((0x1122 + 0))",
        "byte[2]((0x1122), 1)",
        "sha256(byte[](0x112233))",
        "new ScriptPubKeyP2SH(hash)",
    ]
);

expressions!(
    arrays_structs_and_postfix_chains,
    [
        "int[]{}",
        "int[2]{value, value + 1,}",
        "int[_]{1, 2}",
        "int[COUNT]{1, 2}",
        "int[2][1]{int[2]{1, 2}}",
        "Pair { left: value, right: value + 1, }",
        "values[index + 1]",
        "values[outer[inner[index]]]",
        "(values)[index].field",
        "pair.left",
        "(pair).left",
        "buffer.length",
        "buffer.append(value, other,).length",
        "buffer.slice(1, 2).length",
        "buffer.split(1).0",
        "buffer.split(1).1.length",
        "buffer.split(1).1.split(2).0",
        "(buffer).slice(1, 2).length",
        "(value as byte[8])[0]",
    ]
);

expressions!(
    transaction_introspection_and_native_calls,
    [
        "this.activeInputIndex",
        "this.activeScriptPubKey",
        "this.bytecodeSize",
        "this.bytecodeSizeDataPrefix",
        "tx.version",
        "tx.inputs.length + tx.outputs.length",
        "tx.inputs[index].value",
        "tx.inputs[index].scriptPubKey",
        "tx.inputs[index].sigScript",
        "tx.inputs[index].outpointTxId",
        "tx.inputs[index].outpointIndex",
        "tx.outputs[index].value",
        "tx.outputs[index].scriptPubKey",
        "OpBin2Num(byte[](0x01))",
        "readInputState(this.activeInputIndex)",
        "readInputStateWithTemplate(index, prefix_len, suffix_len, hash)",
        "checkSig(signature, key)",
        "checkSigEcdsa(signature, key)",
        "checkMsgSig(signature, hash, key)",
        "checkMsgSigEcdsa(signature, hash, key)",
    ]
);

#[test]
fn fixed_byte_sequence_casts_match_upstream() {
    for (ty, length) in [("byte[32]", 32), ("pubkey", 32), ("sig", 65), ("datasig", 64)] {
        let hex = format!("0x{}", "11".repeat(length));
        for argument in [hex.clone(), format!("({hex})"), format!("(({hex}))")] {
            for comma in ["", ","] {
                assert_expression(&format!("{ty}({argument}{comma})"));
            }
        }
    }
}

#[test]
fn declaration_assignment_and_destructuring_statements_match_upstream() {
    for body in [
        "int result;",
        "int result = value + 1; result = result * 2;",
        "int constant LOCAL = 2;",
        "byte[2] marker = byte[_](0xaabb);",
        "int[COUNT] values = int[COUNT]{1, 2};",
        "Pair pair = Pair { left: 1, right: 2 };",
        "int left, int right = buffer.split(1);",
        "(byte[] left, byte[] right,) = buffer.split(1);",
        "(int result,) = helper(value,);",
        "(int left, bool right) = helper(value);",
        "Pair { left: int first, right: int second, } = pair;",
        "Pair { left: int first, right: int second, } = readInputState(index);",
    ] {
        assert_statements(body);
    }
}

#[test]
fn control_flow_and_terminal_statements_match_upstream() {
    for body in [
        "{ int result = value; require(result > 0); }",
        "if (flag) { require(value > 0); }",
        "if (flag) { require(value > 0); } else { require(value < 0); }",
        "if (flag) { require(value > 0); } else if (other) { require(value == 0); } else { require(value < 0); }",
        "for (i, 0, count, MAX) { require(values[i] > 0); } require(count > 0);",
        "for (i, 0, count, MAX) require(values[i] > 0);",
        "for (i, 0, 2, 2) { if (flag) { for (j, 0, 2, 2) { require(i + j >= 0); } } }",
        "return value;",
        "return (value,);",
        "return (left, right);",
        "return ();",
        "helper(value,);",
        "console.log(value, \"text\",);",
        "require(flag, \"failure\\nmessage\");",
        "require(this.ageDaa >= 10, \"age\");",
        "require(tx.daa >= 10);",
        "require(tx.time >= date(\"2026-01-01T00:00:00\") + 1 days);",
        "validateOutputState(index, next);",
        "validateOutputStateWithTemplate(index, next, prefix, suffix, hash);",
        "validateOutputStateWithInputTemplate(index, next, input, prefix_len, suffix_len, hash);",
        "g16.verify(key, proof, input);",
        "r0.g16.verify(journal, proof, image);",
        "r0.succinct.verify(claim, index, digests, seal, journal, image, control);",
        "r0.succinct.blake2b.verify(claim, index, digests, seal, journal, image, control);",
        "r0.succinct.poseidon2.verify(claim, index, digests, seal, journal, image, control);",
        "r0.succinct.sha256.verify(claim, index, digests, seal, journal, image, control);",
    ] {
        assert_statements(body);
    }
}

#[test]
fn shared_malformed_syntax_is_rejected_by_both_parsers() {
    for body in [
        "int result = ;",
        "require(value + );",
        "require(flag)",
        "if (flag { require(true); }",
        "for (i, 0, 2) { require(true); }",
        "int[2] data = int[2]{1};",
        "int value = 0x;",
        "return value +;",
        "Pair { left: int } = pair;",
        "require(data.split(1).2.length > 0);",
    ] {
        assert!(parse_function_ast(&format!("function probe() {{ {body} }}")).is_err(), "upstream accepted: {body}");
        for context in CONTEXTS {
            let rejected = match SourceSet::discover_inline(PathBuf::from("invalid-sil-corpus.ag"), context.source(body)) {
                Ok(sources) => sources.parse_modules().is_err(),
                Err(_) => true,
            };
            assert!(rejected, "{context:?} accepted: {body}");
        }
    }
}
