use super::*;
use crate::compiler::syntax::node::{ModuleId, SymbolKind};

#[test]
fn compiler_route_leaves_preserve_packed_and_opened_commitment_nodes() {
    let mut graph = RouteGraph::default();
    graph.add_actor("Knight");
    graph.add_emit("Player", "Mux");
    graph.add_emit("Mux", "Knight");
    graph.add_emit("Mux", "Pawn");
    graph.add_emit("Pawn", "Mux");
    graph.add_emit("Mux", "Settle");
    let domains = BTreeMap::from([
        ("BoardState".to_string(), ["Knight", "Mux", "Pawn"].into_iter().map(str::to_string).collect()),
        ("PlayerState".to_string(), vec!["Player".to_string()]),
        ("SettleState".to_string(), vec!["Settle".to_string()]),
    ]);

    let plan = route_plan(&graph, &domains, &[]).expect("route plan is valid");
    let actor_ids = ["Knight", "Player", "Mux", "Pawn", "Settle"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name.to_string(), DeclId::new(ModuleId::new(0), SymbolKind::Actor, index)))
        .collect::<BTreeMap<_, _>>();
    let leaves = compiler_route_leaves(&plan, &actor_ids).expect("commitment nodes lower to compiler leaves");

    assert_eq!(
        leaves[&actor_ids["Player"]],
        [
            RouteRootLeaf::Family("route_family/BoardState/mux".to_string()),
            RouteRootLeaf::Actor(actor_ids["Mux"]),
            RouteRootLeaf::Actor(actor_ids["Settle"]),
        ]
    );
    assert_eq!(
        leaves[&actor_ids["Mux"]],
        [
            RouteRootLeaf::Actor(actor_ids["Knight"]),
            RouteRootLeaf::Actor(actor_ids["Pawn"]),
            RouteRootLeaf::Actor(actor_ids["Mux"]),
            RouteRootLeaf::Actor(actor_ids["Settle"]),
        ]
    );
    assert!(leaves[&actor_ids["Settle"]].is_empty());

    let family_id = "route_family/BoardState/mux".to_string();
    assert_eq!(
        compiler_route_transition(&plan, "Player", "Mux").expect("Player can open the Mux family"),
        CompilerRouteTransition { families_to_open: vec![family_id.clone()], families_to_pack: Vec::new() }
    );
    assert_eq!(
        compiler_route_transition(&plan, "Mux", "Player").expect("Mux can pack its family for Player"),
        CompilerRouteTransition { families_to_open: Vec::new(), families_to_pack: vec![family_id] }
    );
}

#[test]
fn planner_labels_must_belong_to_the_selected_actor_domain() {
    let mut graph = RouteGraph::default();
    graph.add_actor("Unexpected");
    let domains = BTreeMap::from([("State".to_string(), vec!["Unexpected".to_string()])]);
    let plan = route_plan(&graph, &domains, &[]).expect("generic route plan is valid");

    let error = compiler_route_leaves(&plan, &BTreeMap::new()).expect_err("unknown planner actor must be rejected");
    assert!(error.to_string().contains("unknown actor `Unexpected`"), "{error}");
}
