use std::path::PathBuf;

use crate::compiler::loader::{SourceSet, load_inline_program};
use crate::compiler::model::AppCompilationContext;

use super::*;

#[test]
fn observed_output_proof_follows_bound_actor_source() {
    let original = include_str!("../../../../tests/fixtures/emit/open_observed_state_handle/app.ag").replace("\r\n", "\n");
    let different_source = original
        .replace("entry advance()", "entry advance(actor_type<AgentCapsule> other_type)")
        .replace("outputs {\n            agent: self.agent_type,", "outputs {\n            agent: other_type,")
        .replace("agent <- self.agent_type(next_state)", "agent <- other_type(next_state)");
    let open_binding = include_str!("../../../../tests/fixtures/emit/open_observed_actor_binding/app.ag").to_string();
    for (path, source, shared) in
        [("shared.ag", original, true), ("distinct.ag", different_source, false), ("open.ag", open_binding, true)]
    {
        let sources = SourceSet::discover_inline(PathBuf::from(path), source).expect("fixture discovers");
        sources
            .with_resolved(|program| {
                let model = AppCompilationContext::from_resolved(
                    &program,
                    None,
                    &std::collections::BTreeMap::new(),
                    &crate::compiler::model::default_route_planner,
                )?;
                let actor = model.actor_by_decl(model.types.names["Cell"])?;
                let entry = actor.entries.iter().find(|entry| entry.name == "advance").expect("advance exists");
                let entry_model = model.entry_model_by_id(crate::compiler::syntax::node::EntryId {
                    actor: model.types.names[&actor.name],
                    index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
                })?;
                let group = entry_model.existing_groups().next().expect("remote observe group");
                let input = group.inputs()[0].id();
                let output = group.outputs()[0].id();
                let bindings = model.resolution.bindings(entry_model.id.actor);
                let input_site = input.actor_target_site(entry_model.id, model.resolution)?;
                let output_site = output.actor_target_site(entry_model.id, model.resolution)?;
                assert_eq!(bindings.local_actor_targets.get(&input_site) == bindings.local_actor_targets.get(&output_site), shared);
                let proof = model
                    .entry_output_plan_by_id(crate::compiler::syntax::node::EntryId {
                        actor: model.types.names[&actor.name],
                        index: actor
                            .entries
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, entry))
                            .expect("entry belongs to actor"),
                    })?
                    .observed(output)?;
                if shared {
                    assert_eq!(
                        proof,
                        &OutputProofRequirement::BoundObserved(
                            model
                                .input_plan_by_id(crate::compiler::syntax::node::EntryId {
                                    actor: model.types.names[&actor.name],
                                    index: actor
                                        .entries
                                        .iter()
                                        .position(|candidate| std::ptr::eq(candidate, entry))
                                        .expect("entry belongs to actor")
                                })?
                                .observed(input)?
                                .id
                        )
                    );
                } else {
                    assert_eq!(proof, &OutputProofRequirement::ObservedWitness);
                }
                Ok(())
            })
            .expect("observed output proof is selected from bound actor source");
    }
}

#[test]
fn output_proofs_and_physical_targets_are_selected_without_emission() {
    let sources = SourceSet::discover_inline(
        PathBuf::from("output-plan.ag"),
        include_str!("../../../../tests/fixtures/state_layout/function_contexts/app.ag").to_string(),
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
            let routed = model.actor_by_decl(model.types.names["Routed"])?;
            let advance = routed.entries.iter().find(|entry| entry.name == "advance").expect("advance exists");
            let export = routed.entries.iter().find(|entry| entry.name == "export").expect("export exists");

            assert_eq!(
                model
                    .entry_output_plan_by_id(crate::compiler::syntax::node::EntryId {
                        actor: model.types.names[&routed.name],
                        index: routed
                            .entries
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, advance))
                            .expect("entry belongs to actor")
                    })?
                    .actor(&StaticActorId::InApp(model.types.names["Routed"]))?,
                &OutputProofRequirement::Current,
            );
            assert_eq!(
                model
                    .entry_output_plan_by_id(crate::compiler::syntax::node::EntryId {
                        actor: model.types.names[&routed.name],
                        index: routed
                            .entries
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, export))
                            .expect("entry belongs to actor")
                    })?
                    .actor(&StaticActorId::InApp(model.types.names["Foreign"]))?,
                &OutputProofRequirement::WitnessedActor(StaticActorId::InApp(model.types.names["Foreign"])),
            );
            let foreign = model
                .output_plan_by_id(model.types.names[&routed.name])?
                .actor(&StaticActorId::InApp(model.types.names["Foreign"]))?;
            assert_eq!(foreign.physical.source().as_str(), "ForeignState");
            assert_eq!(
                &foreign.target,
                model
                    .state_lowering_by_id(model.types.names["Routed"])?
                    .target_for_actor(&StaticActorId::InApp(model.types.names["Foreign"]))
                    .expect("target")
                    .id()
            );
            Ok(())
        })
        .expect("selected app plans");
}

#[test]
fn current_output_count_checks_are_completed_in_the_model() {
    for (name, source, actor_name, entry_name, exact, range, coordinated) in [
        (
            "ranged-only",
            include_str!("../../../../tests/fixtures/emit/entry_range_inputs_outputs/app.ag"),
            "Batch",
            "rebalance",
            None,
            Some((1, 3, 0)),
            true,
        ),
        (
            "ranged-with-singletons",
            include_str!("../../../../tests/fixtures/emit/entry_range_outputs/app.ag"),
            "Batch",
            "distribute",
            None,
            Some((1, 3, 2)),
            false,
        ),
        (
            "singleton",
            include_str!("../../../../tests/fixtures/emit/single_actor_self_consume/app.ag"),
            "Counter",
            "merge",
            Some(1),
            None,
            true,
        ),
    ] {
        let program = load_inline_program(PathBuf::from(format!("{name}.ag")), source.to_string()).expect("fixture resolves");
        let model = AppCompilationContext::from_resolved(
            &program,
            None,
            &std::collections::BTreeMap::new(),
            &crate::compiler::model::default_route_planner,
        )
        .expect("output plan completes");
        let actor = model.actor_by_decl(model.types.names[actor_name]).expect("actor exists");
        let entry = actor.entries.iter().find(|entry| entry.name == entry_name).expect("entry exists");
        let plan = model
            .entry_output_plan_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
            })
            .expect("entry output plan");
        assert_eq!(plan.exact_current_output_count(), exact, "{name}");
        assert_eq!(plan.coordinates_current_outputs(), coordinated, "{name}");
        let ranges = model
            .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                actor: model.types.names[&actor.name],
                index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
            })
            .expect("entry model")
            .current()
            .outputs()
            .iter()
            .filter_map(|output| plan.current_output_range(output.id()))
            .collect::<Vec<_>>();
        assert_eq!(ranges, range.into_iter().collect::<Vec<_>>(), "{name}");
    }
}

#[test]
fn output_value_policy_is_checked_from_authored_ast_during_model_construction() {
    let source = r#"
        state FooState {}
        actor Foo owns FooState {
            entry step() emits next: Foo {
                require(1 == 1 /* next.value */);
                become next <- self;
            }
        }
        app Test { actor Foo; }
    "#;
    let sources =
        SourceSet::discover_inline(PathBuf::from("output-policy.ag"), source.to_string()).expect("source discovery succeeds");
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
        .expect_err("an unused output value fails before code generation");
    assert!(error.to_string().contains("must reference output value `next.value`"), "unexpected error: {error}");

    let invalid = source.replace("require(1 == 1 /* next.value */);", "unrestricted(self.value);");
    let sources = SourceSet::discover_inline(PathBuf::from("output-policy-invalid.ag"), invalid).expect("source discovery succeeds");
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
        .expect_err("an unrelated output value cannot be declared unrestricted");
    assert!(error.to_string().contains("`self.value` is not one"), "unexpected error: {error}");

    let permitted = source.replace("require(1 == 1 /* next.value */);", "unrestricted(next.value);");
    let sources =
        SourceSet::discover_inline(PathBuf::from("output-policy-permitted.ag"), permitted).expect("source discovery succeeds");
    sources
        .with_resolved(|program| {
            let model = AppCompilationContext::from_resolved(
                &program,
                None,
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )?;
            let actor = model.actor_by_decl(model.types.names["Foo"])?;
            let entry = &actor.entries[0];
            let output = model
                .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                    actor: model.types.names[&actor.name],
                    index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
                })?
                .current()
                .outputs()[0]
                .id();
            assert!(
                model
                    .entry_output_plan_by_id(crate::compiler::syntax::node::EntryId {
                        actor: model.types.names[&actor.name],
                        index: actor
                            .entries
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, entry))
                            .expect("entry belongs to actor")
                    })?
                    .value_uses
                    .values()
                    .any(|id| *id == output)
            );
            Ok(())
        })
        .expect("the explicit output-value policy is complete before emission");
}

#[test]
fn output_range_policy_uses_bound_handle_and_ast_index() {
    for (replacement, index) in
        [("unrestricted(next[3].value);", "3"), ("require(next[3].value >= 0);", "3"), ("unrestricted(next[-1].value);", "-1")]
    {
        let source = include_str!("../../../../tests/fixtures/emit/entry_range_inputs_outputs/app.ag")
            .replace("unrestricted(next[0].value);", replacement);
        let sources = SourceSet::discover_inline(PathBuf::from("output-range-policy.ag"), source).expect("source discovers");
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
            .expect_err("an out-of-range output value fails during model construction");
        assert!(
            error.to_string().contains(&format!("range `next` index `{index}` is outside its declared positions `0..3`")),
            "{error}"
        );
    }
}

#[test]
fn selector_output_proof_uses_the_bound_local() {
    let program = load_inline_program(
        PathBuf::from("selector-proof.ag"),
        r#"
        state Game { int n; }
        actor A owns Game { entry hold() emits none {} }
        actor B owns Game { entry hold() emits none {} }
        actor enum Move { A; B; }
        actor Mux owns Game {
            entry choose(Move target) emits next: Move {
                unrestricted(next.value);
                Game next_state = { n: n + 1 };
                become next <- target(next_state);
            }
        }
        app Test { actor A; actor B; actor Mux; }
        "#
        .to_string(),
    )
    .expect("source resolves");
    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("model plans");
    let actor = model.actor_by_decl(model.types.names["Mux"]).expect("mux actor");
    let entry = &actor.entries[0];
    let selector = &model
        .entry_model_by_id(crate::compiler::syntax::node::EntryId {
            actor: model.types.names[&actor.name],
            index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
        })
        .expect("entry model")
        .template_selectors()["target"];
    let id = selector.binding.expect("selector is bound");
    let proofs = model
        .entry_output_plan_by_id(crate::compiler::syntax::node::EntryId {
            actor: model.types.names[&actor.name],
            index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
        })
        .expect("output proofs");
    assert_eq!(proofs.selector(selector).expect("selector proof"), &OutputProofRequirement::Selector(id));
    let targets = model.output_plan_by_id(model.types.names[&actor.name]).expect("output targets");
    assert!(matches!(targets.selector(selector).expect("selector target").target, PhysicalTargetId::ActorDomain { .. }));

    let mut different_binding = selector.clone();
    different_binding.binding = Some(crate::compiler::resolve::LocalId { index: id.index + 1, ..id });
    assert!(proofs.selector(&different_binding).is_err(), "same display name cannot borrow another local's proof");
    assert!(targets.selector(&different_binding).is_err(), "same display name cannot borrow another local's output target");
}

#[test]
fn covenant_output_targets_are_selected_before_lowering() {
    let fixtures = [
        (
            "observed-fixed.ag",
            include_str!("../../../../tests/fixtures/emit/observed_template_witnesses/app.ag"),
            "Local",
            "step",
            "asset",
            "dst",
            "ForeignState",
            false,
        ),
        (
            "observed-open.ag",
            include_str!("../../../../tests/fixtures/emit/open_observed_state_handle/app.ag"),
            "Cell",
            "advance",
            "remote",
            "agent",
            "AgentCapsule",
            false,
        ),
        (
            "spawned-open.ag",
            include_str!("../../../../tests/fixtures/runtime/context_genesis_spawn/app.ag"),
            "Controller",
            "launch",
            "new_pair",
            "left",
            "PairState",
            true,
        ),
    ];
    for (path, text, actor_name, entry_name, group, handle, source_state, spawned) in fixtures {
        let sources = SourceSet::discover_inline(PathBuf::from(path), text.to_string()).expect("fixture discovers");
        sources
            .with_resolved(|program| {
                let model = AppCompilationContext::from_resolved(
                    &program,
                    spawned.then_some("ControllerApp"),
                    &std::collections::BTreeMap::new(),
                    &crate::compiler::model::default_route_planner,
                )?;
                let actor = model.actor_by_decl(model.types.names[actor_name])?;
                let entry = actor.entries.iter().find(|entry| entry.name == entry_name).expect("fixture entry exists");
                let outputs = model.entry_output_plan_by_id(crate::compiler::syntax::node::EntryId {
                    actor: model.types.names[&actor.name],
                    index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
                })?;
                let interaction = model
                    .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                        actor: model.types.names[&actor.name],
                        index: actor
                            .entries
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, entry))
                            .expect("entry belongs to actor"),
                    })?
                    .groups()
                    .find(|candidate| {
                        candidate.observe().map(|observe| observe.name.as_str()) == Some(group)
                            || candidate.spawn().map(|spawn| spawn.name.as_str()) == Some(group)
                    })
                    .expect("fixture covenant group exists")
                    .outputs()
                    .iter()
                    .find(|output| output.handle() == handle)
                    .expect("fixture output exists");
                let id = interaction.id();
                if spawned {
                    assert!(outputs.value_uses.values().any(|value| *value == id), "spawned output value has a bound use site");
                }
                let target = if spawned {
                    outputs.spawned(id)?;
                    outputs.spawned_target(id)?
                } else {
                    outputs.observed(id)?;
                    outputs.observed_witness(id)?;
                    outputs.observed_target(id)?
                };
                assert_eq!(
                    model.output_plan_by_id(model.types.names[&actor.name])?.target(target)?.physical.source(),
                    &model.source_state_id(source_state)?
                );
                let different = match id {
                    InteractionId::ObservedOutput { observe, output } => {
                        InteractionId::ObservedOutput { observe, output: output + 100 }
                    }
                    InteractionId::SpawnedOutput { spawn, output } => InteractionId::SpawnedOutput { spawn, output: output + 100 },
                    _ => panic!("fixture output must be observed or spawned"),
                };
                assert!(
                    if spawned { outputs.spawned(different).is_err() } else { outputs.observed(different).is_err() },
                    "a different output identity cannot borrow the fixture proof",
                );
                Ok(())
            })
            .expect("model selects covenant output target");
    }
}

#[test]
fn spawned_static_actor_template_uses_its_semantic_identity() {
    let sources = SourceSet::discover_inline(
        PathBuf::from("static-spawn.ag"),
        include_str!("../../../../tests/fixtures/runtime/context_static_actor_spawn/app.ag").to_string(),
    )
    .expect("fixture discovers");
    sources
        .with_resolved(|program| {
            let model = AppCompilationContext::from_resolved(
                &program,
                Some("StaticActorSpawn"),
                &std::collections::BTreeMap::new(),
                &crate::compiler::model::default_route_planner,
            )?;
            let actor = model.actor_by_decl(model.types.names["Launcher"])?;
            let entry = actor.entries.iter().find(|entry| entry.name == "launch").expect("launch entry");
            let output = &model
                .entry_model_by_id(crate::compiler::syntax::node::EntryId {
                    actor: model.types.names[&actor.name],
                    index: actor.entries.iter().position(|candidate| std::ptr::eq(candidate, entry)).expect("entry belongs to actor"),
                })?
                .genesis_groups()
                .next()
                .expect("spawn group")
                .outputs()[0];
            assert_eq!(
                model
                    .entry_output_plan_by_id(crate::compiler::syntax::node::EntryId {
                        actor: model.types.names[&actor.name],
                        index: actor
                            .entries
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, entry))
                            .expect("entry belongs to actor")
                    })?
                    .spawned_actor(output.id())?,
                Some(&StaticActorId::InApp(model.types.names["Child"])),
            );
            assert_eq!(
                model
                    .witness_plan_by_id(crate::compiler::syntax::node::EntryId {
                        actor: model.types.names[&actor.name],
                        index: actor
                            .entries
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, entry))
                            .expect("entry belongs to actor")
                    })?
                    .spawn_output(output.id())?
                    .id,
                output.id()
            );
            Ok(())
        })
        .expect("static spawn has a bound actor template");
}
