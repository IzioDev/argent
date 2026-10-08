//! Stable identities for authored syntax within one compilation graph.

use std::collections::BTreeMap;

use super::body::RouteId;
use super::source::{Origin, SourceId};

/// Index of a module in the frozen source set.
#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ModuleId(usize);

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum SymbolKind {
    Const,
    State,
    Function,
    Actor,
    ActorEnum,
    App,
}

/// A top-level declaration, indexed within its module and declaration kind.
#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct DeclId {
    pub(crate) module: ModuleId,
    pub(crate) kind: SymbolKind,
    /// Index within the module's list for this kind.
    pub(crate) index: usize,
}

/// An entry within an authored actor declaration.
#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct EntryId {
    pub(crate) actor: DeclId,
    pub(crate) index: usize,
}

/// Dense index of one authored structural site within a module.
#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct NodeId {
    pub(crate) module: ModuleId,
    pub(crate) index: usize,
}

/// An address in the authored tree, independent of a node's source range.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct NodeAddress {
    pub(crate) owner: DeclId,
    pub(crate) root: RootSlot,
    pub(crate) children: Vec<ChildEdge>,
}

/// A stable walk to a site in a declaration, independent of source offsets.
#[derive(Debug, Clone)]
pub(crate) struct SourceNodeCursor {
    pub(crate) address: NodeAddress,
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RootSlot {
    Declaration,
    Name,
    ConstType,
    ConstValue,
    StateBase,
    FieldType(usize),
    FieldName(usize),
    DigestField(usize),
    DigestState(usize),
    ActorState,
    ActorFunction(usize),
    Entry(usize),
    ActorEnumVariant(usize),
    AppActor(usize),
}

/// A typed child of a root site. More edges are added as AST bodies migrate.
#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ChildEdge {
    Name,
    ParamType(usize),
    ParamName(usize),
    ReturnType,
    Body,
    TypeDimension(usize),
    ObserveCovenant(usize),
    Statement(usize),
    Expression,
    ExpressionSource,
    ExpressionLeft,
    ExpressionRight,
    ExpressionCondition,
    ExpressionThen,
    ExpressionElse,
    ExpressionIndex,
    ExpressionStart,
    ExpressionEnd,
    ExpressionLimit,
    Argument(usize),
    Element(usize),
    Field(usize),
    TypeUse,
    BindingName,
    AssignmentTarget,
    CallTarget,
    FieldLabel,
    Route(usize),
    RouteOutput,
    RouteActor,
    RouteState,
    ForeignGroup,
    ThenBranch,
    ElseBranch,
    LoopBinding,
    Consume(usize),
    Spawn(usize),
    SpawnOutput(usize),
    Observe(usize),
    ObservedInput(usize),
    ObservedOutput(usize),
    EmitOutput(usize),
    ActorTarget,
    OpenState,
    Covenant,
    CardinalityMin,
    CardinalityMax,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ReferenceRole {
    Value,
    Call,
    Type,
    Binding,
    AssignmentTarget,
    FieldLabel,
    ActorTarget,
    RouteOutput,
    ClauseTarget,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum SourceOperation {
    InputStateCall,
    PhysicalStateConstructor,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceNode {
    pub(crate) address: NodeAddress,
    pub(crate) origin: Origin,
    #[allow(dead_code)]
    pub(crate) reference_role: Option<ReferenceRole>,
    pub(crate) operation: Option<SourceOperation>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ModuleNodeIndex {
    nodes: Vec<SourceNode>,
    by_address: BTreeMap<NodeAddress, NodeId>,
    route_sites: BTreeMap<(EntryId, RouteId), (NodeId, NodeId)>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct SourceNodeIndex {
    modules: Vec<ModuleNodeIndex>,
}

impl ModuleId {
    pub(crate) fn new(index: usize) -> Self {
        Self(index)
    }

    pub(crate) fn index(self) -> usize {
        self.0
    }
}

impl DeclId {
    pub(crate) fn new(module: ModuleId, kind: SymbolKind, index: usize) -> Self {
        Self { module, kind, index }
    }

    pub(crate) fn kind(self) -> SymbolKind {
        self.kind
    }
}

impl SourceNodeCursor {
    pub(crate) fn new(owner: DeclId, root: RootSlot) -> Self {
        Self { address: NodeAddress { owner, root, children: Vec::new() } }
    }

    pub(crate) fn child(&self, edge: ChildEdge) -> Self {
        let mut address = self.address.clone();
        address.children.push(edge);
        Self { address }
    }
}

impl ModuleNodeIndex {
    pub(crate) fn insert(&mut self, address: NodeAddress, origin: Origin) -> NodeId {
        self.insert_with_role(address, origin, None)
    }

    pub(crate) fn insert_with_role(&mut self, address: NodeAddress, origin: Origin, reference_role: Option<ReferenceRole>) -> NodeId {
        let id = NodeId { module: address.owner.module, index: self.nodes.len() };
        assert!(self.by_address.insert(address.clone(), id).is_none(), "authored node address is unique");
        self.nodes.push(SourceNode { address, origin, reference_role, operation: None });
        id
    }

    pub(crate) fn find(&self, address: &NodeAddress) -> Option<NodeId> {
        self.by_address.get(address).copied()
    }

    pub(crate) fn set_role(&mut self, id: NodeId, role: ReferenceRole) {
        self.nodes[id.index].reference_role = Some(role);
    }

    pub(crate) fn set_operation(&mut self, id: NodeId, operation: SourceOperation) {
        self.nodes[id.index].operation = Some(operation);
    }

    pub(crate) fn insert_route_sites(&mut self, entry: EntryId, route: RouteId, actor: NodeId, state: NodeId) {
        assert!(self.route_sites.insert((entry, route), (actor, state)).is_none(), "authored route identity is unique");
    }

    pub(crate) fn node_origin(&self, id: NodeId) -> Origin {
        self.nodes[id.index].origin
    }
}

impl SourceNodeIndex {
    pub(crate) fn push_module(&mut self, id: ModuleId, module: ModuleNodeIndex) {
        assert_eq!(id.index(), self.modules.len(), "source-node modules follow source IDs");
        debug_assert!(module.nodes.iter().all(|node| node.address.owner.module == id));
        self.modules.push(module);
    }

    pub(crate) fn node(&self, id: NodeId) -> &SourceNode {
        &self.modules[id.module.index()].nodes[id.index]
    }

    pub(crate) fn find(&self, address: &NodeAddress) -> Option<NodeId> {
        self.modules.get(address.owner.module.index())?.by_address.get(address).copied()
    }

    pub(crate) fn route_sites(&self, entry: EntryId, route: RouteId) -> Option<(NodeId, NodeId)> {
        self.modules.get(entry.actor.module.index())?.route_sites.get(&(entry, route)).copied()
    }

    pub(crate) fn declaration_origin(&self, owner: DeclId) -> Origin {
        let address = NodeAddress { owner, root: RootSlot::Declaration, children: Vec::new() };
        let node = self.node(self.find(&address).expect("parsed declaration has an authored root"));
        debug_assert_eq!(node.address, address);
        node.origin
    }

    pub(crate) fn source(&self, owner: DeclId) -> SourceId {
        match self.declaration_origin(owner) {
            Origin::Authored { source, .. } => source,
            Origin::Generated { .. } => unreachable!("source declaration is authored"),
        }
    }
}

#[cfg(test)]
mod tests;
