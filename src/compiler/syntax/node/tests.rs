use std::path::PathBuf;

use super::{ChildEdge, DeclId, ModuleId, NodeAddress, ReferenceRole, RootSlot, SourceNodeCursor, SymbolKind};
use crate::compiler::loader::SourceSet;
use crate::compiler::syntax::source::{Origin, SourceId};

#[test]
fn parsed_sites_have_structural_ids_and_authored_origins() {
    let source = "const int[2] FIRST = 1 + 2;\nconst int[2] SECOND = 1 + 2;";
    let sources = SourceSet::discover_inline(PathBuf::from("sites.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let nodes = &program.nodes;
    assert_eq!(program.modules[0].legacy.consts.len(), 2);
    assert_eq!(sources.files[0].text, source);

    let first = DeclId::new(ModuleId::new(0), SymbolKind::Const, 0);
    let second = DeclId::new(ModuleId::new(0), SymbolKind::Const, 1);
    let address = |owner, root, children| NodeAddress { owner, root, children };
    let first_value = nodes.find(&address(first, RootSlot::ConstValue, Vec::new())).expect("first value site");
    let second_value = nodes.find(&address(second, RootSlot::ConstValue, Vec::new())).expect("second value site");
    assert_ne!(first_value, second_value);
    assert_eq!(
        nodes.node(first_value).origin,
        Origin::Authored { source: SourceId(0), start: source.find("1 + 2").unwrap(), end: source.find("1 + 2").unwrap() + 5 }
    );
    assert_eq!(nodes.node(second_value).address.owner, second);

    let dimension = nodes.find(&address(first, RootSlot::ConstType, vec![ChildEdge::TypeDimension(0)])).expect("array dimension site");
    assert_eq!(nodes.node(dimension).origin, Origin::Authored { source: SourceId(0), start: 9, end: 12 });
    assert_eq!(nodes.source(first), SourceId(0));
    assert_eq!(nodes.source(second), SourceId(0));
}

#[test]
fn bound_declarations_resolve_paths_through_the_source_index() {
    let source = "const int LIMIT = 4;\nstate State { int value; }\nactor Actor owns State {}";
    let program = crate::compiler::loader::load_inline_program(PathBuf::from("stable.ag"), source.to_string())
        .expect("source parsing and binding");
    let ids = program.root_declarations().collect::<Vec<_>>();
    assert_eq!(ids.len(), 3);
    for id in ids {
        assert_eq!(program.declaration_path(id), std::path::Path::new("stable.ag"));
    }
}

#[test]
fn member_and_type_sites_follow_the_parsed_structure() {
    let source = r#"
state Ledger {
    int balance;
}
fn total(int value) -> int { return value; }
actor Vault owns Ledger {
    fn read() -> int { return balance; }
    entry pay() emits none {}
}
app Wallet { actor Vault; }
"#;
    let sources = SourceSet::discover_inline(PathBuf::from("members.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let nodes = &program.nodes;
    let module = ModuleId::new(0);
    let state = DeclId::new(module, SymbolKind::State, 0);
    let function = DeclId::new(module, SymbolKind::Function, 0);
    let actor = DeclId::new(module, SymbolKind::Actor, 0);
    let app = DeclId::new(module, SymbolKind::App, 0);
    let at = |owner, root, children| nodes.find(&NodeAddress { owner, root, children }).expect("indexed site");

    let field_type = nodes.node(at(state, RootSlot::FieldType(0), Vec::new())).origin;
    assert_eq!(
        field_type,
        Origin::Authored {
            source: SourceId(0),
            start: source.find("int balance").unwrap(),
            end: source.find("int balance").unwrap() + 3
        }
    );
    let actor_state = nodes.node(at(actor, RootSlot::ActorState, Vec::new())).origin;
    assert_eq!(
        actor_state,
        Origin::Authored {
            source: SourceId(0),
            start: source.find("owns Ledger").unwrap() + 5,
            end: source.find("owns Ledger").unwrap() + 11
        }
    );

    let helper = at(actor, RootSlot::ActorFunction(0), Vec::new());
    let helper_name = at(actor, RootSlot::ActorFunction(0), vec![ChildEdge::Name]);
    let entry = at(actor, RootSlot::Entry(0), Vec::new());
    let entry_name = at(actor, RootSlot::Entry(0), vec![ChildEdge::Name]);
    assert_ne!(helper, entry);
    assert!(
        nodes.find(&NodeAddress { owner: function, root: RootSlot::Declaration, children: vec![ChildEdge::ParamType(0)] }).is_some()
    );
    assert!(
        nodes.find(&NodeAddress { owner: function, root: RootSlot::Declaration, children: vec![ChildEdge::ParamName(0)] }).is_some()
    );
    assert!(
        nodes.find(&NodeAddress { owner: function, root: RootSlot::Declaration, children: vec![ChildEdge::ReturnType] }).is_some()
    );
    assert!(nodes.find(&NodeAddress { owner: actor, root: RootSlot::ActorFunction(0), children: vec![ChildEdge::Body] }).is_some());
    assert!(nodes.find(&NodeAddress { owner: actor, root: RootSlot::Entry(0), children: vec![ChildEdge::Body] }).is_some());
    assert_eq!(
        nodes.node(helper_name).origin,
        Origin::Authored { source: SourceId(0), start: source.find("read()").unwrap(), end: source.find("read()").unwrap() + 4 }
    );
    assert_eq!(
        nodes.node(entry_name).origin,
        Origin::Authored { source: SourceId(0), start: source.find("pay()").unwrap(), end: source.find("pay()").unwrap() + 3 }
    );
    assert!(matches!(nodes.node(at(app, RootSlot::AppActor(0), Vec::new())).origin, Origin::Authored { .. }));
}

#[test]
fn expression_and_statement_references_have_distinct_structural_roles() {
    let source = "const int RESULT = compute(data.field); fn update(int value) { target = compute(value); }";
    let sources = SourceSet::discover_inline(PathBuf::from("roles.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let nodes = &program.nodes;
    let module = ModuleId::new(0);
    let constant = DeclId::new(module, SymbolKind::Const, 0);
    let function = DeclId::new(module, SymbolKind::Function, 0);
    let at = |cursor: SourceNodeCursor| nodes.node(nodes.find(&cursor.address).expect("indexed source site"));

    let const_expr = SourceNodeCursor::new(constant, RootSlot::ConstValue);
    assert_eq!(at(const_expr.child(ChildEdge::CallTarget)).reference_role, Some(ReferenceRole::Call));
    let argument = const_expr.child(ChildEdge::Argument(0));
    assert_eq!(at(argument.child(ChildEdge::FieldLabel)).reference_role, Some(ReferenceRole::FieldLabel));
    assert_eq!(at(argument.child(ChildEdge::ExpressionSource)).reference_role, Some(ReferenceRole::Value));

    let assignment = SourceNodeCursor::new(function, RootSlot::Declaration).child(ChildEdge::Body).child(ChildEdge::Statement(0));
    assert_eq!(at(assignment.child(ChildEdge::AssignmentTarget)).reference_role, Some(ReferenceRole::AssignmentTarget));
    assert_eq!(at(assignment.child(ChildEdge::Expression).child(ChildEdge::CallTarget)).reference_role, Some(ReferenceRole::Call));
    assert_eq!(sources.files[0].text, source);
}

#[test]
fn symbolic_cardinality_bounds_are_indexed_as_value_paths() {
    let source = "actor Agent owns State { entry step() emits next: Agent[MIN..=MAX] { become next <- Agent[](states); } }";
    let sources = SourceSet::discover_inline(PathBuf::from("bounds.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let actor = DeclId::new(ModuleId::new(0), SymbolKind::Actor, 0);
    let output = SourceNodeCursor::new(actor, RootSlot::Entry(0)).child(ChildEdge::EmitOutput(0));
    for (edge, name) in [(ChildEdge::CardinalityMin, "MIN"), (ChildEdge::CardinalityMax, "MAX")] {
        let id = program.nodes.find(&output.child(edge).address).expect("bound source site");
        assert_eq!(program.nodes.node(id).reference_role, Some(ReferenceRole::Value));
        assert_eq!(program.modules[0].name_paths[&id].segments, [name]);
    }
}
