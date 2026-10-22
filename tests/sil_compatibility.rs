//! Compile ordinary Sil through Argent and an independent upstream Sil source,
//! then execute both valid and invalid result claims through Argent's runtime.

use argent::builder::{EntryCall, TxBuilder, TxContext, args, state};
use kaspa_consensus_core::{
    Hash,
    tx::{TransactionId, TransactionOutpoint},
};
use silverscript_lang::{
    ast::Expr,
    compiler::{CompileOptions, compile_contract, sil_abi_artifact_from_compiled},
};

#[derive(Clone, Copy, Debug)]
enum Context {
    Entry,
    GlobalHelper,
    ActorHelper,
}

const CONTEXTS: [Context; 3] = [Context::Entry, Context::GlobalHelper, Context::ActorHelper];

struct Case<'a> {
    name: &'a str,
    statements: &'a str,
    result: &'a str,
    // Explicit reference text is needed only for Argent-only surface syntax.
    sil_statements: Option<&'a str>,
    sil_result: Option<&'a str>,
    samples: &'a [(i64, i64)],
}

fn compile_source(label: impl AsRef<std::path::Path>, source: impl Into<String>) -> argent::Result<argent::artifact::Artifact> {
    let output = tempfile::tempdir().expect("isolated corpus output directory");
    argent::build_inline(label, source, output.path())
}

fn sources(case: &Case<'_>, context: Context) -> (String, String) {
    let check = format!("require(({}) == expected);", case.result);
    let (global, actor, entry) = match context {
        Context::Entry => (String::new(), String::new(), format!("{} {check}", case.statements)),
        Context::GlobalHelper | Context::ActorHelper => {
            let function = format!("fn compute(int value) -> int {{ {} return ({}); }}", case.statements, case.result);
            let entry = "require(compute(value) == expected);".to_string();
            if matches!(context, Context::GlobalHelper) { (function, String::new(), entry) } else { (String::new(), function, entry) }
        }
    };
    let argent = format!(
        r#"
        const int COUNT = 2;
        state S {{ int n; }}
        state Pair {{ int left; int right; }}
        fn add_one(int value) -> int {{ return value + 1; }}
        fn assert_nonnegative(int value) {{ require(value >= 0); }}
        {global}
        actor Probe owns S {{
            {actor}
            entry check(int value, int expected) emits none {{ {entry} }}
        }}
        app Compatibility {{ actor Probe; }}
    "#
    );
    // This reference never uses Argent's emitted Sil or its AST lowerers. The
    // ordinary statements are parsed and compiled directly by upstream Sil.
    let statements = case.sil_statements.unwrap_or(case.statements);
    let result = case.sil_result.unwrap_or(case.result);
    let (function, entry) = match context {
        Context::Entry => (String::new(), format!("{statements} require(({result}) == expected);")),
        Context::GlobalHelper | Context::ActorHelper => (
            format!("function compute(int value): int {{ {statements} return ({result}); }}"),
            "require(compute(value) == expected);".to_string(),
        ),
    };
    let sil = format!(
        r#"
        pragma silverscript ^0.1.0;
        contract Probe(int init_n) {{
            struct Pair {{ int left; int right; }}
            int constant COUNT = 2;
            int n = init_n;
            function add_one(int value): int {{ return value + 1; }}
            function assert_nonnegative(int value) {{ require(value >= 0); }}
            {function}
            entry check(int value, int expected) {{
                require(OpAuthOutputCount(this.activeInputIndex) == 0);
                {entry}
            }}
        }}
    "#
    );
    (argent, sil)
}

fn check_case(case: Case<'_>) {
    for context in CONTEXTS {
        let (source, reference) = sources(&case, context);
        let constructor_args = [Expr::int(0)];
        let compiled = compile_contract(&reference, &constructor_args, CompileOptions::default())
            .unwrap_or_else(|error| panic!("upstream {} / {context:?}: {error}\n{reference}", case.name));
        let expected = sil_abi_artifact_from_compiled(&compiled, &constructor_args).expect("upstream ABI");
        let artifact = compile_source(format!("sil-corpus-{}-{context:?}.ag", case.name), source.clone())
            .unwrap_or_else(|error| panic!("{} / {context:?}: {error}\n{source}", case.name));
        let actual = artifact.sil_abi.contract("Probe").expect("Argent contract");
        assert_eq!(actual, expected.contract("Probe").expect("upstream contract"), "{} / {context:?}: compiled ABI", case.name);
        assert_eq!(artifact.sil_abi.structs, expected.structs, "{} / {context:?}: struct ABI", case.name);

        let builder = TxBuilder::new(&artifact).expect("corpus artifact verifies");
        let initial = state! { n: 7 };
        let utxo = builder
            .covenant_utxo("Probe", initial.clone(), 1_000, 0, false, Some(Hash::from_bytes([0x43; 32])))
            .expect("corpus input state builds");
        let outpoint = TransactionOutpoint { transaction_id: TransactionId::from_bytes([0x44; 32]), index: 0 };
        for &(value, result) in case.samples {
            for claimed in [result, result + 1] {
                let transaction_context = TxContext::new().actor_input(
                    "Probe",
                    initial.clone(),
                    EntryCall::new("check").args(args![value, claimed]),
                    outpoint,
                    utxo.clone(),
                    0,
                );
                let built = builder.build(&transaction_context);
                assert_eq!(
                    built.is_ok(),
                    claimed == result,
                    "{} / {context:?}: input {value}, claim {claimed}, expected {result}: {built:?}",
                    case.name
                );
                if let Err(error) = built {
                    assert!(
                        matches!(error, argent::builder::BuilderError::InputScript { .. }),
                        "rejection must come from execution: {error}"
                    );
                }
            }
        }
    }
}

macro_rules! case {
    ($name:ident, $statements:expr, $result:expr, $samples:expr) => {
        #[test]
        fn $name() {
            check_case(Case {
                name: stringify!($name),
                statements: $statements,
                result: $result,
                sil_statements: None,
                sil_result: None,
                samples: $samples,
            });
        }
    };
}

case!(arithmetic_precedence, "", "value + 2 * 3", &[(0, 6), (5, 11)]);
case!(arithmetic_parentheses, "", "(value + 2) * 3", &[(0, 6), (5, 21)]);
case!(subtraction_and_unary_negation, "", "value--2 - -3", &[(0, 5), (5, 10)]);
case!(division_and_remainder, "", "value / 2 * 3 + value % 2", &[(0, 0), (5, 7)]);
case!(bitwise_precedence, "byte b = value as byte;", "unsigned(b & byte(7) ^ byte(1) | byte(2))", &[(0, 3), (5, 6)]);
case!(boolean_normalization, "", "(value > 2 && value < 8 || value == 0) as int", &[(0, 1), (5, 1), (9, 0)]);
case!(compact_negative_comparison, "", "(-value<-1) as int", &[(0, 0), (5, 1)]);
case!(numeric_units_and_exponents, "", "value + 1_000 + 2e1 + 1 grains", &[(0, 101_020), (5, 101_025)]);
case!(direct_and_nested_calls, "", "add_one(add_one(value))", &[(0, 2), (5, 7)]);
case!(void_helper_call, "assert_nonnegative(value);", "value", &[(0, 0), (5, 5)]);
case!(callable_name_does_not_capture_a_local, "int add_one = value;", "add_one(add_one)", &[(0, 1), (5, 6)]);
case!(
    constructor_name_does_not_capture_a_local,
    "int Pair = value; Pair pair = Pair { left: Pair, right: 1 };",
    "pair.left",
    &[(0, 0), (5, 5)]
);

case!(
    assignment_and_blocks,
    r#"
    int result = value;
    { int increment = 2; result = result + increment; }
    result = result * 2;
"#,
    "result",
    &[(0, 4), (5, 14)]
);

case!(
    conditional_assignment,
    r#"
    int result = value;
    if (value == 0) { result = 10; }
    else if (value < 5) { result = result + 1; }
    else { result = result * 2; }
"#,
    "result",
    &[(0, 10), (3, 4), (5, 10)]
);

case!(
    bounded_loop_and_following_statement,
    r#"
    int result = 0;
    for (i, 0, value, 8) { result = result + i; }
    result = result + 1;
"#,
    "result",
    &[(0, 1), (3, 4), (5, 11)]
);

case!(
    nested_loop_and_branch,
    r#"
    int result = 0;
    for (i, 0, 2, 2) {
        if (value > 0) { for (j, 0, 2, 2) { result = result + value + i + j; } }
    }
"#,
    "result",
    &[(0, 0), (5, 24)]
);

case!(
    dynamic_arrays_and_append,
    r#"
    int[] values;
    values = values.append(value, value + 1);
"#,
    "values[0] + values[1] + values.length",
    &[(0, 3), (5, 13)]
);

case!(
    fixed_arrays_and_named_dimensions,
    "int[COUNT] values = int[COUNT]{value, value + 1,};",
    "values[0] + values[1]",
    &[(0, 1), (5, 11)]
);
case!(nested_arrays, "int[2][1] rows = int[2][1]{int[2]{value, value + 1}};", "rows[0][1]", &[(0, 1), (5, 6)]);
case!(fixed_to_dynamic_array_cast, "int[] values = int[](int[2]{value, value + 1});", "values[1]", &[(0, 1), (5, 6)]);
case!(parenthesized_hex_cast, "byte[2] bytes = byte[2](((0x1122)),);", "bytes.length + value", &[(0, 2), (5, 7)]);
case!(empty_hex_array, "byte[] bytes = byte[]((0x),);", "bytes.length + value", &[(0, 0), (5, 5)]);
case!(named_width_as_cast, "byte[COUNT] bytes = value as byte[COUNT];", "OpBin2Num(byte[](bytes))", &[(0, 0), (5, 5)]);

case!(
    tuple_destructuring,
    "(byte[] left, byte[] right,) = byte[](0x112233).split(1);",
    "left.length * 10 + right.length + value",
    &[(0, 12), (5, 17)]
);
case!(
    unparenthesized_tuple_destructuring,
    "byte[] left, byte[] right = byte[](0x112233).split(1);",
    "left.length * 10 + right.length + value",
    &[(0, 12), (5, 17)]
);
case!(
    struct_literals_and_destructuring,
    r#"
    Pair pair = Pair { left: value, right: value + 1, };
    Pair { left: int first, right: int second, } = pair;
"#,
    "first + second",
    &[(0, 1), (5, 11)]
);

case!(
    postfix_chains,
    "byte[] bytes = byte[](0x11223344);",
    "bytes.split(1).1.slice(0, 2).append(byte(5)).length + value",
    &[(0, 3), (5, 8)]
);
case!(unicode_and_escaped_strings, r#"string text = "a\nb\t\"c\\d\u0041שלום";"#, "text.length + value", &[(0, 17), (5, 22)]);
case!(temporal_literals, "temporal delay = 1 days + 2 hours;", "(delay == 26 hours) as int + value", &[(0, 1), (5, 6)]);
case!(
    native_introspection,
    "require(tx.inputs.length == 1);",
    "this.activeInputIndex + tx.inputs[0].value - 1000 + value",
    &[(0, 0), (5, 5)]
);
case!(scalar_byte_signedness, "byte b = byte(255);", "unsigned(b) + signed(b) + value", &[(0, 128), (5, 133)]);
case!(method_spelling_is_not_a_reserved_variable, "int co_spent = value + 1;", "co_spent", &[(0, 1), (5, 6)]);

#[test]
fn contextual_braced_state_constructor_matches_explicit_sil_constructor() {
    check_case(Case {
        name: "contextual_braced_state_constructor",
        statements: "Pair pair = { left: value, right: value + 1 };",
        sil_statements: Some("Pair pair = Pair { left: value, right: value + 1 };"),
        sil_result: None,
        result: "pair.left + pair.right",
        samples: &[(0, 1), (5, 11)],
    });
}

#[test]
fn covenant_co_spend_method_matches_explicit_sil_introspection() {
    check_case(Case {
        name: "covenant_co_spend_method",
        statements: "cov_id id = cov_id(OpInputCovenantId(this.activeInputIndex));",
        result: "id.co_spent() as int + value",
        sil_statements: Some("byte[32] id = OpInputCovenantId(this.activeInputIndex);"),
        sil_result: Some("(OpCovInputCount(id) > 0) as int + value"),
        samples: &[(0, 1), (5, 6)],
    });
}

#[test]
fn shared_semantic_errors_are_rejected_by_both_compilers() {
    for (name, statements, result) in [
        ("boolean-arithmetic", "", "true + value"),
        ("missing-call-argument", "", "add_one()"),
        ("duplicate-binding", "int value = 1;", "value"),
        ("constant-assignment", "int constant LOCAL = 2; LOCAL = 3;", "LOCAL"),
        ("loop-variable-assignment", "for (i, 0, 2, 2) { i = 1; }", "value"),
        ("short-hex-array", "byte[2] bytes = byte[2](0x11);", "bytes.length"),
        ("boolean-array-index", "int[2] values = int[2]{1, 2};", "values[true]"),
        ("missing-struct-field", "Pair pair = Pair { left: value };", "pair.left"),
    ] {
        let case = Case { name, statements, result, sil_statements: None, sil_result: None, samples: &[] };
        for context in CONTEXTS {
            let (source, reference) = sources(&case, context);
            assert!(
                compile_contract(&reference, &[Expr::int(0)], CompileOptions::default()).is_err(),
                "upstream accepted {name} / {context:?}"
            );
            assert!(compile_source(format!("invalid-{name}-{context:?}.ag"), source).is_err(), "Argent accepted {name} / {context:?}");
        }
    }
}

#[test]
fn unsupported_sil_ternary_is_an_explicit_argent_boundary() {
    let reference = "contract Probe(int init_n) { int n = init_n; entry check(int value) { require((value > 0 ? value : 0) >= 0); } }";
    compile_contract(reference, &[Expr::int(0)], CompileOptions::default()).expect("pinned Sil supports ternary");
    let error = compile_source("ternary-policy.ag", "state S{int n;} actor Probe owns S{entry check(int value) emits none{require((value > 0 ? value : 0) >= 0);}} app Test{actor Probe;}")
        .expect_err("Argent deliberately does not tokenize ternary expressions");
    assert!(error.to_string().contains("unexpected character `?`"), "{error}");
}

#[test]
fn entry_only_and_reserved_operations_have_explicit_diagnostics() {
    for (name, source, message) in [
        ("global-self", "fn helper(int self) -> int { return self; }", "identifier `self` is reserved"),
        (
            "actor-self",
            "actor Probe owns S { fn helper(int self) -> int { return self; } entry check() emits none {} }",
            "identifier `self` is reserved",
        ),
        (
            "global-state",
            "fn helper(S input) -> S { return state(input); }",
            "input-state reconstruction is only available in entry bodies",
        ),
        (
            "actor-state",
            "actor Probe owns S { fn helper() -> S { return state(self); } entry check() emits none {} }",
            "input-state reconstruction is only available in entry bodies",
        ),
        (
            "physical-state",
            "actor Probe owns S { entry check() emits next: Probe { unrestricted(next.value); become next <- Probe(State { n: 1 }); } }",
            "physical `State` is compiler-owned",
        ),
        (
            "unbraced-entry-if",
            "actor Probe owns S { entry check(int value) emits none { if (value > 0) require(value > 1); } }",
            "retained entry body has no direct Sil AST lowering",
        ),
    ] {
        let actor = if source.contains("actor Probe") { "" } else { "actor Probe owns S { entry check() emits none {} }" };
        let source = format!("state S {{ int n; }} {source} {actor} app Test {{ actor Probe; }}");
        let error = compile_source(format!("policy-{name}.ag"), source)
            .err()
            .unwrap_or_else(|| panic!("{name}: expected rejection containing {message:?}"));
        assert!(error.to_string().contains(message), "{name}: expected {message:?}, got {error}");
    }
}

#[test]
fn co_spent_function_name_is_reserved_in_both_helper_scopes() {
    for source in [
        "state S { int n; } fn co_spent(int value) -> bool { return value > 0; } actor Probe owns S { entry check() emits none {} } app Test { actor Probe; }",
        "state S { int n; } actor Probe owns S { fn co_spent(int value) -> bool { return value > 0; } entry check() emits none {} } app Test { actor Probe; }",
    ] {
        let error = compile_source("reserved-co-spent.ag", source)
            .err()
            .unwrap_or_else(|| panic!("co_spent function declaration must be rejected: {source}"));
        let message = error.to_string();
        assert!(message.contains("co_spent") && message.contains("reserved"), "{error}");
    }
}
