use std::path::PathBuf;

use crate::compiler::loader::{SourceSet, load_inline_program};
use crate::compiler::model::{AppCompilationContext, SourceFieldId};
use crate::compiler::syntax::AuthoredEntryStatement;
use silverscript_lang::ast::{ExprKind, Statement};

use super::*;

#[test]
fn current_group_position_policy_is_completed_before_emission() {
    for (source, actor_name, entry_name, expected) in [
        (
            include_str!("../../../../tests/fixtures/emit/single_actor_self_consume/app.ag"),
            "Counter",
            "merge",
            CurrentInputGroupPolicy::LeaderFixed { count: 2 },
        ),
        (
            include_str!("../../../../tests/fixtures/emit/single_actor_self_consume/app.ag"),
            "Counter",
            "hold",
            CurrentInputGroupPolicy::Batchable,
        ),
        (
            include_str!("../../../../tests/fixtures/emit/entry_range_inputs/app.ag"),
            "Batch",
            "inspect",
            CurrentInputGroupPolicy::LeaderRanged,
        ),
        (
            include_str!("../../../../tests/fixtures/runtime/context_static_actor_spawn/app.ag"),
            "Child",
            "support_launch",
            CurrentInputGroupPolicy::Delegate { minimum_count: 2 },
        ),
    ] {
        let program = load_inline_program(PathBuf::from("group-policy.ag"), source.to_string()).expect("fixture resolves");
        let model = AppCompilationContext::from_resolved(
            &program,
            None,
            &std::collections::BTreeMap::new(),
            &crate::compiler::model::default_route_planner,
        )
        .expect("input plan completes");
        let actor = model.actor_by_decl(model.types.names[actor_name]).expect("actor exists");
        let entry = actor.entries.iter().find(|entry| entry.name == entry_name).expect("entry exists");
        let policy = model
            .input_plan_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
            })
            .expect("input plan")
            .current_group_policy();
        assert_eq!(policy, expected, "{actor_name}::{entry_name}");
        assert_eq!(policy.slot_offset(), usize::from(!matches!(expected, CurrentInputGroupPolicy::Delegate { .. })));
    }
}

#[test]
fn input_authentication_and_field_availability_are_complete_before_emission() {
    let program = load_inline_program(
        PathBuf::from("input-plan.ag"),
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
    let reader = model.actor_by_decl(model.types.names["Reader"]).expect("Reader exists");
    let inspect = reader.entries.iter().find(|entry| entry.name == "inspect").expect("inspect exists");
    let plan = model
        .input_plan_by_id(crate::compiler::syntax::node::EntryId {
            actor: model.types.names[&reader.name],
            index: reader.entries.iter().position(|candidate| std::ptr::eq(candidate, inspect)).expect("entry belongs to actor"),
        })
        .expect("input plan exists");

    let peer_id = model
        .entry_model_by_id(crate::compiler::syntax::node::EntryId {
            actor: model.types.names[&reader.name],
            index: reader.entries.iter().position(|candidate| std::ptr::eq(candidate, inspect)).expect("entry belongs to actor"),
        })
        .expect("entry model exists")
        .current()
        .inputs()
        .iter()
        .find(|input| input.handle() == "peer")
        .expect("peer interaction exists")
        .id();
    let peer = plan.consumed(peer_id).expect("peer input is planned");
    let InteractionId::CurrentInput(index) = peer_id else { panic!("peer is a current input") };
    assert!(plan.consumed(InteractionId::CurrentInput(index + 100)).is_err());
    assert_eq!(peer.authentication, InputAuthentication::Template);
    assert_eq!(peer.origin, InputReferenceOrigin::Consumed(peer_id));
    assert_eq!(peer.origin.source_path(inspect).expect("bound input path"), ["peer"]);
    assert!(peer.guaranteed);
    let peer_source = model
        .state_lowering_by_id(model.types.names["Reader"])
        .expect("Reader layout")
        .target(&peer.target)
        .expect("peer target")
        .source();
    assert_eq!(peer.fields[&SourceFieldId::new(peer_source.clone(), "left")], InputFieldAvailability::Direct);
    assert_eq!(peer.fields[&SourceFieldId::new(peer_source.clone(), "right")], InputFieldAvailability::Direct);
    assert_eq!(
        &peer.target,
        model
            .state_lowering_by_id(model.types.names["Reader"])
            .expect("Reader layout")
            .target_for_actor(&StaticActorId::InApp(model.types.names["Routed"]))
            .expect("Routed target")
            .id(),
    );
    assert!(plan.first_authenticated_template(&peer.target).is_some());
}

#[test]
fn active_expansion_availability_uses_the_bound_state_source() {
    let program = load_inline_program(
        PathBuf::from("bound-expanded-input.ag"),
        include_str!("../../../../tests/fixtures/emit/state_expansion/app.ag").to_string(),
    )
    .expect("expanded source resolves");
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("expanded input plans");
    let actor = model.actor_by_decl(model.types.names["Forager"]).expect("selected actor");
    let entry = &actor.entries[0];
    let entry_id = model
        .entry_model_by_id(crate::compiler::syntax::node::EntryId {
            actor: model.types.names[&actor.name],
            index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
        })
        .expect("entry model")
        .id;
    let field = SourceFieldId::new(model.source_state_id("ForagerState").expect("source state"), "strategy");
    assert_eq!(
        model
            .input_plan_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor")
            })
            .expect("input plan")
            .active()
            .fields[&field],
        InputFieldAvailability::CheckedPreimage
    );

    let mut copied_actor = actor.clone();
    copied_actor.state = "unrelated_display_name".to_string();
    let copied = EntryInputPlan::new(entry_id, &copied_actor, &copied_actor.entries[0], &model)
        .expect("bound expansion is independent of copied state spelling");
    assert_eq!(copied.active().fields[&field], InputFieldAvailability::CheckedPreimage);
}

#[test]
fn complete_authored_input_availability_comes_from_the_model_layout() {
    let augmented = r#"
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
    "#;
    for (source, actor_name, entry_name, expected) in [
        (include_str!("../../../../tests/fixtures/emit/single_actor_self_consume/app.ag"), "Counter", "merge", true),
        (include_str!("../../../../tests/fixtures/emit/input_template_route_reuse/app.ag"), "Controller", "step", true),
        (augmented, "Left", "shift", false),
    ] {
        let program = load_inline_program(PathBuf::from("direct-input-plan.ag"), source.to_string()).expect("fixture resolves");
        let model = AppCompilationContext::from_resolved(
            &program,
            None,
            &std::collections::BTreeMap::new(),
            &crate::compiler::model::default_route_planner,
        )
        .expect("input plan completes");
        let actor = model.actor_by_decl(model.types.names[actor_name]).expect("actor exists");
        let entry = actor.entries.iter().find(|entry| entry.name == entry_name).expect("entry exists");
        let interaction = model
            .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
            })
            .expect("entry model")
            .current()
            .inputs()[0]
            .id();
        let reference = model
            .input_plan_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
            })
            .expect("input plan")
            .consumed(interaction)
            .expect("consumed input");
        assert_eq!(reference.direct_authored_state, expected, "{actor_name}::{entry_name}");
        if actor_name == "Counter" {
            assert_eq!(reference.authentication, InputAuthentication::CovenantDomain);
            assert!(
                model
                    .input_plan_by_id(crate::compiler::syntax::node::EntryId {
                        actor: model.types.names[&actor.name],
                        index: actor
                            .entries
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, entry))
                            .expect("entry belongs to actor")
                    })
                    .expect("input plan")
                    .first_authenticated_template(&reference.target)
                    .is_none()
            );
        }
    }
}

#[test]
fn ranged_input_literal_bounds_are_checked_before_lowering() {
    for index in ["2", "-1"] {
        let source = include_str!("../../../../tests/fixtures/emit/entry_range_inputs_outputs/app.ag")
            .replace("accounts[i].value", &format!("accounts[{index}].value"));
        let sources = SourceSet::discover_inline(PathBuf::from("input-range-bounds.ag"), source).expect("source discovers");
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
            .expect_err("an out-of-range input item fails during model construction");
        assert!(
            error.to_string().contains(&format!("range `accounts` index `{index}` is outside its declared positions `0..2`")),
            "{error}"
        );
    }
}

#[test]
fn range_check_helper_requirement_is_decided_by_the_model() {
    for (name, source, actor_name, expected) in [
        ("input-output-ranges", include_str!("../../../../tests/fixtures/emit/entry_range_inputs_outputs/app.ag"), "Batch", true),
        ("output-ranges", include_str!("../../../../tests/fixtures/emit/entry_range_outputs/app.ag"), "Batch", true),
        ("singleton", include_str!("../../../../tests/fixtures/emit/single_actor_self_consume/app.ag"), "Counter", false),
    ] {
        let program = load_inline_program(PathBuf::from(format!("{name}.ag")), source.to_string()).expect("fixture resolves");
        let model = AppCompilationContext::from_resolved(
            &program,
            None,
            &std::collections::BTreeMap::new(),
            &crate::compiler::model::default_route_planner,
        )
        .expect("entry plan completes");
        let actor = model.actor_by_decl(model.types.names[actor_name]).expect("actor exists");
        let entry = &actor.entries[0];
        assert_eq!(
            model
                .input_plan_by_id(crate::compiler::syntax::node::EntryId {
                    actor: model.types.names[&actor.name],
                    index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor")
                })
                .expect("input plan")
                .requires_checked_range_index(),
            expected,
            "{name}"
        );
    }
}

#[test]
fn ranged_consumed_proof_position_includes_the_leader_input() {
    for (name, source, range_handle, expected) in [
        ("range-first", include_str!("../../../../tests/fixtures/emit/entry_range_inputs_outputs/app.ag"), "accounts", 1),
        ("range-after-fixed", include_str!("../../../../tests/fixtures/emit/entry_range_inputs/app.ag"), "accounts", 2),
    ] {
        let program = load_inline_program(PathBuf::from(format!("{name}.ag")), source.to_string()).expect("fixture resolves");
        let model = AppCompilationContext::from_resolved(
            &program,
            None,
            &std::collections::BTreeMap::new(),
            &crate::compiler::model::default_route_planner,
        )
        .expect("input plan completes");
        let actor = model.actor_by_decl(model.types.names["Batch"]).expect("Batch exists");
        let entry = &actor.entries[0];
        let interaction = model
            .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
            })
            .expect("entry model")
            .current()
            .inputs()
            .iter()
            .find(|input| input.handle() == range_handle)
            .expect("range interaction");
        let reference = model
            .input_plan_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
            })
            .expect("input plan")
            .consumed(interaction.id())
            .expect("consumed range");
        assert_eq!(reference.ranged_proof_input_position, Some(expected), "{name}");
    }
}

#[test]
fn observed_covenant_source_uses_its_group_identity() {
    let sources = SourceSet::discover_inline(
        PathBuf::from("observed-group.ag"),
        include_str!("../../../../tests/fixtures/emit/observed_template_witnesses/app.ag").to_string(),
    )
    .expect("fixture discovers");
    sources
        .with_resolved(|program| {
            let model = AppCompilationContext::from_resolved(
                &program,
                None,
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )?;
            let actor = model.actor_by_decl(model.types.names["Local"])?;
            let entry = actor.entries.iter().find(|entry| entry.name == "step").expect("step entry");
            let group = model
                .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                    actor: model.types.names[&actor.name],
                    index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
                })?
                .existing_groups()
                .next()
                .expect("observed group");
            let inputs = model.input_plan_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
            })?;
            let observed = group.inputs().first().expect("observed input");
            let requirement = inputs.observed(observed.id())?;
            assert_eq!(requirement.origin, InputReferenceOrigin::Observed(observed.id()));
            assert_eq!(requirement.origin.source_path(entry)?, ["asset", "inputs", "src"]);
            let body = model.resolution.entry_body(
                model
                    .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                        actor: model.types.names[&actor.name],
                        index: actor
                            .entries
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, entry))
                            .expect("entry belongs to actor"),
                    })?
                    .id,
            )?;
            let AuthoredEntryStatement::Sil(statement) = &body[0] else { panic!("entry starts with an authored declaration") };
            let Statement::VariableDefinition { expr: Some(state_call), .. } = statement.as_ref() else {
                panic!("entry starts with a state value")
            };
            let ExprKind::Call { args, .. } = &state_call.kind else { panic!("observed input is read through state(...)") };
            let site = (args[0].span.start(), args[0].span.end());
            assert_eq!(inputs.reference_uses.get(&site), Some(&requirement.id));
            assert_eq!(
                inputs.observed_covenant_source(group.id())?,
                &CovenantIdSource::StateField { field: SourceFieldId::new(model.source_state_id("LocalState")?, "target_id") },
            );
            assert!(inputs.observed_covenant_source(CovenantGroupId::Existing(100)).is_err());
            assert!(inputs.observed_covenant_source(CovenantGroupId::Genesis(0)).is_err());
            Ok(())
        })
        .expect("observed covenant source is complete before lowering");
}

#[test]
fn unavailable_field_names_in_comments_and_strings_do_not_reject_the_model() {
    let source = r#"
            state Capsule { int nonce; virtual detail; }
            state Details { int count; }
            state Expanded expands Capsule { detail: Details; }

            actor Vault owns Expanded {
                entry hold() emits none { require(nonce >= 0); }
            }

            state ReaderState { int nonce; }
            actor Reader owns ReaderState {
                entry inspect() consumes { vault: Vault, } emits next: Reader {
                    require(vault.nonce >= 0); // vault.detail.count is unavailable
                    require("vault.detail" == "vault.detail");
                    unrestricted(next.value);
                    become next <- self;
                }
            }

            app Test { actor Vault; actor Reader; }
        "#;
    let sources =
        SourceSet::discover_inline(PathBuf::from("input-field-mentions.ag"), source.to_string()).expect("source discovery succeeds");
    sources
        .with_resolved(|program| {
            AppCompilationContext::from_resolved(
                &program,
                None,
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )?;
            Ok(())
        })
        .expect("comments and strings are not field reads");

    let field_use = source.replace("require(vault.nonce >= 0);", "require(vault.detail.count >= 0);");
    let sources = SourceSet::discover_inline(PathBuf::from("input-field-use.ag"), field_use).expect("source discovery succeeds");
    let err = sources
        .with_resolved(|program| {
            AppCompilationContext::from_resolved(
                &program,
                None,
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )
            .map(|_| ())
        })
        .expect_err("an unavailable field read is rejected before emission");
    assert!(err.to_string().contains("expanded input field `detail`"), "unexpected error: {err}");
}
