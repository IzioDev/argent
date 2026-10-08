use std::path::PathBuf;

use super::*;
use crate::compiler::loader::{SourceSet, load_inline_program};
use crate::compiler::model::AppCompilationContext;
use crate::compiler::model::link::DeclarationOrigin;
use crate::compiler::resolve::ResolvedModules;
use crate::compiler::syntax::node::SymbolKind;
use crate::compiler::syntax::node::{ChildEdge, RootSlot, SourceNodeCursor};

fn root_type_table(program: &ResolvedModules<'_>) -> crate::error::Result<TypeTable> {
    let app = program.root_app(None)?;
    let actors = if let Some(app) = app {
        program.app_actor_ids(app)?
    } else {
        program.root_declarations().filter(|id| id.kind() == SymbolKind::Actor).collect()
    };
    let names = program.selected_declaration_names(app, &actors)?;
    TypeTable::new(program, &names)
}

#[test]
fn digest_operand_state_identity_is_completed_before_emission() {
    let source = r#"
        state S { int count; }
        actor A owns S {
            entry check(S snapshot) emits none {
                byte[32] commitment = digest(snapshot);
                require(commitment == commitment);
            }
        }
        app Test { actor A; }
    "#;
    SourceSet::discover_inline(PathBuf::from("digest-operand.ag"), source.to_string())
        .expect("source discovers")
        .with_resolved(|program| {
            let model = AppCompilationContext::from_resolved(
                &program,
                None,
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )?;
            let values = model
                .types
                .digest_operands
                .iter()
                .filter(|(site, _)| {
                    let address = &program.nodes().node(**site).address;
                    address.owner == model.types.names["A"] && address.root == RootSlot::Entry(0)
                })
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            assert_eq!(values.len(), 1);
            assert_eq!(values[0].source(), &model.source_state_id("S")?);
            assert_eq!(values[0].shape(), StateValueShape::Scalar);
            Ok(())
        })
        .expect("digest state is planned in the model");

    let invalid = source.replace("entry check(S snapshot)", "entry check(int snapshot)");
    let error = SourceSet::discover_inline(PathBuf::from("digest-invalid.ag"), invalid)
        .expect("invalid source still parses")
        .with_resolved(|program| {
            AppCompilationContext::from_resolved(
                &program,
                None,
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )
            .map(|_| ())
        })
        .expect_err("non-state digest operand fails during model construction");
    assert!(error.to_string().contains("requires a proven authored state value"), "unexpected error: {error}");
}

#[test]
fn bound_array_value_preserves_source_identity_and_inferred_length() {
    let first = SourceStateId::from_origin(
        "S",
        DeclarationOrigin::Source { path: PathBuf::from("first.ag"), kind: SymbolKind::State, index: 0 },
    );
    let second = SourceStateId::from_origin(
        "S",
        DeclarationOrigin::Source { path: PathBuf::from("second.ag"), kind: SymbolKind::State, index: 0 },
    );
    let ty = silverscript_lang::ast::parse_type_ref("S[_]").expect("authored array type parses");
    let value = PlannedStateValue::from_bound_type(second.clone(), &ty, Some(4)).expect("bound array value");
    assert_eq!(value.source(), &second);
    assert_ne!(value.source(), &first);
    assert_eq!(value.shape(), StateValueShape::FixedArray(FixedArrayLength::Known(4)));
    assert_eq!(
        PlannedStateValue::from_bound_type(first, &ty, None).expect("unresolved array value").shape(),
        StateValueShape::FixedArray(FixedArrayLength::Unresolved)
    );
    let unsupported = silverscript_lang::ast::parse_type_ref("S[2][3]").expect("nested type parses");
    assert!(PlannedStateValue::from_bound_type(second, &unsupported, None).is_none());
}

#[test]
fn state_types_and_callable_signatures_share_source_identity() {
    let program = load_inline_program(
        PathBuf::from("type-table-test.ag"),
        include_str!("../../../../tests/fixtures/state_layout/function_contexts/app.ag").to_string(),
    )
    .expect("fixture resolves");
    let types = root_type_table(&program).expect("source types resolve");
    let state = types.names["SharedState"];
    let actor = types.names["Aligned"];
    let global = types.names["global_fixed"];

    assert_eq!(types.actor_states[&actor], state);
    assert_eq!(types.state_fields[&(state, 0)].base, ResolvedTypeBase::Builtin("int".to_string()));
    assert_eq!(
        types.callables[&CallableId { owner: global, member: None }].params[0],
        ResolvedType { base: ResolvedTypeBase::State(state), array: Some(ArrayDim::Fixed(2)) }
    );
    assert_eq!(
        types.callables[&CallableId { owner: actor, member: Some(1) }].result,
        Some(ResolvedType { base: ResolvedTypeBase::State(state), array: Some(ArrayDim::Fixed(2)) })
    );
    assert_eq!(
        types.entry_params[&(actor, 0, 2)],
        ResolvedType { base: ResolvedTypeBase::State(state), array: Some(ArrayDim::Dynamic) }
    );
    assert!(types.node_state_uses.iter().any(|(site, target)| {
        *target == state
            && program.nodes().node(*site).address.owner == actor
            && program.nodes().node(*site).address.root == RootSlot::ActorFunction(0)
    }));
    assert!(types.route_values.iter().any(|((entry, _), value)| {
        entry.actor == types.names["Routed"]
            && value.actor_target == BoundRouteActor::Fixed(types.names["Routed"])
            && program.nodes().node(value.actor_site).address.root == RootSlot::Entry(entry.index)
            && program.nodes().node(value.state_site).address.root == RootSlot::Entry(entry.index)
    }));
}

#[test]
fn authored_local_operand_types_are_planned_by_type_site() {
    let program = load_inline_program(
        PathBuf::from("operand-types.ag"),
        r#"
        state S { cov_id owner; }
        actor A owns S {
            entry check() emits none {
                cov_id copy = owner;
                S snapshot = self;
                { S copy = snapshot; }
                require(copy.co_spent());
            }
        }
        app Test { actor A; }
        "#
        .to_string(),
    )
    .expect("source resolves");
    let types = root_type_table(&program).expect("type sites resolve");
    let actor = types.names["A"];
    let state = types.names["S"];
    let operands = types
        .type_use_operands
        .iter()
        .filter(|(site, _)| {
            let address = &program.nodes().node(**site).address;
            address.owner == actor && address.root == RootSlot::Entry(0)
        })
        .map(|(_, ty)| *ty)
        .collect::<Vec<_>>();
    assert!(operands.contains(&OperandType::CovenantId));
    assert!(operands.contains(&OperandType::State(state)));
    let copies = program
        .bindings(actor)
        .local_names
        .iter()
        .filter(|(id, name)| id.callable == RootSlot::Entry(0) && name.as_str() == "copy")
        .map(|(id, _)| types.local_operands[id])
        .collect::<Vec<_>>();
    assert_eq!(copies, vec![OperandType::CovenantId, OperandType::State(state)]);
}

#[test]
fn local_actor_handle_state_uses_its_bound_type_site() {
    let program = load_inline_program(
        PathBuf::from("local-actor-handle.ag"),
        r#"
        state RemoteState { int value; }
        state LocalState { int nonce; }
        actor Local owns LocalState {
            entry inspect(actor_type<RemoteState> remote_param) emits none {
                actor_type<RemoteState> remote_local = remote_param;
                require(nonce >= 0);
            }
        }
        app Test { actor Local; }
        "#
        .to_string(),
    )
    .expect("actor-handle source resolves");
    let types = root_type_table(&program).expect("type sites resolve");
    let actor = types.names["Local"];
    let local = program
        .bindings(actor)
        .local_names
        .iter()
        .find(|(id, name)| id.callable == RootSlot::Entry(0) && name.as_str() == "remote_local")
        .map(|(id, _)| *id)
        .expect("local actor handle has a bound identity");
    assert_eq!(types.local_actor_handle_states[&local], types.names["RemoteState"]);
}

#[test]
fn entry_input_and_spawn_covenant_operands_use_bound_local_ids() {
    let program =
        crate::compiler::loader::load_program(std::path::Path::new("tests/fixtures/runtime/context_static_actor_spawn/app.ag"))
            .expect("spawn fixture resolves");
    let types = root_type_table(&program).expect("source types resolve");
    let actor = types.names["Launcher"];
    let bindings = program.bindings(actor);
    let consumed = bindings.entry_consumes[&(0, 0)];
    let covenant = bindings.entry_spawn_covenants[&(0, 0)];
    assert_eq!(types.local_operands[&consumed], OperandType::State(types.names["ChildState"]));
    assert_eq!(types.local_operands[&covenant], OperandType::CovenantId);
}

#[test]
fn co_spend_receiver_is_rejected_during_model_construction() {
    let source = r#"
        state S { byte[32] owner; }
        actor A owns S {
            entry check() emits none {
                require(owner.co_spent());
            }
        }
        app Test { actor A; }
    "#;
    let sources = SourceSet::discover_inline(PathBuf::from("invalid-co-spend.ag"), source.to_string()).expect("source discovers");
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
        .expect_err("co-spend receiver must be checked by the model");
    assert!(error.to_string().contains("requires one `cov_id` receiver"), "{error}");
    assert!(error.to_string().contains("A::check"), "{error}");
}

#[test]
fn selected_app_finishes_routes_layouts_and_value_plans_before_emission() {
    let program = load_inline_program(
        PathBuf::from("completed-model-test.ag"),
        include_str!("../../../../tests/fixtures/state_layout/function_contexts/app.ag").to_string(),
    )
    .expect("fixture resolves");
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("selected app plans");

    for actor in model.app_actors.iter() {
        let actor_model = &model.actor_models[&model.types.names[actor]];
        let values = model.actor_value_plan_by_id(model.types.names[actor]).expect("actor has a value plan");
        assert!(values.required_sources.contains(&model.source_state_id(&actor_model.source().state).expect("state identity")));
        assert!(
            model
                .state_lowering_by_id(model.types.names[actor])
                .expect("actor layout")
                .target_for_actor(&crate::compiler::model::StaticActorId::InApp(model.types.names[actor]))
                .is_some()
        );
        assert!(model.route_leaves_by_actor.contains_key(&model.types.names[actor]));
    }
    let reader = model.actor_by_decl(model.types.names["Reader"]).expect("Reader is selected");
    let inspect = reader.entries.iter().find(|entry| entry.name == "inspect").expect("inspect entry exists");
    let entry_id = model
        .entry_model_by_id(crate::compiler::syntax::node::EntryId {
            actor: model.types.names[&reader.name],
            index: reader.entries.iter().position(|candidate| std::ptr::eq(candidate, inspect)).expect("entry belongs to actor"),
        })
        .expect("inspect is planned")
        .id;
    let reader_values = model.actor_value_plan_by_id(model.types.names["Reader"]).expect("reader values are planned");
    assert_eq!(reader_values.entry_param_ids[&(entry_id, 0)].source(), &model.source_state_id("SharedState").expect("source"));
    assert_eq!(reader_values.entry_param_ids[&(entry_id, 1)].shape(), StateValueShape::FixedArray(FixedArrayLength::Known(2)));
    assert_eq!(reader_values.entry_param_ids[&(entry_id, 2)].shape(), StateValueShape::DynamicArray);
}

#[test]
fn source_field_values_use_nominal_state_ids() {
    SourceSet::discover_inline(
        PathBuf::from("source-field-values.ag"),
        r#"
            state Inner { int count; }
            state Outer { Inner nested; }
            actor Holder owns Outer {
                entry hold() emits none { require(nested.count >= 0); }
            }
            app Test { actor Holder; }
        "#
        .to_string(),
    )
    .expect("source discovers")
    .with_resolved(|program| {
        let model = AppCompilationContext::from_resolved(
            &program,
            None,
            &std::collections::BTreeMap::new(),
            &crate::compiler::model::default_route_planner,
        )?;
        let values = model.actor_value_plan_by_id(model.types.names["Holder"])?;
        let field = SourceFieldId::new(model.source_state_id("Outer")?, "nested");
        let planned = values.field_values.get(&field).expect("nested state field is planned");
        assert_eq!(planned.source(), &model.source_state_id("Inner")?);
        assert_eq!(planned.shape(), StateValueShape::Scalar);
        Ok(())
    })
    .expect("field identities are planned");
}

#[test]
fn source_field_value_uses_bound_type_identity() {
    SourceSet::discover_inline(
        PathBuf::from("bound-source-field.ag"),
        r#"
            state Inner { int count; }
            state Alternate { int count; }
            state Outer { Inner nested; }
            actor Holder owns Outer {
                entry hold() emits none { require(nested.count >= 0); }
            }
            app Test { actor Holder; }
        "#
        .to_string(),
    )
    .expect("source discovers")
    .with_resolved(|program| {
        let mut model = AppCompilationContext::from_resolved(
            &program,
            None,
            &std::collections::BTreeMap::new(),
            &crate::compiler::model::default_route_planner,
        )?;
        let actor = model.actor_by_decl(model.types.names["Holder"])?.clone();
        let actor_id = model.types.names[&actor.name];
        let outer = model.types.names["Outer"];
        let alternate = model.types.names["Alternate"];
        model.types.state_fields.get_mut(&(outer, 0)).expect("nested field type").base = ResolvedTypeBase::State(alternate);
        let values = ActorValuePlan::new(actor_id, &actor, &model)?;
        let field = SourceFieldId::new(model.source_state_id("Outer")?, "nested");
        assert_eq!(values.field_values[&field].source(), &model.source_state_id("Alternate")?);

        model.types.state_fields.remove(&(outer, 0));
        let error = ActorValuePlan::new(actor_id, &actor, &model).expect_err("missing bound field type must fail");
        assert!(error.to_string().contains("has no resolved type"));

        model.types.display_names.insert(alternate, "Inner".to_string());
        let source = model.source_state_id_by_decl(alternate).expect("bound state identity survives display alias");
        assert_eq!(source, model.source_state_id("Alternate")?);
        assert_eq!(source.as_str(), "Alternate");
        assert_eq!(model.state_decl_id_by_source(&source), Some(alternate));
        Ok(())
    })
    .expect("bound field identity is authoritative");
}

#[test]
fn foreign_route_state_identity_is_checked_in_the_model() {
    let fixture = include_str!("../../../../tests/fixtures/emit/in_app_observe_routes/app.ag");
    let source_text = fixture.replace("remote_next <- Foreign(next_foreign)", "remote_next <- Foreign(next_state(0))");
    assert_ne!(source_text, fixture, "fixture has the foreign route");
    SourceSet::discover_inline(PathBuf::from("foreign-route-state.ag"), source_text)
        .expect("source discovers")
        .with_resolved(|program| {
            let error = AppCompilationContext::from_resolved(
                &program,
                None,
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )
            .expect_err("foreign state mismatch must fail before emission");
            assert!(error.to_string().contains("foreign route state"), "unexpected error: {error}");
            Ok(())
        })
        .expect("foreign route validation runs in the model");
}

#[test]
fn model_plans_entry_initializers_from_retained_ast() {
    SourceSet::discover_inline(
        PathBuf::from("retained-entry-ast.ag"),
        include_str!("../../../../tests/fixtures/state_layout/function_contexts/app.ag").to_string(),
    )
    .expect("source discovers")
    .with_resolved(|program| {
        let model = AppCompilationContext::from_resolved(
            &program,
            None,
            &std::collections::BTreeMap::new(),
            &crate::compiler::model::default_route_planner,
        )?;
        let actor = model.actor_by_decl(model.types.names["Reader"])?;
        let entry = actor.entries.iter().find(|entry| entry.name == "inspect").expect("inspect entry");
        let entry_id = model
            .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
            })?
            .id;
        assert!(!program.entry_body(entry_id)?.is_empty(), "model retains authored entry statements");
        let authored = SourceNodeCursor::new(entry_id.actor, RootSlot::Entry(entry_id.index))
            .child(ChildEdge::Body)
            .child(ChildEdge::Statement(2))
            .child(ChildEdge::BindingName);
        let authored_id = program.nodes().find(&authored.address).expect("authored binding site");
        assert_eq!(model.types.body_values[&authored_id].shape(), StateValueShape::FixedArray(FixedArrayLength::Known(3)),);
        Ok(())
    })
    .expect("retained AST model plans");
}

#[test]
fn authored_body_values_follow_bound_parameter_and_actor_field() {
    let source = r#"
        state Left { int n; }
        state Right { int n; }
        state Store { Left item; int quantity; }
        actor A owns Store {
            entry inspect(Right item) emits none {
                Right from_param = item;
                Left from_field = self.item;
                require(from_param.n >= 0);
                require(from_field.n >= 0);
                require(quantity >= 0);
            }
        }
        app Test { actor A; }
    "#;
    SourceSet::discover_inline(PathBuf::from("bound-body-values.ag"), source.to_string())
        .expect("source discovers")
        .with_resolved(|program| {
            let model = AppCompilationContext::from_resolved(
                &program,
                None,
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )?;
            let actor = model.actor_by_decl(model.types.names["A"])?;
            let entry = &actor.entries[0];
            let id = model
                .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                    actor: model.types.names[&actor.name],
                    index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
                })?
                .id;
            let binding = |index| {
                let cursor = SourceNodeCursor::new(id.actor, RootSlot::Entry(id.index))
                    .child(ChildEdge::Body)
                    .child(ChildEdge::Statement(index))
                    .child(ChildEdge::BindingName);
                program.nodes().find(&cursor.address).expect("authored binding site")
            };
            assert_eq!(model.types.body_values[&binding(0)].source(), &model.source_state_id("Right")?,);
            assert_eq!(model.types.body_values[&binding(1)].source(), &model.source_state_id("Left")?,);
            let field_uses = model
                .types
                .actor_field_uses
                .iter()
                .filter(|(site, _)| {
                    let address = &model.resolution.nodes().node(**site).address;
                    address.owner == id.actor && address.root == RootSlot::Entry(id.index)
                })
                .map(|(_, field)| field.clone())
                .collect::<Vec<_>>();
            assert_eq!(
                field_uses,
                [
                    SourceFieldId::new(model.source_state_id("Store")?, "item"),
                    SourceFieldId::new(model.source_state_id("Store")?, "quantity"),
                ]
            );
            Ok(())
        })
        .expect("body values follow bound identities");
}

#[test]
fn qualified_helper_result_shapes_are_planned_by_bound_identity() {
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("system clock").as_nanos();
    let directory = std::env::temp_dir().join(format!("argent-qualified-body-types-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("fixture directory");
    std::fs::write(directory.join("lib.ag"), "state Shared { int value; } fn pass(Shared[2] values) -> Shared[2] { return values; }")
        .expect("library source");
    std::fs::write(
        directory.join("app.ag"),
        r#"import "./lib.ag" as lib;
            state Local { int balance; }
            actor A owns Local {
                entry inspect(lib::Shared[2] values) emits none {
                    lib::Shared[_] copy = lib::pass(values);
                    require(copy.length == 2);
                }
            }
            app Test { actor A; }"#,
    )
    .expect("app source");
    SourceSet::discover_file(&directory.join("app.ag"))
        .expect("sources discover")
        .with_resolved(|program| {
            let model = AppCompilationContext::from_resolved(
                &program,
                Some("Test"),
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )?;
            let actor = model.actor_by_decl(model.types.names["A"])?;
            let entry = &actor.entries[0];
            let id = model
                .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                    actor: model.types.names[&actor.name],
                    index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
                })?
                .id;
            let cursor = SourceNodeCursor::new(id.actor, RootSlot::Entry(id.index))
                .child(ChildEdge::Body)
                .child(ChildEdge::Statement(0))
                .child(ChildEdge::BindingName);
            let binding = program.nodes().find(&cursor.address).expect("authored binding site");
            assert_eq!(model.types.body_values[&binding].shape(), StateValueShape::FixedArray(FixedArrayLength::Known(2)),);
            Ok(())
        })
        .expect("qualified helper plans");
    std::fs::remove_dir_all(directory).expect("remove fixture directory");
}
