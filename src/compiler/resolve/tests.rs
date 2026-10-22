use super::bindings::Binding;
use super::*;
use crate::compiler::loader::load_inline_program;
use crate::compiler::syntax::node::{ChildEdge, RootSlot, SourceNodeCursor};
use crate::compiler::syntax::source::Origin;
use std::path::PathBuf;

#[test]
fn actor_helper_calls_keep_bound_member_indices() {
    let program = load_inline_program(
        PathBuf::from("actor-helper-bindings.ag"),
        r#"
            state S {}
            actor A owns S {
                fn first() -> int { return second(); }
                fn second() -> int { return 2; }
                entry inspect() emits none { require(first() == 2); }
            }
            app Test { actor A; }
        "#
        .to_string(),
    )
    .expect("actor helper source resolves");
    let actor = program.root_declarations().find(|id| id.kind() == SymbolKind::Actor).expect("actor declaration");
    let bindings = program.bindings(actor);
    for (root, member) in [(RootSlot::ActorFunction(0), 1), (RootSlot::Entry(0), 0)] {
        let calls = bindings
            .sites
            .iter()
            .filter(|(site, _)| {
                let address = &program.nodes().node(**site).address;
                address.root == root && address.children.last() == Some(&ChildEdge::CallTarget)
            })
            .map(|(_, binding)| binding)
            .collect::<Vec<_>>();
        assert_eq!(calls, [&Binding::ActorHelper(member)]);
    }
}

#[test]
fn current_entry_target_bindings_keep_actor_and_enum_declarations() {
    let consume_program = load_inline_program(
        PathBuf::from("consume-targets.ag"),
        include_str!("../../../tests/fixtures/emit/input_template_route_reuse/app.ag").to_string(),
    )
    .expect("consume source resolves");
    let controller = consume_program
        .root_declarations()
        .find(|id| id.kind() == SymbolKind::Actor && consume_program.declaration(*id).name() == "Controller")
        .expect("Controller declaration");
    let peer = consume_program
        .root_declarations()
        .find(|id| id.kind() == SymbolKind::Actor && consume_program.declaration(*id).name() == "Peer")
        .expect("Peer declaration");
    assert_eq!(consume_program.bindings(controller).entry_consume_targets[&(0, 0)], peer);
    assert_eq!(consume_program.bindings(controller).entry_emit_targets[&(0, 0, 0)], peer);

    let enum_program = load_inline_program(
        PathBuf::from("enum-targets.ag"),
        include_str!("../../../examples/route_state_body_choice.ag").to_string(),
    )
    .expect("enum source resolves");
    let mux = enum_program
        .root_declarations()
        .find(|id| id.kind() == SymbolKind::Actor && enum_program.declaration(*id).name() == "Mux")
        .expect("Mux declaration");
    let move_enum = enum_program
        .root_declarations()
        .find(|id| id.kind() == SymbolKind::ActorEnum && enum_program.declaration(*id).name() == "MoveActor")
        .expect("MoveActor declaration");
    assert_eq!(enum_program.bindings(mux).entry_emit_targets[&(0, 0, 0)], move_enum);
}

#[test]
fn initializer_uses_outer_binding_before_declaring_local() {
    let program = load_inline_program(
        PathBuf::from("initializer-scope.ag"),
        "const int value = 7; fn copy() -> int { int value = value; return value; }".to_string(),
    )
    .expect("source resolves");
    let owner = DeclId::new(ModuleId::new(0), SymbolKind::Function, 0);
    let body = SourceNodeCursor::new(owner, RootSlot::Declaration).child(ChildEdge::Body);
    let initializer = body.child(ChildEdge::Statement(0)).child(ChildEdge::Expression);
    let returned = body.child(ChildEdge::Statement(1)).child(ChildEdge::Argument(0));
    let initializer_id = program.nodes().find(&initializer.address).expect("initializer indexed");
    let returned_id = program.nodes().find(&returned.address).expect("return indexed");
    assert_eq!(
        program.bindings(owner).sites[&initializer_id],
        Binding::Source(ResolvedName::Declaration(DeclId::new(ModuleId::new(0), SymbolKind::Const, 0)))
    );
    assert!(matches!(program.bindings(owner).sites[&returned_id], Binding::Local(_)));
    assert!(program.source_text(owner.module).contains("int value = value;"), "binding leaves authored text unchanged");
}

#[test]
fn field_labels_are_not_bound_as_values() {
    let program = load_inline_program(
        PathBuf::from("field-label.ag"),
        "const int n = 4; state S { int n; } fn make() -> S { return S { n: n }; }".to_string(),
    )
    .expect("source resolves");
    let owner = DeclId::new(ModuleId::new(0), SymbolKind::Function, 0);
    let literal = SourceNodeCursor::new(owner, RootSlot::Declaration)
        .child(ChildEdge::Body)
        .child(ChildEdge::Statement(0))
        .child(ChildEdge::Argument(0));
    let field = literal.child(ChildEdge::Field(0));
    let label_id = program.nodes().find(&field.child(ChildEdge::FieldLabel).address).expect("label indexed");
    let value_id = program.nodes().find(&field.child(ChildEdge::Expression).address).expect("value indexed");
    assert!(!program.bindings(owner).sites.contains_key(&label_id));
    assert_eq!(
        program.bindings(owner).sites[&value_id],
        Binding::Source(ResolvedName::Declaration(DeclId::new(ModuleId::new(0), SymbolKind::Const, 0)))
    );
}

#[test]
fn unknown_initializer_name_is_rejected_before_it_becomes_a_local() {
    let error = load_inline_program(PathBuf::from("unknown-initializer.ag"), "fn f() { int value = value; }".to_string())
        .expect_err("initializer cannot refer to its own new binding");
    assert!(error.to_string().contains("unknown reference `value`"), "unexpected error: {error}");
}

#[test]
fn named_dimensions_bind_constants_at_each_type_site() {
    let program = load_inline_program(
        PathBuf::from("dimension-bindings.ag"),
        r#"
            const int LENGTH = 2;
            fn inspect(int number) -> int {
                int[4][LENGTH] values = int[4][LENGTH]{int[4]{1, 2, 3, 4}, int[4]{5, 6, 7, 8}};
                byte[2] bytes = number as byte[LENGTH];
                return values.length;
            }
        "#
        .to_string(),
    )
    .expect("array dimensions resolve");
    let owner = DeclId::new(ModuleId::new(0), SymbolKind::Function, 0);
    let constant = DeclId::new(ModuleId::new(0), SymbolKind::Const, 0);
    let body = SourceNodeCursor::new(owner, RootSlot::Declaration).child(ChildEdge::Body);
    let bindings = program.bindings(owner);
    for (cursor, dimension) in [
        (body.child(ChildEdge::Statement(0)).child(ChildEdge::TypeUse), 1),
        (body.child(ChildEdge::Statement(0)).child(ChildEdge::Expression).child(ChildEdge::TypeUse), 1),
        (body.child(ChildEdge::Statement(1)).child(ChildEdge::Expression).child(ChildEdge::CallTarget), 0),
    ] {
        let site = program.nodes().find(&cursor.address).expect("type site is indexed");
        assert_eq!(bindings.dimension_constants[&(site, dimension)], constant);
        assert_eq!(bindings.sites[&site], Binding::Builtin, "base types retain their own binding");
        assert_eq!(
            program.type_use(site).expect("parsed type exists").ty.array_dims[dimension],
            silverscript_lang::ast::ArrayDim::Constant("LENGTH".to_string()),
            "resolution leaves the authored type unchanged"
        );
    }
    assert_eq!(bindings.dimension_constants.len(), 3);
    assert!(bindings.declarations.contains(&constant), "dimension-only references retain the constant dependency");
}

#[test]
fn named_dimensions_reject_missing_and_nonconstant_declarations() {
    for dimension in ["MISSING", "Row"] {
        let error = load_inline_program(
            PathBuf::from("invalid-dimension.ag"),
            format!("state Row {{ int count; }} fn inspect() {{ int[{dimension}] values = int[]{{1, 2}}; }}"),
        )
        .expect_err("dimensions must refer to constants");
        assert!(error.to_string().contains(dimension), "dimension error must name its source: {error}");
    }
}

#[test]
fn global_helpers_reject_bare_builtin_and_runtime_names() {
    for name in ["co_spent", "tx", "this", "r0", "self", "length", "sha256", "OpTxGas"] {
        let source = format!(
            "state S {{ int {name}; }} fn shared() -> int {{ return {name}; }} \
             actor A owns S {{ entry check() emits none {{ require(shared() > 0); }} }} app Test {{ actor A; }}"
        );
        let error = load_inline_program(PathBuf::from("global-runtime-value.ag"), source)
            .expect_err("runtime and builtin names cannot capture actor fields");
        assert!(error.to_string().contains(&format!("unknown reference `{name}`")), "{name}: {error}");
    }
    for root in ["self", "this", "tx", "r0"] {
        let error =
            load_inline_program(PathBuf::from("global-runtime-field.ag"), format!("fn shared() -> int {{ return {root}.count; }}"))
                .expect_err("an arbitrary field access does not qualify as a runtime expression");
        assert!(error.to_string().contains(&format!("unknown reference `{root}`")), "{root}: {error}");
    }
}

#[test]
fn sil_opcode_builtins_resolve() {
    for name in [
        "OpTxSubnetId",
        "OpTxGas",
        "OpTxPayloadLen",
        "OpTxLockTime",
        "OpTxPayloadSubstr",
        "OpOutpointTxId",
        "OpOutpointIndex",
        "OpTxInputScriptSigLen",
        "OpTxInputScriptSigSubstr",
        "OpTxInputSeq",
        "OpTxInputDaaScore",
        "OpTxInputIsCoinbase",
        "OpTxInputSpkLen",
        "OpTxInputSpkSubstr",
        "OpTxOutputSpkLen",
        "OpTxOutputSpkSubstr",
        "OpAuthOutputCount",
        "OpAuthOutputIdx",
        "OpInputCovenantId",
        "OpOutputCovenantId",
        "OpCovInputCount",
        "OpCovInputIdx",
        "OpCovOutputCount",
        "OpCovOutputIdx",
        "OpNum2Bin",
        "OpBin2Num",
        "OpChainblockSeqCommit",
    ] {
        load_inline_program(PathBuf::from("sil-opcode.ag"), format!("const int VALUE = {name}(0);"))
            .unwrap_or_else(|error| panic!("{name} should resolve: {error}"));
    }
}

#[test]
fn sil_state_builtins_resolve_as_calls_not_bare_values() {
    for name in [
        "readInputStateWithTemplate",
        "validateOutputState",
        "validateOutputStateWithTemplate",
        "validateOutputStateWithInputTemplate",
    ] {
        let program = load_inline_program(PathBuf::from("sil-state-builtin.ag"), format!("fn inspect() {{ {name}(0); }}"))
            .unwrap_or_else(|error| panic!("{name} should resolve: {error}"));
        let owner = DeclId::new(ModuleId::new(0), SymbolKind::Function, 0);
        let call = SourceNodeCursor::new(owner, RootSlot::Declaration)
            .child(ChildEdge::Body)
            .child(ChildEdge::Statement(0))
            .child(ChildEdge::CallTarget);
        let site = program.nodes().find(&call.address).expect("builtin call is indexed");
        assert_eq!(program.bindings(owner).sites[&site], Binding::Builtin, "{name}");

        let error =
            load_inline_program(PathBuf::from("sil-state-builtin-value.ag"), format!("fn inspect() {{ int value = {name}; }}"))
                .expect_err("builtin names are not bare values");
        assert!(error.to_string().contains(&format!("unknown reference `{name}`")), "{name}: {error}");
    }
}

#[test]
fn primitive_array_casts_resolve_without_accepting_unknown_types() {
    for name in ["int[]", "int[2]", "int[_]", "temporal[]", "bool[]", "string[]", "pubkey[]", "sig[]", "datasig[]", "byte[]"] {
        load_inline_program(PathBuf::from("array-cast.ag"), format!("const int VALUE = {name}(0);"))
            .unwrap_or_else(|error| panic!("{name} should resolve: {error}"));
    }
    let error = load_inline_program(PathBuf::from("array-cast.ag"), "const int VALUE = Missing[](0);".to_string())
        .expect_err("unknown array cast type must not become a builtin");
    assert!(error.to_string().contains("unknown reference `Missing[]`"), "unexpected error: {error}");
}

#[test]
fn source_bindings_respect_lexical_scopes_and_preserve_authored_text() {
    let program = load_inline_program(
        PathBuf::from("scopes.ag"),
        r#"
        const int LIMIT = 1;
        const int PARAM = 2;
        const int INDEX = 3;
        const int PAIR = 4;
        state Item { int LIMIT; }
        fn scoped(int PARAM) -> int {
            int result = LIMIT;
            { int LIMIT = 5; result = result + LIMIT; }
            for (INDEX, 0, 2, 2) { result = result + INDEX; }
            { int left, int PAIR = sha256(0x00); result = result + PAIR; }
            Item item = Item { LIMIT: result };
            result = result + item.LIMIT;
            return result + LIMIT + PARAM + INDEX;
        }
    "#
        .to_string(),
    )
    .expect("source resolves");
    let function = program.root_declarations().find(|id| id.kind() == SymbolKind::Function).unwrap();
    let text_before = program.source_text(function.module).to_string();
    let source = program.source_text(function.module);
    let mut references = program
        .bindings(function)
        .sites
        .iter()
        .filter_map(|(site, binding)| {
            let Binding::Source(ResolvedName::Declaration(id)) = binding else { return None };
            let Origin::Authored { start, end, .. } = program.nodes().node(*site).origin else { return None };
            Some((start, end, *id))
        })
        .collect::<Vec<_>>();
    references.sort_by_key(|(start, _, _)| *start);
    let names = references
        .iter()
        .map(|(start, end, id)| {
            let name = program.declaration(*id).name().to_string();
            assert_eq!(&source[*start..*end], name);
            name
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["LIMIT", "Item", "Item", "LIMIT", "INDEX"]);
    let closure = program.declaration_closure([function]);
    assert!(!closure.iter().any(|id| matches!(program.declaration(*id).name(), "PARAM" | "PAIR")));
    assert_eq!(program.source_text(function.module), text_before);
}

#[test]
fn source_bindings_distinguish_numeric_units_from_declaration_references() {
    for unit in ["seconds", "minutes", "hours", "days", "weeks", "litras", "grains", "kas"] {
        let ty = if matches!(unit, "litras" | "grains" | "kas") { "int" } else { "temporal" };
        let program = load_inline_program(
            PathBuf::from("numeric-units.ag"),
            format!(
                r#"
                const int {unit} = 2;
                state S {{ int count; }}
                actor A owns S {{
                    entry inspect() emits none {{
                        {ty} value = 5 {unit};
                        require(count + {unit} + int(value) >= 0);
                    }}
                }}
            "#
            ),
        )
        .expect("source resolves");
        let actor = program.root_declarations().find(|id| id.kind() == SymbolKind::Actor).unwrap();
        let constant = program.root_declarations().find(|id| id.kind() == SymbolKind::Const).unwrap();
        let references = program
            .bindings(actor)
            .sites
            .iter()
            .filter(|(site, binding)| {
                program.nodes().node(**site).address.root == RootSlot::Entry(0)
                    && *binding == &Binding::Source(ResolvedName::Declaration(constant))
            })
            .collect::<Vec<_>>();
        assert_eq!(references.len(), 1, "{unit}: only the constant use should bind");
        let Origin::Authored { start, end, .. } = program.nodes().node(*references[0].0).origin else {
            panic!("constant use has an authored location");
        };
        let source = program.source_text(actor.module);
        assert_eq!(&source[start..end], unit);
        assert!(source[..start].ends_with("count + "));
    }
}

#[test]
fn unknown_bare_body_type_is_rejected() {
    let error = load_inline_program(PathBuf::from("unknown-body-type.ag"), "fn check() { Missing value = { n: 1 }; }".to_string())
        .expect_err("body types must resolve before modeling");
    assert!(error.to_string().contains("unknown export `Missing`"), "{error}");
}

#[test]
fn unresolved_spawn_target_is_rejected() {
    let error = load_inline_program(
        PathBuf::from("unknown-spawn-target.ag"),
        r#"
        state S {}
        actor Root owns S {
            entry launch() spawns children by id { outputs { child: Missing, } } emits none {}
        }
        "#
        .to_string(),
    )
    .expect_err("static spawn targets must resolve before modeling");
    assert!(error.to_string().contains("unknown export `Missing`"), "{error}");
}

#[test]
fn unresolved_qualified_body_reference_is_rejected() {
    let error = load_inline_program(PathBuf::from("unknown-qualified-reference.ag"), "fn check() { missing::value(); }".to_string())
        .expect_err("qualified body references must resolve before modeling");
    assert!(error.to_string().contains("unresolved qualified reference `missing::value`"), "{error}");
}

#[test]
fn unknown_actor_enum_variant_is_rejected() {
    let error = load_inline_program(
        PathBuf::from("unknown-enum-variant.ag"),
        r#"
        state S {}
        actor A owns S {}
        actor B owns S {}
        actor enum Kind { A; B; }
        fn check() { Kind::Missing; }
        "#
        .to_string(),
    )
    .expect_err("enum variants must belong to their declared enum");
    assert!(error.to_string().contains("actor enum `Kind` has no variant `Missing`"), "{error}");
}
