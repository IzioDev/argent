//! Adapter between ID-backed app models and the generic route planner.

use std::collections::{BTreeMap, BTreeSet};

use crate::compiler::naming::to_snake;
use crate::compiler::syntax::node::DeclId;
use crate::error::{ArgentError, Result};
use crate::routing::{CommitmentNode, RouteGraph, RoutePlan as PlannerRoutePlan, SelectorRequirement, route_plan};

use super::{ActorModel, AppActors, StaticActorId, TypeTable};

#[cfg(test)]
mod tests;

/// A state-local actor family represented by one ordered route table.
///
/// Entry actors remain direct while table actors are committed in table order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteFamily {
    pub(crate) id: String,
    pub(crate) state_id: DeclId,
    pub(crate) state: String,
    pub(crate) rep_id: DeclId,
    pub(crate) rep: String,
    pub(crate) actor_ids: Vec<DeclId>,
    pub(crate) actors: Vec<String>,
    pub(crate) entry_actors: Vec<String>,
    pub(crate) table_actors: Vec<String>,
    pub(crate) table_actor_ids: Vec<DeclId>,
}

impl RouteFamily {
    /// Return the actor representing this family.
    pub(crate) fn rep(&self) -> &str {
        &self.rep
    }

    /// Return the serialized byte length of the route table.
    pub(crate) fn table_byte_len(&self) -> usize {
        self.table_actor_ids.len() * 32
    }
}

/// One selected root in an actor-carried route commitment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RouteRootLeaf {
    Actor(DeclId),
    Family(String),
}

/// Operations that transform one actor's route cut into another's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CompilerRouteTransition {
    pub(crate) families_to_open: Vec<String>,
    pub(crate) families_to_pack: Vec<String>,
}

/// Compiler route families, actor cuts, and transitions derived for one app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompilerRoutePlan {
    pub(crate) families: Vec<RouteFamily>,
    pub(crate) leaves_by_actor: BTreeMap<DeclId, Vec<RouteRootLeaf>>,
    pub(crate) transitions: BTreeMap<(DeclId, DeclId), CompilerRouteTransition>,
}

/// Injection point between compiler route modeling and generic route planning.
pub(crate) type CompilerRoutePlanner =
    dyn Fn(&RouteGraph, &BTreeMap<String, Vec<String>>, &[SelectorRequirement]) -> Result<PlannerRoutePlan>;

pub(crate) fn default_route_planner(
    graph: &RouteGraph,
    domains: &BTreeMap<String, Vec<String>>,
    selectors: &[SelectorRequirement],
) -> Result<PlannerRoutePlan> {
    route_plan(graph, domains, selectors).map_err(|err| ArgentError::new(err.to_string()))
}

pub(crate) fn infer_direct_routes<'a>(
    actor_models: &BTreeMap<DeclId, ActorModel<'a>>,
    app_actors: &AppActors,
    types: &TypeTable,
    route_planner: &CompilerRoutePlanner,
) -> Result<CompilerRoutePlan> {
    let mut graph = RouteGraph::default();
    let mut domains = BTreeMap::<String, Vec<String>>::new();
    let mut selector_requirements = Vec::new();
    let mut transition_pairs = BTreeSet::new();

    for actor_name in app_actors.iter() {
        let actor_id = types.names[actor_name];
        let actor_model = actor_models.get(&actor_id).expect("selected app actor has a model");
        debug_assert_eq!(actor_model.id, actor_id);
        let actor = actor_model.source();
        // Route-isolated actors still need an empty cut in the final plan.
        graph.add_actor(actor.name.clone());
        domains.entry(types.display_names[&actor_model.state].clone()).or_default().push(actor.name.clone());
        for entry_model in actor_model.entries() {
            // Selectors constrain the table shape independently of concrete
            // relations contributed by this entry's interaction groups.
            for selector in entry_model.template_selectors().values() {
                let variants = selector.variant_actor_ids()?;
                if variants.is_empty() {
                    return Err(ArgentError::new(format!("actor selector `{}` has no variants", selector.name)));
                }
                let names = variants
                    .iter()
                    .map(|variant| {
                        let StaticActorId::InApp(id) = variant else {
                            return Err(ArgentError::new(format!(
                                "actor selector `{}` targets an actor outside the selected app",
                                selector.name
                            )));
                        };
                        if types.actor_states.get(id) != Some(&actor_model.state) {
                            return Err(ArgentError::new(format!(
                                "actor selector `{}` targets a different source state",
                                selector.name
                            )));
                        }
                        app_actors
                            .name(*id)
                            .map(str::to_string)
                            .ok_or_else(|| ArgentError::new(format!("actor selector `{}` has an unknown actor target", selector.name)))
                    })
                    .collect::<Result<Vec<_>>>()?;
                selector_requirements.push(SelectorRequirement {
                    domain: types.display_names[&actor_model.state].clone(),
                    source: actor.name.clone(),
                    variants: names,
                });
            }
            for group in entry_model.groups() {
                for interaction in group.inputs() {
                    for target in interaction.target().static_actors() {
                        let StaticActorId::InApp(target_id) = target else { continue };
                        let Some(target_name) = app_actors.name(*target_id) else { continue };
                        // A single-actor covenant already authenticates its only
                        // possible template, so its self-input needs no route leaf.
                        if !app_actors.is_singleton_actor_self_target(actor_model.id, *target_id) {
                            graph.add_consume(actor.name.clone(), target_name.to_string());
                        }
                    }
                }
                for interaction in group.outputs() {
                    for target in interaction.target().static_actors() {
                        let StaticActorId::InApp(target_id) = target else { continue };
                        let Some(target_name) = app_actors.name(*target_id) else { continue };
                        // A self-output adds no dependency edge. Still plan its no-op
                        // cut transition so an actor-enum output may select the current actor.
                        if actor_id != *target_id {
                            graph.add_emit(actor.name.clone(), target_name.to_string());
                        }
                        transition_pairs.insert((actor.name.clone(), target_name.to_string()));
                    }
                }
            }
        }
    }

    let plan = route_planner(&graph, &domains, &selector_requirements)?;
    let leaves_by_actor = compiler_route_leaves(&plan, &types.names)?;
    let transitions = transition_pairs
        .into_iter()
        .map(|(source, target)| {
            let transition = compiler_route_transition(&plan, &source, &target)?;
            Ok(((types.names[&source], types.names[&target]), transition))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let families = plan
        .families
        .into_iter()
        .map(|family| -> Result<RouteFamily> {
            let table_actors = family.table.iter().cloned().collect::<BTreeSet<_>>();
            let entry_actors = family.members.iter().filter(|actor| !table_actors.contains(*actor)).cloned().collect();
            let state_id = types
                .names
                .get(&family.domain)
                .copied()
                .ok_or_else(|| ArgentError::new(format!("route planner returned unknown state domain `{}`", family.domain)))?;
            let rep_id = types
                .names
                .get(&family.rep)
                .copied()
                .ok_or_else(|| ArgentError::new(format!("route planner returned unknown representative `{}`", family.rep)))?;
            let actor_ids = family
                .members
                .iter()
                .map(|actor| {
                    types
                        .names
                        .get(actor)
                        .copied()
                        .ok_or_else(|| ArgentError::new(format!("route planner returned unknown actor `{actor}`")))
                })
                .collect::<Result<Vec<_>>>()?;
            let table_actor_ids = family
                .table
                .iter()
                .map(|actor| {
                    types
                        .names
                        .get(actor)
                        .copied()
                        .ok_or_else(|| ArgentError::new(format!("route planner returned unknown table actor `{actor}`")))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(RouteFamily {
                id: route_template_family_receipt_id(&family.domain, &family.rep),
                state_id,
                state: family.domain,
                rep_id,
                actor_ids,
                actors: family.members,
                entry_actors,
                rep: family.rep,
                table_actors: family.table,
                table_actor_ids,
            })
        })
        .collect::<Result<_>>()?;

    Ok(CompilerRoutePlan { families, leaves_by_actor, transitions })
}

fn compiler_route_leaves(
    plan: &PlannerRoutePlan,
    actor_ids: &BTreeMap<String, DeclId>,
) -> Result<BTreeMap<DeclId, Vec<RouteRootLeaf>>> {
    let mut leaves_by_actor = BTreeMap::new();
    for actor in plan.commitments.cuts.keys() {
        let nodes = plan.commitments.cut_nodes(actor).expect("an actor with a planned cut must resolve its cut nodes");
        let mut leaves = Vec::new();
        for node in nodes {
            leaves.push(compiler_route_leaf(plan, node, actor_ids)?);
        }
        let id = actor_ids.get(actor).ok_or_else(|| ArgentError::new(format!("route planner returned unknown actor `{actor}`")))?;
        leaves_by_actor.insert(*id, leaves);
    }
    Ok(leaves_by_actor)
}

fn compiler_route_transition(plan: &PlannerRoutePlan, source: &str, target: &str) -> Result<CompilerRouteTransition> {
    let transition = plan.commitments.cut_transition(source, target).map_err(|err| ArgentError::new(err.to_string()))?;
    let families_to_open =
        transition.branches_to_open.into_iter().map(|branch| compiler_route_family_id(plan, branch)).collect::<Result<Vec<_>>>()?;
    let families_to_pack =
        transition.branches_to_pack.into_iter().map(|branch| compiler_route_family_id(plan, branch)).collect::<Result<Vec<_>>>()?;
    Ok(CompilerRouteTransition { families_to_open, families_to_pack })
}

fn compiler_route_family_id(plan: &PlannerRoutePlan, branch: &CommitmentNode) -> Result<String> {
    let CommitmentNode::Branch { children } = branch else {
        return Err(ArgentError::new("commitment transition operation must reference a route family branch"));
    };
    let table = children
        .iter()
        .map(|child| match child {
            CommitmentNode::Leaf { actor } => Ok(actor.clone()),
            CommitmentNode::Branch { .. } => Err(ArgentError::new("nested commitment families cannot be lowered by the compiler")),
        })
        .collect::<Result<Vec<_>>>()?;
    let family = plan
        .families
        .iter()
        .find(|family| family.table == table)
        .ok_or_else(|| ArgentError::new(format!("commitment branch {:?} has no matching route family", table)))?;
    Ok(route_template_family_receipt_id(&family.domain, &family.rep))
}

fn compiler_route_leaf(plan: &PlannerRoutePlan, node: &CommitmentNode, actor_ids: &BTreeMap<String, DeclId>) -> Result<RouteRootLeaf> {
    match node {
        CommitmentNode::Leaf { actor } => actor_ids
            .get(actor)
            .copied()
            .map(RouteRootLeaf::Actor)
            .ok_or_else(|| ArgentError::new(format!("route planner returned unknown actor `{actor}`"))),
        CommitmentNode::Branch { .. } => Ok(RouteRootLeaf::Family(compiler_route_family_id(plan, node)?)),
    }
}

fn route_template_family_receipt_id(state: &str, rep_actor: &str) -> String {
    format!("route_family/{state}/{}", to_snake(rep_actor))
}
