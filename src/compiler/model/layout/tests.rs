use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::compiler::loader::{ResolvedModules, load_inline_program};
use crate::compiler::syntax::TypeRef;
use crate::compiler::syntax::node::{ModuleId, SymbolKind};

use super::*;

#[test]
fn source_state_ids_use_provenance_across_compatibility_names() {
    let first = DeclarationOrigin::Source { path: PathBuf::from("first.ag"), kind: SymbolKind::State, index: 0 };
    let second = DeclarationOrigin::Source { path: PathBuf::from("second.ag"), kind: SymbolKind::State, index: 0 };
    let local = SourceStateId::from_origin("S", first.clone());
    let renamed = SourceStateId::from_origin("Argent__linked__1__S", first);
    let foreign = SourceStateId::from_origin("S", second);

    assert_eq!(local, renamed);
    assert_ne!(local, foreign);
    assert_eq!([local, renamed, foreign].into_iter().collect::<BTreeSet<_>>().len(), 2);
}

#[test]
fn compiled_actor_ids_use_bound_identity_across_display_names() {
    let first_id = DeclId::new(ModuleId::new(0), SymbolKind::Actor, 0);
    let second_id = DeclId::new(ModuleId::new(0), SymbolKind::Actor, 1);
    let first = CompiledActorId { identity: StaticActorId::InApp(first_id), actor: "Actor".to_string() };
    let renamed = CompiledActorId { identity: StaticActorId::InApp(first_id), actor: "Renamed".to_string() };
    let second = CompiledActorId { identity: StaticActorId::InApp(second_id), actor: "Actor".to_string() };

    assert_eq!(first, renamed);
    assert_ne!(first, second);
    assert_eq!([first, renamed, second].into_iter().collect::<BTreeSet<_>>().len(), 2);
}

#[test]
fn state_layout_rejects_a_same_spelled_foreign_source() {
    let program = program("state S { int count; } actor A owns S {} app Test { actor A; }");
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("app model plans");
    let foreign = SourceStateId::from_origin(
        "S",
        DeclarationOrigin::Source { path: PathBuf::from("unrelated.ag"), kind: SymbolKind::State, index: 0 },
    );

    let error = state_layouts(&foreign, &model).expect_err("display spelling must not replace source identity");
    assert!(error.to_string().contains("conflicting source identity"), "unexpected error: {error}");
}

#[test]
fn state_lookup_accepts_a_shared_source_with_a_different_display_name() {
    let program = program("state S { int count; } actor A owns S {} app Test { actor A; }");
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("app model plans");
    let renamed = SourceStateId::from_origin("LinkedS", model.declaration_origins["S"].clone());

    assert_eq!(model.state_by_source(&renamed).expect("shared source resolves").name, "S");
    assert_eq!(model.storage_state_by_source(&renamed).expect("shared storage resolves").name, "S");
    state_layouts(&renamed, &model).expect("shared source layout resolves by provenance");
}

#[test]
fn expanded_storage_lookup_follows_source_identity_across_display_names() {
    let program = program(include_str!("../../../../tests/fixtures/emit/state_expansion/app.ag"));
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("expanded state plans");
    let renamed = SourceStateId::from_origin("RenamedForager", model.declaration_origins["ForagerState"].clone());

    assert_eq!(model.state_by_source(&renamed).expect("source resolves").name, "ForagerState");
    assert_eq!(model.storage_state_by_source(&renamed).expect("storage follows source identity").name, "AgentCapsule");
    let (_, _, relation) = state_layouts(&renamed, &model).expect("expanded layout follows source identity");
    let memory = model.source_state_id("ForagerStrategy").expect("memory identity");
    assert!(relation.fields().iter().any(|field| field.expanded_state() == Some(&memory)));
}

#[test]
fn nested_packed_width_uses_the_bound_state_field() {
    let program =
        program("state Inner { int count; bool ready; } state Outer { Inner nested; } actor A owns Outer {} app Test { actor A; }");
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("nested state model plans");
    let outer = model.source_state_id("Outer").expect("outer identity");
    let (_, storage, _) = state_layouts(&outer, &model).expect("nested state layout");
    assert_eq!(storage.fields[0].2, 9);
    let renamed = SourceStateId::from_origin("RenamedOuter", model.declaration_origins["Outer"].clone());
    let (_, renamed_storage, _) = state_layouts(&renamed, &model).expect("shared source keeps nested field width");
    assert_eq!(renamed_storage.fields[0].2, 9);
}

#[test]
fn actor_enum_storage_fields_are_rejected_before_emission() {
    let program = program(
        "state Deck { Move chosen; } actor A owns Deck {} actor B owns Deck {} actor enum Move { A; B; } app Test { actor A; actor B; }",
    );
    let error = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect_err("actor enum storage fields are unsupported");
    assert!(error.to_string().contains("unsupported type `Move`"), "unexpected error: {error}");
}

fn program(source: &str) -> ResolvedModules<'static> {
    let path = PathBuf::from("state-layout-plan-test.ag");
    load_inline_program(path, source.to_string()).expect("test source resolves")
}

#[test]
fn contract_plans_select_state_only_for_the_aligned_active_source() {
    let program = program(include_str!("../../../../tests/fixtures/state_layout/function_contexts/app.ag"));
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("function context fixture plans");
    let shared = model.source_state_id("SharedState").expect("shared state has identity");

    let aligned = model.state_lowering_by_id(model.types.names["Aligned"]).expect("Aligned lowering exists");
    let aligned_shared = aligned.source_representation(&shared).expect("SharedState is planned for Aligned");
    assert_eq!(aligned_shared.sil_type(), &SilStateType::State);

    let routed = model.state_lowering_by_id(model.types.names["Routed"]).expect("Routed lowering exists");
    let routed_shared = routed.source_representation(&shared).expect("SharedState is planned for Routed");
    assert_eq!(routed_shared.sil_type(), &SilStateType::Source(shared.clone()));

    let aligned_target = aligned.target_for_actor(&StaticActorId::InApp(model.types.names["Aligned"])).expect("active target exists");
    let routed_target = aligned.target_for_actor(&StaticActorId::InApp(model.types.names["Routed"])).expect("Routed target exists");
    assert_eq!(aligned_target.source(), routed_target.source());
    assert!(aligned_target.storage_to_physical().is_identity());
    assert!(!routed_target.storage_to_physical().is_identity());
    assert!(!routed_target.active_compatible());
    assert_eq!(
        routed.active().physical().fields().iter().map(LayoutField::sil_name).collect::<Vec<_>>(),
        ["gen__foreign_template", "left", "right"]
    );
    assert!(matches!(
        routed.active().physical().fields()[0].id(),
        PhysicalFieldId::Generated(GeneratedFieldId::Template(actor)) if actor.actor() == "Foreign"
    ));
}

#[test]
fn compatible_foreign_target_does_not_select_the_active_authored_representation() {
    let program = program(
        r#"
            state SharedState { int count; }

            actor First owns SharedState {}
            actor Second owns SharedState {}

            app Test {
                actor First;
                actor Second;
            }
        "#,
    );
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("compatible actors plan");
    let lowering = model.state_lowering_by_id(model.types.names["First"]).expect("First lowering exists");
    assert_eq!(
        lowering
            .source_representation(&model.source_state_id("SharedState").expect("shared state has identity"))
            .expect("active source is represented")
            .sil_type(),
        &SilStateType::State
    );
    let second = lowering.target_for_actor(&StaticActorId::InApp(model.types.names["Second"])).expect("Second target exists");

    assert!(second.active_compatible());
    assert!(second.source_to_storage().is_identity());
    assert!(second.storage_to_physical().is_identity());
    assert_eq!(second.sil_type(), &SilStateType::Source(model.source_state_id("SharedState").expect("shared state has identity")));
    assert_ne!(second.sil_type(), &SilStateType::State);

    let output =
        lowering.output_type_for_actor(&StaticActorId::InApp(model.types.names["Second"])).expect("Second output target exists");
    assert_eq!(output.target(), second.id());
    assert_eq!(output.canonical_target(), second.id());
    assert_eq!(output.sil_type(), &SilStateType::State);
}

#[test]
fn equal_looking_foreign_source_remains_nominally_named() {
    let program = program(
        r#"
            state LocalState { int count; }
            state ForeignState { int count; }

            actor Local owns LocalState {
                entry inspect(ForeignState foreign) emits none {
                    require(foreign.count + count >= 0);
                }
            }
            actor Foreign owns ForeignState {}

            app Test {
                actor Local;
                actor Foreign;
            }
        "#,
    );
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("equal-looking foreign state plans");
    let lowering = model.state_lowering_by_id(model.types.names["Local"]).expect("Local lowering exists");
    let local = model.source_state_id("LocalState").expect("local state has identity");
    let foreign = model.source_state_id("ForeignState").expect("foreign state has identity");

    assert_eq!(lowering.source_representation(&local).expect("local source is represented").sil_type(), &SilStateType::State);
    assert_eq!(
        lowering.source_representation(&foreign).expect("foreign source is represented").sil_type(),
        &SilStateType::Source(foreign)
    );
}

#[test]
fn nominal_source_identity_is_independent_of_shared_storage_compatibility() {
    let program = program(
        r#"
            state Capsule {
                virtual detail;
                int count;
            }
            state Detail { int value; }
            state FirstView expands Capsule { detail: Detail; }
            state SecondView expands Capsule { detail: Detail; }

            actor First owns FirstView {}
            actor Second owns SecondView {}

            app Test {
                actor First;
                actor Second;
            }
        "#,
    );
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("shared storage views plan");
    let lowering = model.state_lowering_by_id(model.types.names["First"]).expect("First lowering exists");
    let second = lowering.target_for_actor(&StaticActorId::InApp(model.types.names["Second"])).expect("Second target exists");
    let detail = lowering
        .active()
        .source
        .fields
        .iter()
        .find(|(id, _)| id.field() == "detail")
        .expect("source detail field is indexed")
        .0
        .clone();
    let stored_detail = lowering
        .active()
        .source_to_storage
        .fields()
        .iter()
        .find(|field| field.source() == &detail)
        .expect("source detail maps to storage")
        .storage();
    let physical_detail =
        lowering.active().storage_to_physical.physical_field(stored_detail).expect("stored detail maps to physical state");

    assert!(second.active_compatible());
    assert!(!second.source_to_storage().is_identity());
    assert!(second.storage_to_physical().is_identity());
    assert_ne!(second.source(), &model.source_state_id("FirstView").expect("first view has identity"));
    assert_eq!(second.source(), &model.source_state_id("SecondView").expect("second view has identity"));
    assert_eq!(
        second.sil_type(),
        &SilStateType::StoragePhysical(model.source_state_id("SecondView").expect("second view has identity"))
    );
    assert_eq!(detail.field(), "detail");
    assert_eq!(stored_detail.field(), "detail");
    assert!(lowering.active().physical().field(physical_detail).is_some());
}

#[test]
fn open_actor_type_targets_have_a_state_keyed_storage_cut() {
    let program = program(
        r#"
            state RemoteState { int value; }
            state LocalState { actor_type<RemoteState> remote; }

            actor Local owns LocalState {
                entry inspect() emits none {
                    require(1 == 1);
                }
            }

            app Test { actor Local; }
        "#,
    );
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("open actor type plans");
    let lowering = model.state_lowering_by_id(model.types.names["Local"]).expect("Local lowering exists");
    let remote = model.source_state_id("RemoteState").expect("remote state has identity");
    let target = lowering.open_state_target(&remote).expect("open state target exists");

    assert_eq!(target.id(), &PhysicalTargetId::OpenState(remote.clone()));
    assert_eq!(target.sil_type(), &SilStateType::Source(remote.clone()));
    assert!(target.storage_to_physical().is_identity());
    assert!(!target.active_compatible());

    let output = lowering.output_type_for_open_state(&remote).expect("open output type exists");
    assert_eq!(output.sil_type(), &SilStateType::Source(remote));
    assert_ne!(output.sil_type(), &SilStateType::State);
}

#[test]
fn same_source_open_output_does_not_inherit_active_generated_fields() {
    let program = program(
        r#"
            state SharedState {
                actor_type<SharedState> peer;
                int count;
            }

            actor Current owns SharedState {
                entry send() emits next: Peer {
                    SharedState next_state = SharedState {
                        peer: peer,
                        count: count + 1,
                    };
                    unrestricted(next.value);
                    become next <- Peer(next_state);
                }
            }

            actor Peer owns SharedState {
                entry hold() emits none { require(count >= 0); }
            }

            app Test {
                actor Current;
                actor Peer;
            }
        "#,
    );
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("same-source open target plans");
    let lowering = model.state_lowering_by_id(model.types.names["Current"]).expect("Current lowering exists");
    let shared = model.source_state_id("SharedState").expect("shared state has identity");
    let target = lowering.open_state_target(&shared).expect("same-source open target exists");

    assert!(lowering.active().physical().fields().iter().any(|field| matches!(field.id(), PhysicalFieldId::Generated(_))));
    assert!(target.storage_to_physical().is_identity());
    assert!(target.physical().fields().iter().all(|field| matches!(field.id(), PhysicalFieldId::Storage(_))));
    assert!(!target.active_compatible());

    let output = lowering.output_type_for_open_state(&shared).expect("same-source open output type exists");
    assert_eq!(output.sil_type(), &SilStateType::Source(shared));
    assert_ne!(output.sil_type(), &SilStateType::State);
}

#[test]
fn physical_compatibility_distinguishes_equal_width_generated_roles() {
    let field = |actor: &str| LayoutField {
        id: PhysicalFieldId::Generated(GeneratedFieldId::Template(CompiledActorId {
            identity: StaticActorId::InApp(DeclId::new(ModuleId::new(0), SymbolKind::Actor, usize::from(actor == "Second"))),
            actor: actor.to_string(),
        })),
        sil_name: "gen__template".to_string(),
        ty: TypeRef::array("byte", 32),
        sil_type: "byte[32]".to_string(),
        packed_len: 32,
    };
    let first = PhysicalStateLayout { fields: vec![field("First")] };
    let second = PhysicalStateLayout { fields: vec![field("Second")] };

    assert_eq!(first.fields()[0].packed_len, second.fields()[0].packed_len);
    assert_eq!(first.fields()[0].sil_type(), second.fields()[0].sil_type());
    assert!(!first.is_sil_compatible_with(&second));
}

#[test]
fn dynamic_actor_domains_reject_incompatible_semantic_layouts() {
    let actor = |name: &str| CompiledActorId {
        identity: StaticActorId::InApp(DeclId::new(ModuleId::new(0), SymbolKind::Actor, usize::from(name == "Second"))),
        actor: name.to_string(),
    };
    let target_id = |name: &str| PhysicalTargetId::Actor(actor(name));
    let shared_state = SourceStateId::from_origin(
        "SharedState",
        DeclarationOrigin::Source { path: PathBuf::from("shared.ag"), kind: SymbolKind::State, index: 0 },
    );
    let plan = |name: &str| {
        let id = target_id(name);
        TargetPhysicalPlan {
            id: id.clone(),
            source: shared_state.clone(),
            source_to_storage: SourceStorageRelation::Identity { fields: Vec::new() },
            storage_to_physical: StoragePhysicalRelation::Augmented {
                generated_fields: vec![GeneratedFieldId::Template(actor(name))],
                storage_to_physical: Vec::new(),
            },
            physical: PhysicalStateLayout {
                fields: vec![LayoutField {
                    id: PhysicalFieldId::Generated(GeneratedFieldId::Template(actor(name))),
                    sil_name: "gen__template".to_string(),
                    ty: TypeRef::array("byte", 32),
                    sil_type: "byte[32]".to_string(),
                    packed_len: 32,
                }],
            },
            sil_type: SilStateType::TargetPhysical(id),
            active_compatible: false,
        }
    };
    let plans = BTreeMap::from([("First", plan("First")), ("Second", plan("Second"))])
        .into_values()
        .map(|plan| (plan.id.clone(), plan))
        .collect::<BTreeMap<_, _>>();
    let variants = vec![target_id("First"), target_id("Second")];
    let domain = PhysicalTargetId::ActorDomain { state: shared_state, actors: vec![actor("First"), actor("Second")] };
    let active = plans[&target_id("First")].physical.clone();

    let err = canonical_domain_plan(&domain, &variants, &active, &plans).expect_err("semantic role mismatch is rejected");
    assert!(err.to_string().contains("semantic physical layout"), "unexpected error: {err}");
}

#[test]
fn actor_domain_output_records_its_target_and_canonical_type_owner() {
    let program = program(include_str!("../../../../examples/route_state_body_choice.ag"));
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("selector example plans");
    let lowering = model.state_lowering_by_id(model.types.names["Mux"]).expect("Mux lowering exists");
    let variants = vec![StaticActorId::InApp(model.types.names["Pawn"]), StaticActorId::InApp(model.types.names["Knight"])];
    let output = lowering
        .output_type_for_actor_domain(&model.source_state_id("BoardState").expect("board state has identity"), &variants)
        .expect("selector domain output exists");

    assert!(
        matches!(output.target(), PhysicalTargetId::ActorDomain { state, actors } if state.as_str() == "BoardState" && actors.len() == 2)
    );
    assert!(matches!(output.canonical_target(), PhysicalTargetId::Actor(actor) if actor.actor() == "Pawn"));
    assert_eq!(output.sil_type(), &SilStateType::State);
}
