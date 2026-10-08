use std::path::PathBuf;

use silverscript_lang::ast::{ExprKind, Statement};

use super::{AuthoredEntryStatement, AuthoredSuccessor};
use crate::compiler::loader::SourceSet;
use crate::compiler::syntax::RouteId;

#[test]
fn entries_use_the_shared_parser_for_sil_and_transition_syntax() {
    let source = r#"
        state State { int value; }
        actor Agent owns State {
            entry step() emits next: Agent {
                int value = 1;
                if (value > 0) {
                    become next <- Agent(State { value: value });
                } else {
                    become next <- self;
                }
            }
        }
    "#;
    let sources = SourceSet::discover_inline(PathBuf::from("entry.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let body = program.modules[0].entry_bodies.values().next().expect("entry AST");
    assert!(matches!(&body[0], AuthoredEntryStatement::Sil(statement) if matches!(**statement, Statement::VariableDefinition { .. })));
    let AuthoredEntryStatement::If { then_branch, else_branch, .. } = &body[1] else {
        panic!("entry condition must retain its branches");
    };
    let AuthoredEntryStatement::Block { statements, .. } = &**then_branch else {
        panic!("then branch must be a source block");
    };
    let AuthoredEntryStatement::Become { routes, .. } = &statements[0] else {
        panic!("then branch must transition");
    };
    let AuthoredSuccessor::Constructed { actor, state, many: false } = &routes[0].successor else {
        panic!("constructed successor expected");
    };
    assert!(matches!(actor.kind, ExprKind::Identifier(_)));
    assert!(matches!(state.kind, ExprKind::StructLiteral { .. }));
    assert!(matches!(**else_branch.as_ref().expect("else branch"), AuthoredEntryStatement::Block { .. }));
}

#[test]
fn entry_transition_inside_a_loop_fails_in_source_parsing() {
    let source = "actor Agent owns State { entry step() emits next: Agent { for (i, 0, 1, 1) { become next <- self; } } }";
    let sources = SourceSet::discover_inline(PathBuf::from("loop.ag"), source.to_string()).expect("source discovery");
    let error = sources.parse_modules().expect_err("transition inside a loop must fail");
    assert!(error.message.contains("transitions are not allowed inside ordinary loops"), "{error}");
    assert_eq!(error.location.expect("authored location").byte_offset, source.find("become").unwrap());
}

#[test]
fn constructed_route_requires_a_state_expression_during_parsing() {
    let source = "actor Agent owns State { entry step() emits next: Agent { become next <- Agent(); } }";
    let sources = SourceSet::discover_inline(PathBuf::from("empty-route-state.ag"), source.to_string()).expect("source discovery");
    sources.parse_modules().expect_err("a constructed route cannot omit its state expression");
}

#[test]
fn terminal_routes_use_authored_branch_order_and_entry_local_ids() {
    let source = r#"
        actor Agent owns State {
            entry first(bool choice) emits next: Agent {
                if (choice) {
                    become next <- Agent(State { value: 1 });
                } else {
                    become next <- self;
                }
            }
            entry second() emits next: Agent {
                become next <- Agent[](states);
            }
        }
    "#;
    let sources = SourceSet::discover_inline(PathBuf::from("routes.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let entries = &program.modules[0].legacy.actors[0].entries;
    assert_eq!(entries[0].terminal_route_sets, vec![vec![RouteId(0)], vec![RouteId(1)]]);
    assert_eq!(entries[0].routes.iter().map(|route| route.id).collect::<Vec<_>>(), vec![RouteId(0), RouteId(1)]);
    assert_eq!(entries[1].terminal_route_sets, vec![vec![RouteId(0)]]);
    assert_eq!(entries[1].routes[0].id, RouteId(0));

    let authored = program.modules[0].entry_bodies.values().collect::<Vec<_>>();
    let AuthoredEntryStatement::If { then_branch, else_branch, .. } = &authored[0][0] else {
        panic!("first entry must retain authored branches");
    };
    let AuthoredEntryStatement::Block { statements, .. } = &**then_branch else {
        panic!("then branch must be a block");
    };
    assert!(matches!(&statements[0], AuthoredEntryStatement::Become { routes, .. } if routes[0].id == entries[0].routes[0].id));
    assert!(
        matches!(&**else_branch.as_ref().expect("else branch"), AuthoredEntryStatement::Block { statements, .. } if matches!(&statements[0], AuthoredEntryStatement::Become { routes, .. } if routes[0].id == entries[0].routes[1].id))
    );
}

#[test]
fn nonterminal_authored_routes_report_the_original_source_location() {
    for (body, marker, expected) in [
        ("become next <- self;\n                require(true);", "require", "`become` must be terminal"),
        ("if (true) { become next <- self; }", "}", "conditional `become` must be terminal"),
    ] {
        let source = format!("/* café */\r\nactor Agent owns State {{ entry step() emits next: Agent {{ {body} }} }}");
        let sources = SourceSet::discover_inline(PathBuf::from("route-error.ag"), source.clone()).expect("source discovery");
        let error = sources.parse_modules().expect_err("nonterminal route must fail");
        assert!(error.message.contains(expected), "{error}");
        assert_eq!(error.path.as_deref(), Some(std::path::Path::new("route-error.ag")));
        let location = error.location.expect("authored location");
        let expected_offset = if marker == "}" {
            source.rfind("} }").expect("entry closing brace")
        } else {
            source.find(marker).expect("following statement")
        };
        assert_eq!(location.byte_offset, expected_offset);
    }
}

#[test]
fn entry_ast_keeps_sil_bindings_and_constructed_route_shapes() {
    let source = r#"
        actor Mux owns State {
            entry step() emits none {
                actor_type<State> target = Move[index + offset];
                for (i, 0, count, MAX_COUNT) { require(i >= 0); }
                require children.outputs become {
                    child <- self.child_type(next_child),
                };
                become {
                    one <- Account(state),
                    many <- Account[](states),
                    selected <- Move[index][](selected_states),
                };
            }
        }
    "#;
    let sources = SourceSet::discover_inline(PathBuf::from("entry-ast.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let module = &program.modules[0];
    let body = module.entry_bodies.values().next().expect("entry AST");
    let AuthoredEntryStatement::Sil(declaration) = &body[0] else { panic!("local declaration is Sil AST") };
    let Statement::VariableDefinition { name, expr: Some(initializer), .. } = declaration.as_ref() else {
        panic!("typed selector has a parsed initializer");
    };
    assert_eq!(name, "target");
    assert!(matches!(initializer.kind, ExprKind::ArrayIndex { .. }));
    assert!(module.type_uses.values().any(|ty| ty.actor_state.as_ref().is_some_and(|state| state.segments == ["State"])));
    assert!(matches!(&body[1], AuthoredEntryStatement::Sil(statement) if matches!(statement.as_ref(), Statement::For { .. })));
    let AuthoredEntryStatement::ForeignBecome { group, routes, .. } = &body[2] else {
        panic!("foreign routes retain their authored group");
    };
    assert_eq!(group.segments, ["children"]);
    assert!(
        matches!(&routes[0].successor, AuthoredSuccessor::Constructed { actor, .. } if matches!(actor.kind, ExprKind::FieldAccess { .. }))
    );
    let AuthoredEntryStatement::Become { routes, .. } = &body[3] else { panic!("current routes remain structured") };
    assert_eq!(
        routes.iter().map(|route| route.output.segments.as_slice()).collect::<Vec<_>>(),
        [["one"].as_slice(), ["many"].as_slice(), ["selected"].as_slice()]
    );
    assert!(matches!(&routes[0].successor, AuthoredSuccessor::Constructed { many: false, .. }));
    assert!(matches!(&routes[1].successor, AuthoredSuccessor::Constructed { many: true, .. }));
    assert!(matches!(&routes[2].successor, AuthoredSuccessor::Constructed { many: true, actor, .. }
        if matches!(actor.kind, ExprKind::ArrayIndex { .. })));
    assert_eq!(module.legacy.actors[0].entries[0].terminal_route_sets, vec![vec![routes[0].id, routes[1].id, routes[2].id]]);
}

#[test]
fn malformed_routes_fail_on_the_shared_source_parser() {
    for (body, expected) in [
        ("become { first <- A, second <- B(next) };", "expected `(` after become target"),
        ("become { first <- A(next); second <- B(next) };", "expected `,` or `}`"),
        ("become Done(next);", "must name its output"),
    ] {
        let source = format!("actor Agent owns State {{ entry step() emits none {{ {body} }} }}");
        let sources = SourceSet::discover_inline(PathBuf::from("invalid-routes.ag"), source).expect("source discovery");
        let error = sources.parse_modules().expect_err("malformed route must fail");
        assert!(error.message.contains(expected), "{error}");
    }
}
