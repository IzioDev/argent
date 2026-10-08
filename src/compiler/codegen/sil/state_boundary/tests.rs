use std::path::PathBuf;

use crate::compiler::loader::{ResolvedModules, SourceSet, load_inline_program};
use crate::compiler::model::AppCompilationContext;
use crate::compiler::syntax::{ActorDecl, AuthoredEntryStatement, EntryDecl};

use super::*;

fn program(source: &str) -> ResolvedModules<'static> {
    let path = PathBuf::from("state-boundary-test.ag");
    load_inline_program(path, source.to_string()).expect("test source resolves")
}

fn actor_entry<'a>(model: &'a AppCompilationContext<'a>, actor: &str, entry: &str) -> (&'a ActorDecl, &'a EntryDecl) {
    let actor = model.actor_by_decl(model.types.names[actor]).expect("actor exists");
    let entry = actor.entries.iter().find(|candidate| candidate.name == entry).expect("entry exists");
    (actor, entry)
}

fn input_reference_plan(actor: &ActorDecl, entry: &EntryDecl, model: &AppCompilationContext<'_>) -> EntryInputReferencePlan {
    let entry_id = model
        .entry_model_by_id(crate::compiler::syntax::node::EntryId {
            actor: model.types.names[&actor.name],
            index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
        })
        .expect("entry model")
        .id;
    let state_values = StateValueTypes::new(entry_id.actor, model).expect("state value types");
    plan_entry_input_references(entry_id, actor, entry, model, &state_values).expect("input references plan")
}

fn struct_fields(expr: SilExpr<'static>, expected_type: &str) -> Vec<SilStateFieldExpr<'static>> {
    let SilExprKind::StructLiteral { name, fields, .. } = expr.kind else { panic!("expected a struct expression") };
    assert_eq!(name, expected_type);
    fields
}

#[test]
fn state_valued_input_field_requires_its_completed_type_plan() {
    let program = program(
        "state Inner { int value; } state Outer { Inner nested; } actor A owns Outer { entry hold() emits none { require(nested.value >= 0); } } app Test { actor A; }",
    );
    let mut model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("state-valued input plans");
    let actor_id = model.types.names["A"];
    let field = SourceFieldId::new(model.source_state_id("Outer").expect("source state"), "nested");
    let values = StateValueTypes::new(actor_id, &model).expect("state value types");
    source_field_sil_type(&field, &values, &model).expect("planned state-valued field renders");

    model.actor_value_plans.get_mut(&actor_id).expect("value plan").field_values.remove(&field);
    let values = StateValueTypes::new(actor_id, &model).expect("state value types");
    let error =
        source_field_sil_type(&field, &values, &model).expect_err("missing state-value fact must not fall back to source type text");
    assert!(error.to_string().contains("no completed state-value plan"), "unexpected error: {error}");
}

#[test]
fn aligned_active_input_is_direct_authored_state_with_the_covenant_domain_proof() {
    let program = program(include_str!("../../../../../tests/fixtures/emit/single_actor_self_consume/app.ag"));
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("self-consume fixture plans");
    let (actor, entry) = actor_entry(&model, "Counter", "merge");
    let plan = input_reference_plan(actor, entry, &model);
    let input = plan.consumed(InteractionId::CurrentInput(0)).expect("self input exists");

    assert!(matches!(input.physical.as_ref().map(|physical| &physical.proof), Some(InputTemplateProof::CovenantDomain)));
    assert_eq!(input.physical.as_ref().map_or("State", |physical| physical.sil_type.as_str()), "State");
    assert_eq!(input.access.authored_sil_type, "State");
    assert!(input.access.complete.is_some());
    assert!(matches!(input.complete_authored_ast().expect("aligned input is authored").kind,
        SilExprKind::Identifier(name) if name == "gen__other_state"));

    let active = plan.active();
    assert!(matches!(
        active.native_value_ast().kind,
        SilExprKind::IndexedIntrospection { kind: SilIndexedIntrospectionKind::InputValue, .. }
    ));
    assert!(matches!(active.covenant_id_ast().kind,
        SilExprKind::Call { name, .. } if name == "OpInputCovenantId"));
    assert!(matches!(active.project_field_ast("count").expect("active field projects").kind,
        SilExprKind::Identifier(name) if name == "count"));
    assert!(!struct_fields(active.complete_authored_ast().expect("active state reconstructs"), "State").is_empty());
}

#[test]
fn named_identity_input_is_already_an_authored_source_value() {
    let program = program(include_str!("../../../../../tests/fixtures/emit/input_template_route_reuse/app.ag"));
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("peer input fixture plans");
    let (actor, entry) = actor_entry(&model, "Controller", "step");
    let plan = input_reference_plan(actor, entry, &model);
    let input = plan.consumed(InteractionId::CurrentInput(0)).expect("peer input exists");

    assert!(!matches!(input.physical.as_ref().map(|physical| &physical.proof), Some(InputTemplateProof::CovenantDomain)));
    assert_eq!(input.physical.as_ref().map_or("State", |physical| physical.sil_type.as_str()), "PeerState");
    assert!(input.access.complete.is_some());
    assert_eq!(input.access.source.as_str(), "PeerState");
    assert!(matches!(input.complete_authored_ast().expect("named input is authored").kind,
        SilExprKind::Identifier(name) if name == "gen__peer_state"));
}

#[test]
fn augmented_input_projects_only_user_fields_from_its_actor_keyed_type() {
    let program = program(
        r#"
            state BoxState { int units; }

            actor Left owns BoxState {
                entry shift() consumes { peer: Right, } emits { left: Left, remote: Right, } {
                    BoxState next_left = { units: units - 1, };
                    BoxState next_peer = { units: peer.units + 1, };
                    unrestricted(left.value);
                    unrestricted(remote.value);
                    become { left <- Left(next_left), remote <- Right(next_peer), };
                }
            }

            actor Right owns BoxState {
                delegate accept() consumes { leader: Left, } {}
            }

            app Test { actor Left; actor Right; }
        "#,
    );
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("paired actors plan");
    let (actor, entry) = actor_entry(&model, "Left", "shift");
    let plan = input_reference_plan(actor, entry, &model);
    let input = plan.consumed(InteractionId::CurrentInput(0)).expect("peer input exists");
    assert_eq!(input.physical.as_ref().map_or("State", |physical| physical.sil_type.as_str()), "Gen__RightState");
    assert!(input.access.complete.is_none());
    let fields = struct_fields(input.complete_authored_ast().expect("identity user fields materialize"), "BoxState");
    assert_eq!(fields.len(), 1, "generated route fields must not enter authored state");
    assert_eq!(fields[0].name, "units");
    assert!(matches!(&fields[0].expr.kind, SilExprKind::FieldAccess { source, field, .. }
        if field == "units" && matches!(&source.kind, SilExprKind::Identifier(name) if name == "gen__peer_state")));
}

#[test]
fn expanded_input_requires_a_validated_preimage_for_authored_access() {
    let program = program(
        r#"
            state Capsule { int nonce; virtual detail; }
            state Details { int count; }
            state Expanded expands Capsule { detail: Details; }

            actor Vault owns Expanded {
                entry hold() emits none { require(nonce >= 0); }
            }

            state ReaderState { int nonce; }
            actor Reader owns ReaderState {
                entry inspect() consumes { vault: Vault, } emits next: Reader {
                    require(vault.nonce >= 0);
                    unrestricted(next.value);
                    become next <- self;
                }
            }

            app Test { actor Vault; actor Reader; }
        "#,
    );
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("expanded input plans");
    let (actor, entry) = actor_entry(&model, "Reader", "inspect");
    let plan = input_reference_plan(actor, entry, &model);
    let input = plan.consumed(InteractionId::CurrentInput(0)).expect("vault input exists");

    assert_eq!(input.physical.as_ref().map_or("State", |physical| physical.sil_type.as_str()), "Gen__PhysicalExpanded");
    assert!(input.complete_authored_ast().is_none(), "expanded value needs its preimage");
    assert!(input.project_field_ast("detail").is_none(), "unavailable expanded field has no projection");
}

#[test]
fn active_expanded_reference_reconstructs_from_validated_openings() {
    let program = program(include_str!("../../../../../tests/fixtures/emit/state_expansion/app.ag"));
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("expanded active state plans");
    let (actor, entry) = actor_entry(&model, "Forager", "hold");
    let plan = input_reference_plan(actor, entry, &model);
    let active = plan.active();

    let strategy = struct_fields(active.project_field_ast("strategy").expect("validated opening projects"), "ForagerStrategy");
    assert!(
        strategy.iter().any(|field| field.name == "hunger"
            && matches!(&field.expr.kind, SilExprKind::Identifier(name) if name == "gen__strategy_hunger"))
    );

    let authored = struct_fields(active.complete_authored_ast().expect("expanded active state reconstructs"), "ForagerState");
    assert!(authored.iter().any(|field| field.name == "strategy"
        && matches!(&field.expr.kind, SilExprKind::StructLiteral { name, .. } if name == "ForagerStrategy")));
    assert!(
        authored
            .iter()
            .any(|field| field.name == "energy" && matches!(&field.expr.kind, SilExprKind::Identifier(name) if name == "energy"))
    );
}

#[test]
fn entry_input_views_distinguish_complete_body_lowering_from_clause_expressions() {
    let program = program(
        r#"
            state ObserverState {}
            state SourceState { int amount; }
            state PeerState { int amount; }

            actor Observer owns ObserverState {
                entry inspect(cov_id remote_id)
                consumes { source: Source, }
                observes remote by remote_id {
                    inputs { peer: Peer, }
                }
                emits none {
                    require(source.amount >= remote.inputs.peer.amount);
                }
            }

            actor Source owns SourceState {
                entry hold() emits none { require(amount >= 0); }
            }

            actor Peer owns PeerState {
                entry hold() emits none { require(amount >= 0); }
            }

            app Test { actor Observer; actor Source; actor Peer; }
        "#,
    );
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("mixed input entry plans");
    let (actor, entry) = actor_entry(&model, "Observer", "inspect");
    let plan = input_reference_plan(actor, entry, &model);

    assert!(plan.consumed(InteractionId::CurrentInput(0)).is_ok());
    assert!(plan.observed(InteractionId::ObservedInput { observe: 0, input: 0 }).is_ok());
}

#[test]
fn observed_input_plan_uses_the_canonical_reference_identity() {
    let program = program(include_str!("../../../../../tests/fixtures/emit/observed_template_witnesses/app.ag"));
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("observed input fixture plans");
    let (actor, entry) = actor_entry(&model, "Local", "step");
    let plan = input_reference_plan(actor, entry, &model);
    let input = plan.observed(InteractionId::ObservedInput { observe: 0, input: 0 }).expect("observed input exists");

    assert_eq!(input.reference(), "asset.inputs.src");
    assert_eq!(input.access.source.as_str(), "ForeignState");
    assert!(input.access.complete.is_some());
    let SilStatement::VariableDefinition { type_ref, name, expr: Some(expr), .. } = input.read_statement() else {
        panic!("observed input must define an authenticated value")
    };
    assert_eq!(type_ref.type_name(), "ForeignState");
    assert_eq!(name, "gen__asset_src_state");
    let SilExprKind::Call { name: builtin, args, .. } = expr.kind else { panic!("observed input read must be a call") };
    assert_eq!(builtin, "readInputStateWithTemplate");
    assert!(matches!(&args[0].kind, SilExprKind::Identifier(index) if index == "gen__asset_src_input_idx"));
}

#[test]
fn retained_observed_input_reference_uses_bound_root() {
    SourceSet::discover_inline(
        PathBuf::from("bound-observed-input.ag"),
        include_str!("../../../../../tests/fixtures/emit/observed_template_witnesses/app.ag")
            .replace("state(asset.inputs.src)", "state(asset . inputs . src)"),
    )
    .expect("source discovers")
    .with_resolved(|program| {
        let model = AppCompilationContext::from_resolved(
            &program,
            None,
            &std::collections::BTreeMap::new(),
            &crate::compiler::model::default_route_planner,
        )?;
        let (actor, entry) = actor_entry(&model, "Local", "step");
        let plan = input_reference_plan(actor, entry, &model);
        let view = &plan;
        let entry_id = model
            .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
            })?
            .id;
        let statements = model.resolution.entry_body(entry_id)?;
        let AuthoredEntryStatement::Sil(statement) = &statements[0] else { panic!("first statement is an authored declaration") };
        let SilStatement::VariableDefinition { expr: Some(state_call), .. } = statement.as_ref() else {
            panic!("first statement initializes the previous state")
        };
        let SilExprKind::Call { args, .. } = &state_call.kind else { panic!("previous state uses state(...)") };
        assert_ne!(args[0].span.as_str().trim(), "asset.inputs.src");
        assert_eq!(view.reference_ast(&args[0]).map(PlannedEntryInputReference::reference), Some("asset.inputs.src"));
        assert!(view.reference_ast(state_call).is_none());
        let contract = ContractLowerer::new(
            model
                .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                    actor: model.types.names[&actor.name],
                    index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
                })?
                .id
                .actor,
            &model,
        )?
        .lower_actor()?;
        let emitted = silverscript_lang::ast::format_contract_ast(&contract);
        assert!(emitted.contains("gen__asset_src_state"), "{emitted}");
        Ok(())
    })
    .expect("retained observed reference binds");
}

#[test]
fn selector_output_uses_the_actor_domain_plan_and_its_selected_type() {
    let program = program(include_str!("../../../../../examples/route_state_body_choice.ag"));
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("selector example plans");
    let (actor, entry) = actor_entry(&model, "Mux", "choose");
    let selector = model
        .entry_model_by_id(crate::compiler::syntax::node::EntryId {
            actor: model.types.names[&actor.name],
            index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
        })
        .expect("entry model exists")
        .template_selectors()
        .get("target")
        .expect("target selector exists");
    let actor_id = model
        .entry_model_by_id(crate::compiler::syntax::node::EntryId {
            actor: model.types.names[&actor.name],
            index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
        })
        .expect("entry model")
        .id
        .actor;
    let planned = model.output_plan_by_id(actor_id).expect("output plan").selector(selector).expect("selector target");
    let output = plan_selector_output_state(actor_id, selector, &model).expect("selector output state plans");

    assert!(
        matches!(&planned.target, PhysicalTargetId::ActorDomain { state, actors } if state.as_str() == "BoardState" && actors.len() == 2)
    );
    assert!(matches!(&planned.canonical_target, PhysicalTargetId::Actor(actor) if actor.actor() == "Pawn"));
    assert_eq!(output.physical_type(), "State");
}

#[test]
fn augmented_output_materialization_injects_only_planned_generated_fields() {
    let program = program(include_str!("../../../../../tests/fixtures/state_layout/function_contexts/app.ag"));
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("function context fixture plans");
    let (actor, _) = actor_entry(&model, "Routed", "advance");
    let target = plan_actor_output_state(
        model.types.names[&actor.name],
        &model.static_actor_target("Routed").expect("selected actor").id(),
        &model,
    )
    .expect("self output plans");
    let physical = target
        .materialize_authored_ast(
            SilExpr::identifier("next_state".to_string()),
            model.state_lowering_by_id(model.types.names["Routed"]).expect("Routed lowering exists"),
            &model,
        )
        .expect("authored output materializes");
    let fields = struct_fields(physical, target.physical_type());
    assert!(fields.iter().any(|field| field.name == "gen__foreign_template"
        && matches!(&field.expr.kind, SilExprKind::Identifier(name) if name == "gen__foreign_template")));
    for name in ["left", "right"] {
        assert!(fields.iter().any(|field| field.name == name
            && matches!(&field.expr.kind, SilExprKind::FieldAccess { source, field: projected, .. }
                if projected == name && matches!(&source.kind, SilExprKind::Identifier(root) if root == "next_state"))));
    }
}

#[test]
fn expanded_output_lowers_authored_values_to_digest_storage_before_physical_state() {
    let program = program(include_str!("../../../../../tests/fixtures/emit/state_expansion/app.ag"));
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("expanded state fixture plans");
    let (actor, _) = actor_entry(&model, "Forager", "hold");
    let target = plan_actor_output_state(
        model.types.names[&actor.name],
        &model.static_actor_target("Forager").expect("selected actor").id(),
        &model,
    )
    .expect("expanded self output plans");
    let physical = target
        .materialize_authored_ast(
            SilExpr::identifier("next_state".to_string()),
            model.state_lowering_by_id(model.types.names["Forager"]).expect("Forager lowering exists"),
            &model,
        )
        .expect("expanded output materializes");
    let fields = struct_fields(physical, "State");
    assert!(
        fields
            .iter()
            .any(|field| field.name == "strategy" && matches!(&field.expr.kind, SilExprKind::Call { name, .. } if name == "blake3"))
    );
    assert!(fields.iter().any(|field| field.name == "energy"
        && matches!(&field.expr.kind, SilExprKind::FieldAccess { source, field: projected, .. }
            if projected == "energy" && matches!(&source.kind, SilExprKind::Identifier(root) if root == "next_state"))));
}
