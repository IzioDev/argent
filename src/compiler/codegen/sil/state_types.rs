//! Checked AST-directed lowering of contract-local authored state types.
//!
//! Bound source identities select Sil representations; the final AST audit
//! rejects omitted authored types that remain in any type node.

use std::collections::{BTreeMap, BTreeSet};

use silverscript_lang::ast::visit::{AstVisitorMut, walk_expr_mut};
use silverscript_lang::ast::{
    ArrayDim as SilArrayDim, ContractAst, ContractFieldAst, Expr, ExprKind, ParamAst, StructAst, StructFieldAst, TypeBase, TypeRef,
};
use silverscript_lang::span::Span;

use crate::compiler::model::{
    ActorValuePlan, AppCompilationContext, BoundRouteActor, CallableId, CallableSignaturePlan, ContractStateLowering,
    FixedArrayLength, PhysicalFieldId, PhysicalStateLayout, PlannedStateValue, ResolvedSuccessor, ResolvedType, ResolvedTypeBase,
    SilStateType, SourceFieldId, SourceStateId, StateValueShape,
};
use crate::compiler::syntax::lexer::RESERVED_GENERATED_PREFIX;
use crate::compiler::syntax::node::{DeclId, EntryId, RootSlot};
use crate::compiler::syntax::{ActorDecl, ArrayDim as ArgentArrayDim, TypeRef as ArgentTypeRef, word};
use crate::error::{ArgentError, Result};

use super::ContractLowerer;
use super::names::{SilNames, hidden_physical_field_init_name};
use super::state_boundary::{output_state_target, plan_selector_output_state, render_sil_state_type};

/// Renders completed state-value facts in this actor's Sil representation.
pub(in crate::compiler::codegen) struct StateValueTypes<'m> {
    representations: &'m ContractStateLowering,
    values: &'m ActorValuePlan,
}

impl<'m> StateValueTypes<'m> {
    pub(in crate::compiler::codegen) fn new(actor: DeclId, model: &'m AppCompilationContext<'_>) -> Result<Self> {
        Ok(Self { representations: model.state_lowering_by_id(actor)?, values: model.actor_value_plan_by_id(actor)? })
    }

    pub(in crate::compiler::codegen) fn signature_id(&self, id: CallableId) -> Option<&CallableSignaturePlan> {
        self.values.signature_ids.get(&id)
    }

    pub(in crate::compiler::codegen) fn constant_id(&self, id: DeclId) -> Option<&PlannedStateValue> {
        self.values.constant_ids.get(&id)
    }

    pub(in crate::compiler::codegen) fn entry_param(&self, entry: EntryId, index: usize) -> Option<&PlannedStateValue> {
        self.values.entry_param_ids.get(&(entry, index))
    }

    pub(in crate::compiler::codegen) fn field_value(&self, field: &SourceFieldId) -> Option<&PlannedStateValue> {
        self.values.field_values.get(field)
    }

    pub(in crate::compiler::codegen) fn digest_helper_name(&self, source: &SourceStateId) -> String {
        format!("{RESERVED_GENERATED_PREFIX}digest_{}", source.as_str())
    }

    pub(in crate::compiler::codegen) fn sil_type_ref(&self, value: &PlannedStateValue) -> TypeRef {
        let element_type = self.authored_sil_type(value.source()).expect("planned state value belongs to this contract");
        let array_dims = match value.shape() {
            StateValueShape::Scalar => Vec::new(),
            StateValueShape::FixedArray(FixedArrayLength::Known(len)) => vec![SilArrayDim::Fixed(len)],
            StateValueShape::FixedArray(FixedArrayLength::Unresolved) => vec![SilArrayDim::Inferred],
            StateValueShape::DynamicArray => vec![SilArrayDim::Dynamic],
        };
        TypeRef { base: TypeBase::Custom(element_type.to_string()), array_dims }
    }

    pub(in crate::compiler::codegen) fn authored_sil_type(&self, source: &SourceStateId) -> Option<&str> {
        match self.representations.source_representation(source)?.sil_type() {
            SilStateType::State => Some("State"),
            SilStateType::Source(planned) if planned == source => Some(planned.as_str()),
            SilStateType::Source(_) | SilStateType::StoragePhysical(_) | SilStateType::TargetPhysical(_) => None,
        }
    }

    pub(in crate::compiler::codegen) fn equivalent_state_sources(&self) -> impl Iterator<Item = &SourceStateId> {
        self.representations
            .source_representations()
            .iter()
            .filter_map(|(source, plan)| matches!(plan.sil_type(), SilStateType::State).then_some(source))
    }

    pub(in crate::compiler::codegen) fn required_named_source_declarations(&self) -> impl Iterator<Item = &SourceStateId> {
        self.values.required_sources.iter().filter(|source| {
            self.representations.source_representation(source).is_some_and(|plan| !matches!(plan.sil_type(), SilStateType::State))
        })
    }
}

struct BoundStateExprLowerer<'a, 'm, 'p, 'src> {
    names: &'a SilNames<'m, 'src>,
    state_values: &'a StateValueTypes<'p>,
    error: Option<ArgentError>,
}

impl BoundStateExprLowerer<'_, '_, '_, '_> {
    fn lower_name(&mut self, name: &mut String, span: Span<'_>) {
        match self.names.state_type_source(span) {
            Ok(Some(source)) => {
                if let Some(authored) = self.state_values.authored_sil_type(&source) {
                    *name = authored.to_string();
                } else {
                    self.error.get_or_insert_with(|| {
                        ArgentError::new(format!("bound state `{}` has no authored Sil representation", source.as_str()))
                    });
                }
            }
            Ok(None) => {}
            Err(err) => {
                self.error.get_or_insert(err);
            }
        }
    }
}

impl<'i> AstVisitorMut<'i> for BoundStateExprLowerer<'_, '_, '_, '_> {
    fn visit_expr(&mut self, expr: &mut Expr<'i>) {
        match &mut expr.kind {
            ExprKind::StructLiteral { name, name_span, .. } | ExprKind::New { name, name_span, .. } => {
                self.lower_name(name, *name_span);
            }
            ExprKind::Array { type_ref: TypeRef { base: TypeBase::Custom(name), .. }, type_span, .. } => {
                self.lower_name(name, *type_span);
            }
            _ => {}
        }
        walk_expr_mut(self, expr);
    }
}

impl StateValueTypes<'_> {
    pub(in crate::compiler::codegen) fn lower_bound_expression_types(
        &self,
        expr: &mut Expr<'_>,
        names: &SilNames<'_, '_>,
    ) -> Result<()> {
        let mut lowerer = BoundStateExprLowerer { names, state_values: self, error: None };
        lowerer.visit_expr(expr);
        lowerer.error.map_or(Ok(()), Err)
    }
}

impl ContractLowerer<'_, '_> {
    /// Select required authored and physical structs for this actor.
    pub(in crate::compiler::codegen) fn state_structs(&self) -> Result<(BTreeSet<String>, Vec<StructAst<'static>>)> {
        let mut emitted = BTreeSet::new();
        let mut structs = Vec::new();
        let mut omitted_authored_structs =
            self.state_values.equivalent_state_sources().map(|source| source.as_str().to_string()).collect::<BTreeSet<_>>();
        let named_authored_states = self.state_values.required_named_source_declarations().cloned().collect::<BTreeSet<_>>();

        // Keep local declarations in their established order, then append linked
        // declarations discovered authoritatively by the contract value plan.
        let mut state_sources = self
            .model
            .app_actors
            .iter_with_ids()
            .map(|(actor, _)| {
                let state =
                    self.model.types.actor_states.get(&actor).ok_or_else(|| ArgentError::new("selected actor has no bound state"))?;
                self.model.source_state_id_by_decl(*state)
            })
            .collect::<Result<Vec<_>>>()?;
        state_sources.extend(self.model.states.keys().map(|name| self.model.source_state_id(name)).collect::<Result<Vec<_>>>()?);
        state_sources.extend(named_authored_states.iter().cloned());
        let mut emitted_sources = BTreeSet::new();
        for candidate in state_sources {
            let Some(source) = named_authored_states.get(&candidate) else {
                continue;
            };
            if !emitted_sources.insert(source.clone()) {
                continue;
            }
            let state_name = source.as_str();
            if !emitted.insert(state_name.to_string()) {
                return Err(ArgentError::new(format!("distinct source states share Sil struct name `{state_name}`")));
            }
            let structure = self.authored_struct(source)?;
            structs.push(structure);
        }

        let lowering = self.model.state_lowering_by_id(self.actor_id)?;
        let mut named_physical_layouts = BTreeMap::new();
        for (index, _) in self.actor.entries.iter().enumerate() {
            let entry_id = EntryId { actor: self.actor_id, index };
            for target_id in self.model.input_plan_by_id(entry_id)?.external_targets() {
                let target = lowering.target(target_id).ok_or_else(|| ArgentError::new("input requirement has no physical target"))?;
                let sil_type = render_sil_state_type(target.sil_type())?;
                if sil_type == "State" {
                    continue;
                }
                insert_named_physical_layout(&mut named_physical_layouts, &sil_type, target.physical(), self.actor)?;
            }
        }

        let output_plan = self.model.output_plan_by_id(self.actor_id)?;
        for (index, _) in self.actor.entries.iter().enumerate() {
            let entry_id = EntryId { actor: self.actor_id, index };
            let entry_model = self.model.entry_model_by_id(entry_id)?;
            let entry_outputs = self.model.entry_output_plan_by_id(entry_id)?;
            for group in entry_model.existing_groups() {
                for interaction in group.outputs() {
                    let target_id = entry_outputs.observed_target(interaction.id())?;
                    let output = output_state_target(self.actor_id, output_plan.target(target_id)?, self.model)?;
                    if let Some((sil_type, layout)) = output.named_physical_layout() {
                        insert_named_physical_layout(&mut named_physical_layouts, sil_type, layout, self.actor)?;
                    }
                }
            }
            for group in entry_model.genesis_groups() {
                for interaction in group.outputs() {
                    let target_id = entry_outputs.spawned_target(interaction.id())?;
                    let output = output_state_target(self.actor_id, output_plan.target(target_id)?, self.model)?;
                    if let Some((sil_type, layout)) = output.named_physical_layout() {
                        insert_named_physical_layout(&mut named_physical_layouts, sil_type, layout, self.actor)?;
                    }
                }
            }
            for route in entry_model.routes() {
                let ResolvedSuccessor::Constructed { bound: Some(bound), .. } = &route.successor else {
                    continue;
                };
                if matches!(bound.actor_target, BoundRouteActor::Fixed(_) | BoundRouteActor::Linked(_)) {
                    for target in self.model.route_target_ids_by_id(entry_id, route)? {
                        let output = output_state_target(self.actor_id, output_plan.actor(&target)?, self.model)?;
                        if let Some((sil_type, layout)) = output.named_physical_layout() {
                            insert_named_physical_layout(&mut named_physical_layouts, sil_type, layout, self.actor)?;
                        }
                    }
                }
            }
        }

        for (index, _) in self.actor.entries.iter().enumerate() {
            let entry_id = EntryId { actor: self.actor_id, index };
            for selector in self.model.entry_model_by_id(entry_id)?.template_selectors().values() {
                let output = plan_selector_output_state(self.actor_id, selector, self.model)?;
                if let Some((sil_type, layout)) = output.named_physical_layout() {
                    insert_named_physical_layout(&mut named_physical_layouts, sil_type, layout, self.actor)?;
                }
            }
        }

        for (sil_type, layout) in named_physical_layouts {
            if !emitted.insert(sil_type.clone()) {
                continue;
            }
            omitted_authored_structs.remove(&sil_type);
            structs.push(self.physical_struct(&sil_type, &layout));
        }
        Ok((omitted_authored_structs, structs))
    }

    /// Lower the authored representation of a required source state.
    pub(in crate::compiler::codegen) fn authored_struct(&self, source: &SourceStateId) -> Result<StructAst<'static>> {
        let storage_state = self.model.storage_state_by_source(source)?;
        let fields = storage_state
            .fields
            .iter()
            .map(|field| {
                let field_id = SourceFieldId::new(source.clone(), &field.name);
                let mut type_ref = lower_planned_type(&field.ty, false);
                if let Some(value) = self.state_values.field_value(&field_id) {
                    type_ref = self.state_values.sil_type_ref(value);
                }
                Ok(StructFieldAst {
                    type_ref,
                    name: field.name.clone(),
                    span: Span::default(),
                    type_span: Span::default(),
                    name_span: Span::default(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(StructAst { name: source.as_str().to_string(), fields, span: Span::default(), name_span: Span::default() })
    }

    /// Lower one named physical layout in generated-field then storage-field order.
    pub(in crate::compiler::codegen) fn physical_struct(&self, sil_type: &str, layout: &PhysicalStateLayout) -> StructAst<'static> {
        let fields = layout
            .fields()
            .iter()
            .filter(|field| matches!(field.id(), PhysicalFieldId::Generated(_)))
            .chain(layout.fields().iter().filter(|field| matches!(field.id(), PhysicalFieldId::Storage(_))))
            .map(|field| StructFieldAst {
                type_ref: lower_planned_type(field.ty(), false),
                name: field.sil_name().to_string(),
                span: Span::default(),
                type_span: Span::default(),
                name_span: Span::default(),
            })
            .collect();
        StructAst { name: sil_type.to_string(), fields, span: Span::default(), name_span: Span::default() }
    }
}

pub(in crate::compiler::codegen) fn contract_state_shell<'src>(
    actor_id: DeclId,
    model: &AppCompilationContext<'src>,
) -> Result<(Vec<ParamAst<'src>>, Vec<ContractFieldAst<'src>>)> {
    let state = model.storage_state_for_actor(actor_id)?;
    let physical = model.state_lowering_by_id(actor_id)?.active().physical();
    let names = SilNames::new(model, actor_id, RootSlot::ActorState);
    let mut params = Vec::new();
    let mut fields = Vec::new();
    for field in physical.fields().iter().filter(|field| matches!(field.id(), PhysicalFieldId::Generated(_))) {
        let PhysicalFieldId::Generated(id) = field.id() else { unreachable!("filtered generated fields") };
        let name = names.generated_field_name(id).ok_or_else(|| ArgentError::new("missing allocated generated field name"))?;
        let init_name = hidden_physical_field_init_name(name);
        let type_ref = lower_planned_type(field.ty(), false);
        params.push(ParamAst {
            type_ref: type_ref.clone(),
            name: init_name.clone(),
            span: Span::default(),
            type_span: Span::default(),
            name_span: Span::default(),
        });
        fields.push(ContractFieldAst {
            type_ref,
            name: name.to_string(),
            expr: Expr::identifier(init_name),
            span: Span::default(),
            type_span: Span::default(),
            name_span: Span::default(),
        });
    }
    for field in &state.fields {
        let init_name = format!("init_{}", field.name);
        let type_ref = lower_planned_type(&field.ty, false);
        params.push(ParamAst {
            type_ref: type_ref.clone(),
            name: init_name.clone(),
            span: Span::default(),
            type_span: Span::default(),
            name_span: Span::default(),
        });
        fields.push(ContractFieldAst {
            type_ref,
            name: field.name.clone(),
            expr: Expr::identifier(init_name),
            span: Span::default(),
            type_span: Span::default(),
            name_span: Span::default(),
        });
    }
    Ok((params, fields))
}

/// Materialize the selected contract representation without rendering an
/// authored type and asking the Sil parser to recover it.
/// Render a type whose nominal category was established during model construction.
pub(in crate::compiler::codegen) fn lower_bound_type(ty: &ArgentTypeRef, resolved: &ResolvedType) -> TypeRef {
    lower_planned_type(ty, matches!(resolved.base, ResolvedTypeBase::ActorEnum(_)))
}

/// Render a type using the model's resolved actor-enum category.
pub(in crate::compiler::codegen) fn lower_planned_type(ty: &ArgentTypeRef, actor_enum: bool) -> TypeRef {
    if ty.is_actor_type() || (ty.name == crate::compiler::syntax::word::COVENANT_ID && ty.array.is_none()) {
        return TypeRef { base: TypeBase::Byte, array_dims: vec![SilArrayDim::Fixed(32)] };
    }
    let base = if actor_enum {
        TypeBase::Int
    } else {
        match ty.name.as_str() {
            "int" => TypeBase::Int,
            "temporal" => TypeBase::Temporal,
            "bool" => TypeBase::Bool,
            "string" => TypeBase::String,
            "pubkey" => TypeBase::Pubkey,
            "sig" => TypeBase::Sig,
            "datasig" => TypeBase::Datasig,
            "byte" => TypeBase::Byte,
            name => TypeBase::Custom(name.to_string()),
        }
    };
    let array_dims = match ty.array {
        Some(ArgentArrayDim::Dynamic) => vec![SilArrayDim::Dynamic],
        Some(ArgentArrayDim::Fixed(len)) => vec![SilArrayDim::Fixed(len)],
        None => Vec::new(),
    };
    TypeRef { base, array_dims }
}

struct OmittedStateAudit<'a> {
    omitted: &'a BTreeSet<String>,
    violation: Option<(String, usize)>,
}

impl<'i> AstVisitorMut<'i> for OmittedStateAudit<'_> {
    fn visit_type(&mut self, type_ref: &TypeRef, span: Span<'i>) {
        if let TypeBase::Custom(name) = &type_ref.base
            && self.omitted.contains(name)
        {
            self.violation.get_or_insert_with(|| (name.clone(), span.start()));
        }
    }
}

pub(in crate::compiler::codegen) fn audit_omitted_equivalent_state_structs(
    contract: &mut ContractAst<'_>,
    omitted: &BTreeSet<String>,
) -> Result<()> {
    if omitted.is_empty() {
        return Ok(());
    }
    if let Some(state) = contract.structs.iter().find(|item| omitted.contains(&item.name)) {
        return Err(ArgentError::new(format!(
            "optimized authored struct `{}` was declared after being omitted from generated Sil",
            state.name
        )));
    }

    let mut audit = OmittedStateAudit { omitted, violation: None };
    audit.visit_contract(contract);
    if let Some((name, offset)) = audit.violation {
        return Err(ArgentError::new(format!(
            "omitted authored state `{name}` remains in a generated Sil type or constructor at offset {offset}"
        )));
    }
    Ok(())
}

pub(in crate::compiler::codegen) fn source_type_ref(ty: &ArgentTypeRef) -> String {
    if let Some(state) = &ty.actor_state { format!("{}<{state}>", word::ACTOR_TYPE) } else { ty.to_sil() }
}

fn insert_named_physical_layout(
    layouts: &mut BTreeMap<String, PhysicalStateLayout>,
    sil_type: &str,
    layout: &PhysicalStateLayout,
    actor: &ActorDecl,
) -> Result<()> {
    if let Some(existing) = layouts.get(sil_type)
        && !layout.is_sil_compatible_with(existing)
    {
        return Err(ArgentError::new(format!(
            "physical type `{sil_type}` names incompatible target layouts in actor `{}`",
            actor.name
        )));
    }
    layouts.insert(sil_type.to_string(), layout.clone());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::syntax::node::{ModuleId, SymbolKind};
    use silverscript_lang::ast::parse_contract_ast;

    #[test]
    fn bound_type_category_overrides_a_shared_display_spelling() {
        let authored = ArgentTypeRef::new("Choice");
        let state = ResolvedType { base: ResolvedTypeBase::State(DeclId::new(ModuleId::new(0), SymbolKind::State, 0)), array: None };
        let actor_enum =
            ResolvedType { base: ResolvedTypeBase::ActorEnum(DeclId::new(ModuleId::new(1), SymbolKind::ActorEnum, 0)), array: None };

        assert!(matches!(lower_bound_type(&authored, &state).base, TypeBase::Custom(name) if name == "Choice"));
        assert!(matches!(lower_bound_type(&authored, &actor_enum).base, TypeBase::Int));
    }

    #[test]
    fn omitted_name_audit_includes_struct_field_types() {
        let source = r#"contract Inspect() {
    struct Wrapper {
        CounterState value;
    }
}"#;
        let omitted = ["CounterState".to_string()].into_iter().collect();
        let mut contract = parse_contract_ast(source).expect("audit fixture parses");
        let err = audit_omitted_equivalent_state_structs(&mut contract, &omitted)
            .expect_err("an omitted authored name cannot remain in a struct field type");
        assert!(err.to_string().contains("omitted authored state `CounterState` remains"), "unexpected error: {err}");
    }

    #[test]
    fn omitted_name_audit_includes_as_cast_target_types() {
        let source = r#"contract Inspect() {
    function retain(State[] values) : State[] {
        return values as CounterState[];
    }
}"#;
        let omitted = ["CounterState".to_string()].into_iter().collect();
        let mut contract = parse_contract_ast(source).expect("audit fixture parses");
        let err = audit_omitted_equivalent_state_structs(&mut contract, &omitted)
            .expect_err("an omitted authored name cannot remain in an as-cast target");

        assert!(err.to_string().contains("omitted authored state `CounterState` remains"), "unexpected error: {err}");
    }
}
