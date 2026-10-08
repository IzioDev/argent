use std::path::PathBuf;

use crate::compiler::loader::SourceSet;
use crate::compiler::loader::load_inline_program;
use crate::compiler::model::AppCompilationContext;

use super::*;

#[test]
fn registry_covers_every_entry_namespace_role() {
    let sources = SourceSet::discover_inline(
        PathBuf::from("entry-namespace.ag"),
        r#"
            state OwnerState { cov_id peer_id; }
            state PeerState { int value; }

            actor Owner owns OwnerState {
                entry inspect(int parameter)
                consumes { consumed: Peer, }
                observes observed by self.peer_id {
                    inputs { agent: actor_type<PeerState> as open_peer, }
                    outputs { agent: open_peer, }
                }
                spawns first_spawn by first_covenant {
                    outputs { left: Peer, }
                }
                spawns second_spawn by second_covenant {
                    outputs { left: Peer, }
                }
                emits emitted: Owner {}
            }

            actor Peer owns PeerState {}
            app Test { actor Owner; actor Peer; }
        "#
        .to_string(),
    )
    .expect("source discovers");
    let program = sources.parse_modules().expect("source parses");
    let module = &program.modules[0].legacy;
    let actor = &module.actors[0];
    let names = ReservedEntryNames::for_entry(actor, &actor.entries[0]).expect("entry namespace is collision-free");
    let roles = names.body_bindings.iter().map(|(name, role)| (name.as_str(), role.description())).collect::<BTreeMap<_, _>>();

    assert_eq!(roles.get("self").map(String::as_str), Some("current actor context"));
    assert_eq!(roles.get("consumed").map(String::as_str), Some("consume handle"));
    assert_eq!(roles.get("emitted").map(String::as_str), Some("emit handle"));
    assert_eq!(roles.get("observed").map(String::as_str), Some("observe root"));
    assert_eq!(roles.get("agent").map(String::as_str), Some("observe `observed` output label"));
    assert_eq!(roles.get("first_spawn").map(String::as_str), Some("spawn root"));
    assert_eq!(roles.get("second_spawn").map(String::as_str), Some("spawn root"));
    assert_eq!(roles.get("left").map(String::as_str), Some("spawn `first_spawn` output label"));
    assert_eq!(roles.get("first_covenant").map(String::as_str), Some("spawn `first_spawn` covenant binding"));
    assert_eq!(roles.get("second_covenant").map(String::as_str), Some("spawn `second_spawn` covenant binding"));
    assert_eq!(roles.get("open_peer").map(String::as_str), Some("observe `observed` open-actor binding"));
    assert_eq!(roles.get("parameter").map(String::as_str), Some("entry parameter"));
}

#[test]
fn registry_rejects_clause_collisions_before_emission() {
    let program = load_inline_program(
        PathBuf::from("entry-namespace-collision.ag"),
        r#"
            state OwnerState { int units; }
            state PeerState { int units; }

            actor Owner owns OwnerState {
                entry inspect() consumes { next: Peer, } emits next: Owner {}
            }

            actor Peer owns PeerState {}
            app Test { actor Owner; actor Peer; }
        "#
        .to_string(),
    )
    .expect("source resolves");
    let err = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect_err("model rejects colliding clause handles");
    assert!(err.to_string().contains("emit handle `next` collides with consume handle of the same name"), "unexpected error: {err}");
}

#[test]
fn direct_indexed_enum_route_fails_in_model_validation() {
    let sources = SourceSet::discover_inline(
        PathBuf::from("direct-enum-route.ag"),
        r#"
            state Game { int n; }
            actor A owns Game { entry hold() emits none { require(n >= 0); } }
            actor B owns Game { entry hold() emits none { require(n > 0); } }
            actor enum Move { A; B; }
            actor Mux owns Game {
                entry choose(int choice) emits next: Move {
                    unrestricted(next.value);
                    Game next_state = { n: n + 1 };
                    become next <- Move[choice](next_state);
                }
            }
            app Test { actor A; actor B; actor Mux; }
        "#
        .to_string(),
    )
    .expect("source discovers");
    let error = sources
        .with_resolved(|program| {
            AppCompilationContext::from_resolved(
                &program,
                None,
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )
            .map(|_| ())
        })
        .expect_err("a direct indexed enum route has no materialized selector proof");
    assert!(error.to_string().contains("assign an indexed actor enum choice to a local actor handle"), "unexpected error: {error}");
}

#[test]
fn helper_state_operations_fail_before_emission() {
    for (operation, expected) in [
        ("state(value)", "input-state reconstruction is only available in entry bodies"),
        ("State { count: value }", "physical `State` is compiler-owned"),
    ] {
        let source = format!(
            "state S {{ int count; }} fn bad(int value) -> S {{ return {operation}; }} actor A owns S {{ entry hold() emits none {{ require(count >= 0); }} }} app Test {{ actor A; }}"
        );
        let program = load_inline_program(PathBuf::from("helper-state-operation.ag"), source).expect("source resolves");
        let error = AppCompilationContext::from_resolved(
            &program,
            None,
            &std::collections::BTreeMap::new(),
            &crate::compiler::model::default_route_planner,
        )
        .expect_err("model rejects helper state operation");
        assert!(error.to_string().contains(expected), "unexpected error: {error}");
    }
}

#[test]
fn entry_physical_constructor_fails_before_emission() {
    let program = load_inline_program(
        PathBuf::from("entry-physical-constructor.ag"),
        "state S { int count; } actor A owns S { entry hold() emits none { State next = State { count: count + 1 }; require(next.count > 0); } } app Test { actor A; }".to_string(),
    )
    .expect("source resolves");
    let error = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect_err("model rejects physical constructor");
    assert!(error.to_string().contains("physical `State` is compiler-owned"), "unexpected error: {error}");
}

#[test]
fn spawn_output_routes_fail_during_model_validation() {
    let route = "require children.outputs become { child <- Child(ChildState {}) };";
    for (body, expected) in [
        ("require(true);".to_string(), "must be validated with"),
        (format!("if (true) {{ {route} }}"), "output validation must be unconditional"),
        (format!("{route} {route}"), "outputs are validated more than once"),
        ("require children.outputs become { other <- Child(ChildState {}) };".to_string(), "has no output `other`"),
    ] {
        let source = format!(
            "state LauncherState {{}} state ChildState {{}} actor Launcher owns LauncherState {{ \
             entry launch() spawns children by children_id {{ outputs {{ child: Child, }} }} \
             emits none {{ unrestricted(children.outputs.child.value); {body} }} }} \
             actor Child owns ChildState {{}} app Test {{ actor Launcher; actor Child; }}"
        );
        let sources = SourceSet::discover_inline(PathBuf::from("spawn-output-routes.ag"), source).expect("source discovers");
        let error = sources
            .with_resolved(|program| {
                AppCompilationContext::from_resolved(
                    &program,
                    None,
                    &std::collections::BTreeMap::new(),
                    &crate::compiler::model::default_route_planner,
                )
                .map(|_| ())
            })
            .expect_err("model rejects invalid spawn route");
        assert!(error.to_string().contains(expected), "expected `{expected}`, got: {error}");
    }
}

#[test]
fn foreign_route_actor_field_matches_bound_clause_field() {
    let source = include_str!("../../../../tests/fixtures/runtime/context_multiple_genesis_spawns/app.ag")
        .replace("left <- self.pair_type(", "left <- pair_type(")
        .replace("right <- self.pair_type(", "right <- pair_type(")
        .replace("pair <- self.pair_type(", "pair <- pair_type(");
    let sources = SourceSet::discover_inline(PathBuf::from("bound-foreign-route-field.ag"), source).expect("source discovers");
    sources
        .with_resolved(|program| {
            AppCompilationContext::from_resolved(
                &program,
                Some("ControllerApp"),
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )
            .map(|_| ())
        })
        .expect("bare and qualified state fields have the same bound actor source");
}

#[test]
fn foreign_route_actor_local_cannot_impersonate_state_field() {
    let source = include_str!("../../../../tests/fixtures/runtime/context_multiple_genesis_spawns/app.ag")
        .replace("entry launch(\n", "entry launch(\n        actor_type<PairState> pair_type,\n")
        .replace("left <- self.pair_type(", "left <- pair_type(")
        .replace("right <- self.pair_type(", "right <- pair_type(")
        .replace("pair <- self.pair_type(", "pair <- pair_type(");
    let sources = SourceSet::discover_inline(PathBuf::from("shadowed-foreign-route-field.ag"), source).expect("source discovers");
    let error = sources
        .with_resolved(|program| {
            AppCompilationContext::from_resolved(
                &program,
                Some("ControllerApp"),
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )
            .map(|_| ())
        })
        .expect_err("local actor handle cannot satisfy the state-field target");
    assert!(error.to_string().contains("expects `self.pair_type`, but route uses `pair_type`"), "unexpected error: {error}");
}
