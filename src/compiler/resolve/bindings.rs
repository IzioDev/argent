//! Source reference bindings, including lexical scopes in the retained AST.

use super::*;
use crate::compiler::syntax::node::{ChildEdge, NodeId, RootSlot, SourceNodeCursor};
use crate::compiler::syntax::source::Origin;
use silverscript_lang::ast::{self as sil, ExprKind, Statement};

impl DeclarationBinder<'_, '_> {
    fn node_id(&self, cursor: &SourceNodeCursor) -> Result<NodeId> {
        self.program
            .nodes()
            .find(&cursor.address)
            .ok_or_else(|| ArgentError::at(self.program.declaration_path(self.owner), "missing indexed source reference"))
    }

    fn local(&mut self, scope: &mut BTreeMap<String, Binding>, name: &str, callable: RootSlot) -> LocalId {
        let id = LocalId { owner: self.owner, callable, index: self.next_local };
        self.next_local += 1;
        scope.insert(name.to_string(), Binding::Local(id));
        self.bindings.local_names.insert(id, name.to_string());
        id
    }

    fn local_at(&mut self, scope: &mut BTreeMap<String, Binding>, name: &str, cursor: &SourceNodeCursor) -> Result<()> {
        let id = self.local(scope, name, cursor.address.root);
        let site = self.node_id(cursor)?;
        self.bindings.sites.insert(site, Binding::Local(id));
        Ok(())
    }

    fn required_site(&mut self, cursor: &SourceNodeCursor, expected: &[SymbolKind]) -> Result<DeclId> {
        let id = self.node_id(cursor)?;
        let path = self
            .source
            .name_paths
            .get(&id)
            .ok_or_else(|| ArgentError::at(self.program.declaration_path(self.owner), "missing indexed source path"))?;
        let target = self.required(&path.segments.join("::"), expected)?;
        self.bindings.sites.insert(id, Binding::Source(ResolvedName::Declaration(target)));
        Ok(target)
    }

    fn type_use(&mut self, cursor: &SourceNodeCursor) -> Result<()> {
        let id = self.node_id(cursor)?;
        let Some(ty) = self.source.type_uses.get(&id) else { return Ok(()) };
        let name = if let Some(state) = &ty.actor_state {
            Some((state.segments.join("::"), vec![SymbolKind::State]))
        } else if let sil::TypeBase::Custom(name) = &ty.ty.base {
            Some((name.clone(), vec![SymbolKind::State, SymbolKind::ActorEnum]))
        } else {
            None
        };
        if let Some((name, expected)) = name {
            if name == "State" || TypeRef::new(&name).is_builtin() {
                self.bindings.sites.insert(id, Binding::Builtin);
                return Ok(());
            }
            let target = self.required(&name, &expected)?;
            self.bindings.sites.insert(id, Binding::Source(ResolvedName::Declaration(target)));
        } else {
            self.bindings.sites.insert(id, Binding::Builtin);
        }
        Ok(())
    }

    fn reference(&mut self, cursor: &SourceNodeCursor, name: &str, scope: &BTreeMap<String, Binding>) -> Result<()> {
        let id = self.node_id(cursor)?;
        let Origin::Authored { .. } = self.program.nodes().node(id).origin else {
            return Err(ArgentError::at(self.program.declaration_path(self.owner), "generated source reference"));
        };
        if !name.contains("::")
            && let Some(binding) = scope.get(name)
        {
            self.bindings.sites.insert(id, binding.clone());
            return Ok(());
        }

        let resolved = self.program.resolve(self.owner.module, name);
        if let Ok(target) = resolved {
            if matches!(target, ResolvedName::Module(_)) {
                return Err(ArgentError::at(
                    self.program.declaration_path(self.owner),
                    format!("module namespace `{name}` cannot be used as a value"),
                ));
            }
            self.record(target);
            self.bindings.sites.insert(id, Binding::Source(target));
            return Ok(());
        }

        if let Some((prefix, variant)) = name.rsplit_once("::")
            && let Ok(ResolvedName::Declaration(enumeration)) = self.program.resolve(self.owner.module, prefix)
            && enumeration.kind == SymbolKind::ActorEnum
            && let ResolvedDeclaration::ActorEnum(actor_enum) = self.program.declaration(enumeration)
        {
            let actor = actor_enum
                .variants
                .iter()
                .find_map(|reference| {
                    let ResolvedName::Declaration(actor) = self.program.resolve(enumeration.module, reference).ok()? else {
                        return None;
                    };
                    (self.program.declaration(actor).name() == variant).then_some(actor)
                })
                .ok_or_else(|| {
                    ArgentError::at(
                        self.program.declaration_path(self.owner),
                        format!("actor enum `{prefix}` has no variant `{variant}`"),
                    )
                })?;
            self.record(ResolvedName::Declaration(enumeration));
            self.record(ResolvedName::Declaration(actor));
            self.bindings.sites.insert(id, Binding::EnumVariant { enumeration, actor });
            return Ok(());
        }

        let builtin = matches!(
            name,
            "int"
                | "signed"
                | "unsigned"
                | "temporal"
                | "bool"
                | "byte"
                | "string"
                | "pubkey"
                | "sig"
                | "datasig"
                | "length"
                | "sha256"
                | "blake2b"
                | "blake2bWithKey"
                | "blake3"
                | "blake3WithKey"
                | "templateHash"
                | "checkSig"
                | "checkSigEcdsa"
                | "checkMsgSig"
                | "checkMsgSigEcdsa"
                | "co_spent"
                | "state"
                | "digest"
                | "cov_id"
                | "unrestricted"
                | "State"
                | "invocation_uid"
                | "readInputState"
                | "ScriptPubKeyP2PK"
                | "ScriptPubKeyP2SH"
                | "ScriptPubKeyP2SHFromRedeemScript"
                | "g16.verify"
                | "r0.g16.verify"
                | "r0.succinct.verify"
                | "r0.succinct.blake2b.verify"
                | "r0.succinct.poseidon2.verify"
                | "r0.succinct.sha256.verify"
                // The bundled std::core source uses these two SIL primitives.
                | "OpOutpointIndex"
                | "OpOutpointTxId"
        ) || name.starts_with("__as_cast_")
            || (name.starts_with("byte[") && name.ends_with(']'));
        if builtin {
            self.bindings.sites.insert(id, Binding::Builtin);
            return Ok(());
        }
        if matches!(name, "self" | "this" | "tx" | "r0") {
            self.bindings.sites.insert(id, Binding::RuntimeRoot);
            return Ok(());
        }
        Err(if name.contains("::") {
            ArgentError::at(self.program.declaration_path(self.owner), format!("unresolved qualified reference `{name}`"))
        } else {
            ArgentError::at(self.program.declaration_path(self.owner), format!("unknown reference `{name}`"))
        })
    }

    fn expr(&mut self, cursor: &SourceNodeCursor, expr: &sil::Expr<'_>, scope: &BTreeMap<String, Binding>) -> Result<()> {
        match &expr.kind {
            ExprKind::Identifier(name) => self.reference(cursor, name, scope)?,
            ExprKind::Array { values, .. } => {
                self.type_use(&cursor.child(ChildEdge::TypeUse))?;
                for (index, value) in values.iter().enumerate() {
                    self.expr(&cursor.child(ChildEdge::Element(index)), value, scope)?;
                }
            }
            ExprKind::Call { name, args, name_span } => {
                if name == crate::compiler::syntax::word::CO_SPENT && name_span.start() > expr.span.start() {
                    let target = self.node_id(&cursor.child(ChildEdge::CallTarget))?;
                    self.bindings.sites.insert(target, Binding::Builtin);
                } else {
                    self.reference(&cursor.child(ChildEdge::CallTarget), name, scope)?;
                }
                for (index, arg) in args.iter().enumerate() {
                    self.expr(&cursor.child(ChildEdge::Argument(index)), arg, scope)?;
                }
            }
            ExprKind::New { name, args, .. } => {
                self.reference(&cursor.child(ChildEdge::TypeUse), name, scope)?;
                for (index, arg) in args.iter().enumerate() {
                    self.expr(&cursor.child(ChildEdge::Argument(index)), arg, scope)?;
                }
            }
            ExprKind::Split { source, index, .. } | ExprKind::ArrayIndex { source, index } => {
                self.expr(&cursor.child(ChildEdge::ExpressionSource), source, scope)?;
                self.expr(&cursor.child(ChildEdge::ExpressionIndex), index, scope)?;
            }
            ExprKind::Slice { source, start, end, .. } => {
                self.expr(&cursor.child(ChildEdge::ExpressionSource), source, scope)?;
                self.expr(&cursor.child(ChildEdge::ExpressionStart), start, scope)?;
                self.expr(&cursor.child(ChildEdge::ExpressionEnd), end, scope)?;
            }
            ExprKind::Append { source, args, .. } => {
                self.expr(&cursor.child(ChildEdge::ExpressionSource), source, scope)?;
                for (index, arg) in args.iter().enumerate() {
                    self.expr(&cursor.child(ChildEdge::Argument(index)), arg, scope)?;
                }
            }
            ExprKind::Unary { expr, .. } => self.expr(&cursor.child(ChildEdge::Expression), expr, scope)?,
            ExprKind::Binary { left, right, .. } => {
                self.expr(&cursor.child(ChildEdge::ExpressionLeft), left, scope)?;
                self.expr(&cursor.child(ChildEdge::ExpressionRight), right, scope)?;
            }
            ExprKind::Ternary { condition, then_expr, else_expr } => {
                self.expr(&cursor.child(ChildEdge::ExpressionCondition), condition, scope)?;
                self.expr(&cursor.child(ChildEdge::ExpressionThen), then_expr, scope)?;
                self.expr(&cursor.child(ChildEdge::ExpressionElse), else_expr, scope)?;
            }
            ExprKind::IndexedIntrospection { index, .. } => {
                self.expr(&cursor.child(ChildEdge::ExpressionIndex), index, scope)?;
            }
            ExprKind::StructLiteral { name, fields, .. } => {
                if !name.is_empty() {
                    self.reference(&cursor.child(ChildEdge::TypeUse), name, scope)?;
                }
                for (index, field) in fields.iter().enumerate() {
                    self.expr(&cursor.child(ChildEdge::Field(index)).child(ChildEdge::Expression), &field.expr, scope)?;
                }
            }
            ExprKind::FieldAccess { source, .. } | ExprKind::UnarySuffix { source, .. } => {
                self.expr(&cursor.child(ChildEdge::ExpressionSource), source, scope)?;
            }
            ExprKind::Int(_)
            | ExprKind::Temporal(_)
            | ExprKind::Bool(_)
            | ExprKind::Byte(_)
            | ExprKind::String(_)
            | ExprKind::DateLiteral(_)
            | ExprKind::Introspection(_)
            | ExprKind::NumberWithUnit { .. } => {}
        }
        Ok(())
    }
}

/// A simple clause reference with its authored field or parameter position.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum ClauseReference {
    StateField { name: String, index: Option<usize> },
    EntryArgument { name: String, index: usize },
    BareStateField { name: String, index: usize },
    Bare(String),
}

impl DeclarationBinder<'_, '_> {
    /// Bind a retained clause expression to the entry and storage declarations.
    fn clause_reference(&self, expr: &sil::Expr<'_>, entry_index: usize) -> Result<Option<ClauseReference>> {
        let ResolvedDeclaration::Actor(actor) = self.program.declaration(self.owner) else {
            return Err(ArgentError::new("clause reference owner is not an actor"));
        };
        let entry = actor.entries.get(entry_index).ok_or_else(|| ArgentError::new("clause reference has no entry"))?;
        let Some(ResolvedName::Declaration(mut state_id)) = self.bindings.names.get(&actor.state).copied() else {
            return Err(ArgentError::new("clause reference has no bound actor state"));
        };
        let mut visited = BTreeSet::new();
        let state = loop {
            if !visited.insert(state_id) {
                return Err(ArgentError::new("clause reference state expansion has a cycle"));
            }
            let ResolvedDeclaration::State(state) = self.program.declaration(state_id) else {
                return Err(ArgentError::new("clause reference state binding is not a state"));
            };
            let Some(expansion) = &state.expansion else { break state };
            let ResolvedName::Declaration(base) = self.program.resolve(state_id.module, &expansion.base)? else {
                return Err(ArgentError::new("expanded clause state has no bound storage state"));
            };
            state_id = base;
        };
        match &expr.kind {
            ExprKind::Identifier(name) => {
                if let Some(index) = entry.params.iter().position(|param| param.name == *name) {
                    Ok(Some(ClauseReference::EntryArgument { name: name.clone(), index }))
                } else if let Some(index) = state.fields.iter().position(|field| field.name == *name) {
                    Ok(Some(ClauseReference::BareStateField { name: name.clone(), index }))
                } else {
                    Ok(Some(ClauseReference::Bare(name.clone())))
                }
            }
            ExprKind::FieldAccess { source, field, .. } if matches!(&source.kind, ExprKind::Identifier(root) if root == "self") => {
                Ok(Some(ClauseReference::StateField {
                    name: field.clone(),
                    index: state.fields.iter().position(|candidate| candidate.name == *field),
                }))
            }
            _ => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LocalId {
    pub owner: DeclId,
    pub callable: RootSlot,
    pub index: usize,
}

/// The declaration supplying a dynamic actor target within one entry.
#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ActorSourceBinding {
    Local(LocalId),
    StateField(usize),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum Binding {
    Source(ResolvedName),
    EnumVariant { enumeration: DeclId, actor: DeclId },
    Local(LocalId),
    ActorField,
    ActorHelper(usize),
    ClauseMember,
    Builtin,
    RuntimeRoot,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DeclarationBindings {
    pub names: BTreeMap<String, ResolvedName>,
    pub sites: BTreeMap<NodeId, Binding>,
    pub local_names: BTreeMap<LocalId, String>,
    pub parameter_ids: BTreeMap<(RootSlot, usize), LocalId>,
    pub entry_consumes: BTreeMap<(usize, usize), LocalId>,
    pub entry_emits: BTreeMap<(usize, usize), LocalId>,
    pub entry_consume_targets: BTreeMap<(usize, usize), DeclId>,
    pub entry_emit_targets: BTreeMap<(usize, usize, usize), DeclId>,
    pub entry_spawns: BTreeMap<(usize, usize), LocalId>,
    pub entry_observes: BTreeMap<(usize, usize), LocalId>,
    pub entry_spawn_covenants: BTreeMap<(usize, usize), LocalId>,
    pub actor_targets: BTreeMap<NodeId, ResolvedName>,
    pub local_actor_targets: BTreeMap<NodeId, ActorSourceBinding>,
    pub open_state_targets: BTreeMap<NodeId, DeclId>,
    pub clause_actor_references: BTreeMap<NodeId, Option<ClauseReference>>,
    pub route_actor_references: BTreeMap<NodeId, Option<ClauseReference>>,
    pub observe_covenant_references: BTreeMap<(usize, usize), Option<ClauseReference>>,
    pub declarations: BTreeSet<DeclId>,
    pub apps: BTreeSet<AppMember>,
}

struct DeclarationBinder<'a, 'src> {
    program: &'a ResolvedModules<'src>,
    source: &'a SourceModule<'a>,
    owner: DeclId,
    bindings: DeclarationBindings,
    actor_fields: BTreeSet<String>,
    actor_helpers: BTreeMap<String, usize>,
    /// Track shadowing so earlier target validation preserves entry diagnostics.
    entry_parameters: BTreeMap<String, bool>,
    next_local: usize,
}

impl DeclarationBindings {
    pub(super) fn resolve<'a>(program: &'a ResolvedModules<'_>, source: &'a SourceModule<'a>, owner: DeclId) -> Result<Self> {
        let mut binder = DeclarationBinder {
            program,
            source,
            owner,
            bindings: Self::default(),
            actor_fields: BTreeSet::new(),
            actor_helpers: BTreeMap::new(),
            entry_parameters: BTreeMap::new(),
            next_local: 0,
        };
        match program.declaration(owner) {
            ResolvedDeclaration::Const(item) => {
                binder.ty(&item.ty)?;
                let cursor = SourceNodeCursor::new(owner, RootSlot::ConstValue);
                binder.expr(&cursor, &source.const_values[owner.index], &BTreeMap::new())?;
            }
            ResolvedDeclaration::State(item) => {
                for field in &item.fields {
                    binder.ty(&field.ty)?;
                }
                if let Some(expansion) = &item.expansion {
                    binder.required_site(&SourceNodeCursor::new(owner, RootSlot::StateBase), &[SymbolKind::State])?;
                    for (index, _) in expansion.digests.iter().enumerate() {
                        binder.required_site(&SourceNodeCursor::new(owner, RootSlot::DigestState(index)), &[SymbolKind::State])?;
                    }
                }
            }
            ResolvedDeclaration::Function(item) => binder.function(0, item, &BTreeSet::new())?,
            ResolvedDeclaration::Actor(item) => {
                let state = binder.required_site(&SourceNodeCursor::new(owner, RootSlot::ActorState), &[SymbolKind::State])?;
                binder.actor_helpers.extend(item.functions.iter().enumerate().map(|(index, function)| (function.name.clone(), index)));
                let mut scope = item.functions.iter().map(|function| function.name.clone()).collect::<BTreeSet<_>>();
                let mut pending = Some(state);
                let mut visited = BTreeSet::new();
                while let Some(id) = pending.take() {
                    if !visited.insert(id) {
                        break;
                    }
                    if let ResolvedDeclaration::State(state) = program.declaration(id) {
                        scope.extend(state.fields.iter().map(|field| field.name.clone()));
                        binder
                            .actor_fields
                            .extend(state.fields.iter().filter(|field| field.ty.is_actor_type()).map(|field| field.name.clone()));
                        if let Some(expansion) = &state.expansion
                            && let ResolvedName::Declaration(base) = program.resolve(id.module, &expansion.base)?
                        {
                            pending = Some(base);
                        }
                    }
                }
                for (index, function) in item.functions.iter().enumerate() {
                    binder.function(index, function, &scope)?;
                }
                for (index, entry) in item.entries.iter().enumerate() {
                    binder.entry(index, entry, &scope)?;
                }
            }
            ResolvedDeclaration::ActorEnum(item) => {
                for actor in &item.variants {
                    binder.required(actor, &[SymbolKind::Actor])?;
                }
            }
            ResolvedDeclaration::App(item) => {
                for (index, _) in item.actors.iter().enumerate() {
                    binder.required_site(&SourceNodeCursor::new(owner, RootSlot::AppActor(index)), &[SymbolKind::Actor])?;
                }
            }
        }
        Ok(binder.bindings)
    }
}

impl DeclarationBinder<'_, '_> {
    fn record(&mut self, target: ResolvedName) {
        match target {
            ResolvedName::Declaration(id) => {
                self.bindings.declarations.insert(id);
            }
            ResolvedName::AppMember(member) => {
                self.bindings.apps.insert(member);
            }
            ResolvedName::Module(_) => {}
        }
    }

    fn required(&mut self, name: &str, expected: &[SymbolKind]) -> Result<DeclId> {
        match self.program.resolve(self.owner.module, name)? {
            ResolvedName::Declaration(id) if expected.contains(&id.kind) => {
                self.bindings.names.insert(name.to_string(), ResolvedName::Declaration(id));
                self.record(ResolvedName::Declaration(id));
                Ok(id)
            }
            ResolvedName::Declaration(id) => Err(self.program.wrong_kind(self.owner.module, name, expected, id.kind)),
            ResolvedName::AppMember(_) => Err(ArgentError::at(
                self.program.declaration_path(self.owner),
                format!("app member `{name}` cannot be used as a local declaration"),
            )),
            ResolvedName::Module(_) => Err(ArgentError::at(
                self.program.declaration_path(self.owner),
                format!("module namespace `{name}` does not name a declaration"),
            )),
        }
    }

    fn ty(&mut self, ty: &TypeRef) -> Result<()> {
        if let Some(state) = &ty.actor_state {
            self.required(state, &[SymbolKind::State])?;
        // built-in types do not need resolution
        } else if !ty.is_builtin() {
            self.required(&ty.name, &[SymbolKind::State, SymbolKind::ActorEnum])?;
        }
        Ok(())
    }

    fn cardinality(&mut self, cardinality: &Cardinality) -> Result<()> {
        if let Cardinality::Range { minimum, maximum } = cardinality {
            for bound in [minimum, maximum] {
                if let CardinalityBound::Const(name) = bound {
                    self.required(name, &[SymbolKind::Const])?;
                }
            }
        }
        Ok(())
    }

    fn actor_target(
        &mut self,
        cursor: &SourceNodeCursor,
        entry: usize,
        name: &str,
        locals: &BTreeSet<String>,
        scope: &BTreeMap<String, Binding>,
    ) -> Result<()> {
        let id = self.node_id(cursor)?;
        let authored = if let Some(expr) = self.source.actor_targets.get(&id) {
            self.bindings.clause_actor_references.insert(id, self.clause_reference(expr, entry)?);
            match &expr.kind {
                ExprKind::Identifier(name) => name.clone(),
                ExprKind::FieldAccess { source, field, .. } if matches!(&source.kind, ExprKind::Identifier(root) if root == "self") => {
                    format!("self.{field}")
                }
                _ => name.to_string(),
            }
        } else {
            self.source
                .name_paths
                .get(&id)
                .ok_or_else(|| ArgentError::at(self.program.declaration_path(self.owner), "missing indexed actor target"))?
                .segments
                .join("::")
        };
        debug_assert_eq!(authored, name);
        // Open observed actors and actor_type parameters are local bindings.
        if locals.contains(name) {
            let source = match self.bindings.clause_actor_references.get(&id).and_then(Option::as_ref) {
                Some(ClauseReference::StateField { index: Some(index), .. }) => ActorSourceBinding::StateField(*index),
                Some(ClauseReference::BareStateField { index, .. }) if matches!(scope.get(name), Some(Binding::ActorField)) => {
                    ActorSourceBinding::StateField(*index)
                }
                _ => match scope.get(name) {
                    Some(Binding::Local(id)) => ActorSourceBinding::Local(*id),
                    _ => return Err(ArgentError::new(format!("entry actor target `{name}` has no bound source"))),
                },
            };
            self.bindings.local_actor_targets.insert(id, source);
            self.bindings.sites.insert(id, Binding::ClauseMember);
            return Ok(());
        }
        match self.program.resolve(self.owner.module, name) {
            Ok(target @ ResolvedName::AppMember(_)) => {
                self.bindings.actor_targets.insert(id, target);
                self.bindings.sites.insert(id, Binding::Source(target));
                self.record(target);
            }
            Ok(ResolvedName::Declaration(actor_id)) if matches!(actor_id.kind, SymbolKind::Actor | SymbolKind::ActorEnum) => {
                let target = ResolvedName::Declaration(actor_id);
                self.bindings.actor_targets.insert(id, target);
                self.bindings.sites.insert(id, Binding::Source(target));
                self.record(target);
            }
            Ok(ResolvedName::Declaration(id)) => {
                return Err(self.program.wrong_kind(self.owner.module, name, &[SymbolKind::Actor, SymbolKind::ActorEnum], id.kind));
            }
            Ok(ResolvedName::Module(_)) => {
                return Err(ArgentError::at(
                    self.program.declaration_path(self.owner),
                    format!("module namespace `{name}` cannot be used as an actor"),
                ));
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }

    fn function(&mut self, index: usize, function: &FunctionDecl, outer: &BTreeSet<String>) -> Result<()> {
        let root = if self.owner.kind() == SymbolKind::Actor { RootSlot::ActorFunction(index) } else { RootSlot::Declaration };
        let mut scope = outer
            .iter()
            .map(|name| {
                let binding = self.actor_helpers.get(name).map_or(Binding::ActorField, |index| Binding::ActorHelper(*index));
                (name.clone(), binding)
            })
            .collect::<BTreeMap<_, _>>();
        for (index, param) in function.params.iter().enumerate() {
            self.ty(&param.ty)?;
            let id = self.local(&mut scope, &param.name, root);
            self.bindings.parameter_ids.insert((root, index), id);
        }
        if let Some(ty) = &function.return_ty {
            self.ty(ty)?;
        }
        let cursor = SourceNodeCursor::new(self.owner, root).child(ChildEdge::Body);
        let source = self.source;
        let body = source
            .function_bodies
            .get(&self.node_id(&cursor)?)
            .ok_or_else(|| ArgentError::at(self.program.declaration_path(self.owner), "missing indexed function body"))?;
        self.sil_statements(&cursor, body, &mut scope)?;
        Ok(())
    }

    fn entry(&mut self, index: usize, entry: &EntryDecl, outer: &BTreeSet<String>) -> Result<()> {
        self.entry_parameters = entry.params.iter().map(|param| (param.name.clone(), false)).collect();
        let root = RootSlot::Entry(index);
        let mut scope = outer
            .iter()
            .map(|name| {
                let binding = self.actor_helpers.get(name).map_or(Binding::ActorField, |index| Binding::ActorHelper(*index));
                (name.clone(), binding)
            })
            .collect::<BTreeMap<_, _>>();
        for (index, param) in entry.params.iter().enumerate() {
            self.ty(&param.ty)?;
            let id = self.local(&mut scope, &param.name, root);
            self.bindings.parameter_ids.insert((root, index), id);
        }
        for (consume_index, consume) in entry.consumes.iter().enumerate() {
            let actor = self.required(&consume.actor, &[SymbolKind::Actor])?;
            self.bindings.entry_consume_targets.insert((index, consume_index), actor);
            self.cardinality(&consume.cardinality)?;
            let id = self.local(&mut scope, &consume.name, root);
            self.bindings.entry_consumes.insert((index, consume_index), id);
        }
        for (observe_index, observe) in entry.observes.iter().enumerate() {
            let id = self.local(&mut scope, &observe.name, root);
            self.bindings.entry_observes.insert((index, observe_index), id);
            for actor in &observe.inputs {
                if actor.open_state.is_some() {
                    self.local(&mut scope, &actor.actor, root);
                }
            }
        }
        for (spawn_index, spawn) in entry.spawns.iter().enumerate() {
            let group = self.local(&mut scope, &spawn.name, root);
            self.bindings.entry_spawns.insert((index, spawn_index), group);
            let covenant = self.local(&mut scope, &spawn.covenant, root);
            self.bindings.entry_spawn_covenants.insert((index, spawn_index), covenant);
        }
        if let EmitSpec::Outputs(outputs) = &entry.emits {
            for (output_index, output) in outputs.iter().enumerate() {
                for (actor_index, actor) in output.actors.iter().enumerate() {
                    let target = self.required(actor, &[SymbolKind::Actor, SymbolKind::ActorEnum])?;
                    self.bindings.entry_emit_targets.insert((index, output_index, actor_index), target);
                }
                self.cardinality(&output.cardinality)?;
                let id = self.local(&mut scope, &output.name, root);
                self.bindings.entry_emits.insert((index, output_index), id);
            }
        }
        let mut actor_locals = entry.params.iter().filter(|param| {
            param.ty.is_actor_type() || (param.ty.array.is_none() && matches!(self.program.resolve(self.owner.module, &param.ty.name), Ok(ResolvedName::Declaration(id)) if id.kind == SymbolKind::ActorEnum))
        }).map(|param| param.name.clone()).collect::<BTreeSet<_>>();
        actor_locals.extend(self.actor_fields.iter().flat_map(|field| [field.clone(), format!("self.{field}")]));
        for (observe_index, observe) in entry.observes.iter().enumerate() {
            let cursor = SourceNodeCursor::new(self.owner, root).child(ChildEdge::ObserveCovenant(observe_index));
            let source = self.source;
            let expr = source
                .observe_exprs
                .get(&self.node_id(&cursor)?)
                .ok_or_else(|| ArgentError::at(self.program.declaration_path(self.owner), "missing indexed observe expression"))?;
            self.bindings.observe_covenant_references.insert((index, observe_index), self.clause_reference(expr, index)?);
            self.expr(&cursor, expr, &scope)?;
            let mut actor_scope = actor_locals.clone();
            actor_scope.extend(observe.inputs.iter().filter(|actor| actor.open_state.is_some()).map(|actor| actor.actor.clone()));
            let owner = self.owner;
            let inputs = observe.inputs.iter().enumerate().map(|(input, actor)| {
                (
                    SourceNodeCursor::new(owner, root)
                        .child(ChildEdge::Observe(observe_index))
                        .child(ChildEdge::ObservedInput(input))
                        .child(ChildEdge::ActorTarget),
                    actor,
                )
            });
            let outputs = observe.outputs.iter().enumerate().map(|(output, actor)| {
                (
                    SourceNodeCursor::new(owner, root)
                        .child(ChildEdge::Observe(observe_index))
                        .child(ChildEdge::ObservedOutput(output))
                        .child(ChildEdge::ActorTarget),
                    actor,
                )
            });
            for (cursor, actor) in inputs.chain(outputs) {
                let site = self.node_id(&cursor)?;
                if let Some(state) = &actor.open_state {
                    let target = self.required(state, &[SymbolKind::State])?;
                    self.bindings.open_state_targets.insert(site, target);
                }
                self.actor_target(&cursor, index, &actor.actor, &actor_scope, &scope)?;
                self.cardinality(&actor.cardinality)?;
            }
        }
        let actor_scope = actor_locals.clone();
        for (spawn_index, spawn) in entry.spawns.iter().enumerate() {
            for (output_index, output) in spawn.outputs.iter().enumerate() {
                self.actor_target(
                    &SourceNodeCursor::new(self.owner, root)
                        .child(ChildEdge::Spawn(spawn_index))
                        .child(ChildEdge::SpawnOutput(output_index))
                        .child(ChildEdge::ActorTarget),
                    index,
                    &output.actor,
                    &actor_scope,
                    &scope,
                )?;
                self.cardinality(&output.cardinality)?;
            }
        }
        actor_locals.extend(
            entry
                .observes
                .iter()
                .flat_map(|observe| observe.inputs.iter())
                .filter(|actor| actor.open_state.is_some())
                .map(|actor| actor.actor.clone()),
        );
        let cursor = SourceNodeCursor::new(self.owner, root).child(ChildEdge::Body);
        let source = self.source;
        let body = source
            .entry_bodies
            .get(&self.node_id(&cursor)?)
            .ok_or_else(|| ArgentError::at(self.program.declaration_path(self.owner), "missing indexed entry body"))?;
        self.entry_statements(&cursor, body, &mut scope, &mut actor_locals)?;
        Ok(())
    }

    fn sil_statements(
        &mut self,
        cursor: &SourceNodeCursor,
        statements: &[Statement<'_>],
        scope: &mut BTreeMap<String, Binding>,
    ) -> Result<()> {
        for (index, statement) in statements.iter().enumerate() {
            self.sil_statement(&cursor.child(ChildEdge::Statement(index)), statement, scope)?;
        }
        Ok(())
    }

    fn sil_statement(
        &mut self,
        cursor: &SourceNodeCursor,
        statement: &Statement<'_>,
        scope: &mut BTreeMap<String, Binding>,
    ) -> Result<()> {
        match statement {
            Statement::VariableDefinition { name, expr, .. } => {
                self.type_use(&cursor.child(ChildEdge::TypeUse))?;
                if let Some(expr) = expr {
                    self.expr(&cursor.child(ChildEdge::Expression), expr, scope)?;
                }
                self.local_at(scope, name, &cursor.child(ChildEdge::BindingName))?;
            }
            Statement::TupleAssignment { left_name, right_name, expr, .. } => {
                self.type_use(&cursor.child(ChildEdge::ExpressionLeft).child(ChildEdge::TypeUse))?;
                self.type_use(&cursor.child(ChildEdge::ExpressionRight).child(ChildEdge::TypeUse))?;
                self.expr(&cursor.child(ChildEdge::Expression), expr, scope)?;
                self.local_at(scope, left_name, &cursor.child(ChildEdge::ExpressionLeft).child(ChildEdge::BindingName))?;
                self.local_at(scope, right_name, &cursor.child(ChildEdge::ExpressionRight).child(ChildEdge::BindingName))?;
            }
            Statement::FunctionCall { name, args, .. }
            | Statement::FunctionCallAssign { name, args, .. }
            | Statement::StateFunctionCallAssign { name, args, .. } => {
                if let Statement::StateFunctionCallAssign { .. } = statement {
                    self.type_use(&cursor.child(ChildEdge::TypeUse))?;
                }
                self.reference(&cursor.child(ChildEdge::CallTarget), name, scope)?;
                for (index, arg) in args.iter().enumerate() {
                    self.expr(&cursor.child(ChildEdge::Argument(index)), arg, scope)?;
                }
                match statement {
                    Statement::FunctionCallAssign { bindings, .. } => {
                        for (index, binding) in bindings.iter().enumerate() {
                            self.type_use(&cursor.child(ChildEdge::Element(index)).child(ChildEdge::TypeUse))?;
                            self.local_at(
                                scope,
                                &binding.name,
                                &cursor.child(ChildEdge::Element(index)).child(ChildEdge::BindingName),
                            )?;
                        }
                    }
                    Statement::StateFunctionCallAssign { bindings, .. } => {
                        for (index, binding) in bindings.iter().enumerate() {
                            self.type_use(&cursor.child(ChildEdge::Field(index)).child(ChildEdge::TypeUse))?;
                            self.local_at(scope, &binding.name, &cursor.child(ChildEdge::Field(index)).child(ChildEdge::BindingName))?;
                        }
                    }
                    _ => {}
                }
            }
            Statement::StructDestructure { bindings, expr, .. } => {
                if self.program.nodes().find(&cursor.child(ChildEdge::TypeUse).address).is_some() {
                    self.type_use(&cursor.child(ChildEdge::TypeUse))?;
                }
                self.expr(&cursor.child(ChildEdge::Expression), expr, scope)?;
                for (index, binding) in bindings.iter().enumerate() {
                    self.type_use(&cursor.child(ChildEdge::Field(index)).child(ChildEdge::TypeUse))?;
                    self.local_at(scope, &binding.name, &cursor.child(ChildEdge::Field(index)).child(ChildEdge::BindingName))?;
                }
            }
            Statement::Assign { name, expr, .. } => {
                let root = name.split('.').next().unwrap_or(name);
                self.reference(&cursor.child(ChildEdge::AssignmentTarget), root, scope)?;
                self.expr(&cursor.child(ChildEdge::Expression), expr, scope)?;
            }
            Statement::RequireAgeDaa { expr, .. }
            | Statement::RequireTxDaa { expr, .. }
            | Statement::RequireTxTime { expr, .. }
            | Statement::Require { expr, .. } => {
                self.expr(&cursor.child(ChildEdge::Expression), expr, scope)?;
            }
            Statement::Block { body, .. } => {
                self.sil_statements(cursor, body, &mut scope.clone())?;
            }
            Statement::If { condition, then_branch, else_branch, .. } => {
                self.expr(&cursor.child(ChildEdge::ExpressionCondition), condition, scope)?;
                self.sil_statements(&cursor.child(ChildEdge::ThenBranch), then_branch, &mut scope.clone())?;
                if let Some(else_branch) = else_branch {
                    self.sil_statements(&cursor.child(ChildEdge::ElseBranch), else_branch, &mut scope.clone())?;
                }
            }
            Statement::For { ident, start, end, max_iterations, body, .. } => {
                self.expr(&cursor.child(ChildEdge::ExpressionStart), start, scope)?;
                self.expr(&cursor.child(ChildEdge::ExpressionEnd), end, scope)?;
                self.expr(&cursor.child(ChildEdge::ExpressionLimit), max_iterations, scope)?;
                let mut loop_scope = scope.clone();
                self.local_at(&mut loop_scope, ident, &cursor.child(ChildEdge::LoopBinding))?;
                self.sil_statements(cursor, body, &mut loop_scope)?;
            }
            Statement::Return { exprs, .. } | Statement::Console { args: exprs, .. } => {
                for (index, expr) in exprs.iter().enumerate() {
                    self.expr(&cursor.child(ChildEdge::Argument(index)), expr, scope)?;
                }
            }
        }
        Ok(())
    }

    fn entry_statements(
        &mut self,
        cursor: &SourceNodeCursor,
        statements: &[AuthoredEntryStatement<'_>],
        scope: &mut BTreeMap<String, Binding>,
        actor_locals: &mut BTreeSet<String>,
    ) -> Result<()> {
        for (index, statement) in statements.iter().enumerate() {
            self.entry_statement(&cursor.child(ChildEdge::Statement(index)), statement, scope, actor_locals)?;
        }
        Ok(())
    }

    fn entry_statement(
        &mut self,
        cursor: &SourceNodeCursor,
        statement: &AuthoredEntryStatement<'_>,
        scope: &mut BTreeMap<String, Binding>,
        actor_locals: &mut BTreeSet<String>,
    ) -> Result<()> {
        match statement {
            AuthoredEntryStatement::Block { statements, .. } => {
                self.entry_statements(cursor, statements, &mut scope.clone(), &mut actor_locals.clone())?;
            }
            AuthoredEntryStatement::If { condition, then_branch, else_branch, .. } => {
                self.expr(&cursor.child(ChildEdge::ExpressionCondition), condition, scope)?;
                self.entry_statement(
                    &cursor.child(ChildEdge::ThenBranch),
                    then_branch,
                    &mut scope.clone(),
                    &mut actor_locals.clone(),
                )?;
                if let Some(else_branch) = else_branch {
                    self.entry_statement(
                        &cursor.child(ChildEdge::ElseBranch),
                        else_branch,
                        &mut scope.clone(),
                        &mut actor_locals.clone(),
                    )?;
                }
            }
            AuthoredEntryStatement::Become { routes, .. } | AuthoredEntryStatement::ForeignBecome { routes, .. } => {
                if let AuthoredEntryStatement::ForeignBecome { group, .. } = statement {
                    self.reference(&cursor.child(ChildEdge::ForeignGroup), &group.segments.join("::"), scope)?;
                }
                for (index, route) in routes.iter().enumerate() {
                    let route_cursor = cursor.child(ChildEdge::Route(index));
                    if matches!(statement, AuthoredEntryStatement::ForeignBecome { .. }) {
                        let id = self.node_id(&route_cursor.child(ChildEdge::RouteOutput))?;
                        self.bindings.sites.insert(id, Binding::ClauseMember);
                    } else {
                        self.reference(&route_cursor.child(ChildEdge::RouteOutput), &route.output.segments.join("::"), scope)?;
                    }
                    if let AuthoredSuccessor::Constructed { actor, state, .. } = &route.successor {
                        let actor_name = match &actor.kind {
                            ExprKind::Identifier(name) => name.as_str(),
                            ExprKind::FieldAccess { source, field, .. } if matches!(&source.kind, ExprKind::Identifier(name) if name == "self") => {
                                field.as_str()
                            }
                            _ => "",
                        };
                        if !actor_name.is_empty() && !actor_locals.contains(actor_name) && scope.contains_key(actor_name) {
                            if self.entry_parameters.get(actor_name) == Some(&true) {
                                return Err(ArgentError::at(
                                    self.program.declaration_path(self.owner),
                                    format!("entry binding `{actor_name}` collides with entry parameter of the same name"),
                                ));
                            }
                            return Err(ArgentError::at(
                                self.program.declaration_path(self.owner),
                                format!("local `{actor_name}` is not an actor selector"),
                            ));
                        }
                        if !actor_name.is_empty()
                            && !actor_locals.contains(actor_name)
                            && !scope.contains_key(actor_name)
                            && !actor_name.contains("::")
                            && self.program.resolve(self.owner.module, actor_name).is_err()
                        {
                            return Err(ArgentError::at(
                                self.program.declaration_path(self.owner),
                                format!("actor handle `{actor_name}` is not visible in this scope"),
                            ));
                        }
                        let actor_site = self.node_id(&route_cursor.child(ChildEdge::RouteActor))?;
                        let RootSlot::Entry(entry_index) = cursor.address.root else {
                            return Err(ArgentError::new("route actor has no entry scope"));
                        };
                        let actor_reference = self.clause_reference(actor, entry_index)?;
                        self.bindings.route_actor_references.insert(actor_site, actor_reference);
                        self.expr(&route_cursor.child(ChildEdge::RouteActor), actor, scope)?;
                        self.expr(&route_cursor.child(ChildEdge::RouteState), state, scope)?;
                    }
                }
            }
            AuthoredEntryStatement::Sil(statement) => {
                self.sil_statement(cursor, statement, scope)?;
                let declared = match statement.as_ref() {
                    Statement::VariableDefinition { name, type_ref, .. } => {
                        let type_cursor = cursor.child(ChildEdge::TypeUse);
                        let actor_type = self
                            .program
                            .nodes()
                            .find(&type_cursor.address)
                            .and_then(|id| self.source.type_uses.get(&id))
                            .is_some_and(|ty| ty.actor_state.is_some());
                        let actor_enum = matches!(&type_ref.base, sil::TypeBase::Custom(name)
                            if matches!(self.program.resolve(self.owner.module, name), Ok(ResolvedName::Declaration(id)) if id.kind == SymbolKind::ActorEnum));
                        actor_locals.remove(name);
                        if actor_type || actor_enum {
                            actor_locals.insert(name.clone());
                        }
                        vec![name.as_str()]
                    }
                    Statement::TupleAssignment { left_name, right_name, .. } => vec![left_name.as_str(), right_name.as_str()],
                    Statement::FunctionCallAssign { bindings, .. } => bindings.iter().map(|binding| binding.name.as_str()).collect(),
                    Statement::StateFunctionCallAssign { bindings, .. } | Statement::StructDestructure { bindings, .. } => {
                        bindings.iter().map(|binding| binding.name.as_str()).collect()
                    }
                    _ => Vec::new(),
                };
                for name in declared {
                    if let Some(shadowed) = self.entry_parameters.get_mut(name) {
                        *shadowed = true;
                    }
                }
            }
        }
        Ok(())
    }
}
