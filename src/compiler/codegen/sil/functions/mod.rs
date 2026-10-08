//! Owns function-specific namespace lowering and actor capture validation.
//!
//! Sil classifies names and their source spans; Argent resolves which names
//! belong to the isolated global-function namespace.

use std::collections::BTreeSet;

use crate::compiler::model::{AppCompilationContext, CallableId, ResolvedTypeBase};
use crate::compiler::syntax::node::{DeclId, SymbolKind};
use crate::compiler::syntax::{ActorDecl, FunctionDecl, word};
use crate::error::ArgentError;
use crate::error::Result;
use silverscript_lang::ast::visit::AstVisitorMut;
use silverscript_lang::ast::{self as sil, FunctionAst, ParamAst};

use super::expr::HelperExpressionLowerer;
use super::state_types::StateValueTypes;
use super::state_types::lower_bound_type;

/// Turns retained authored helper statements into the function AST used by Sil.
pub(in crate::compiler::codegen) struct FunctionAstLowerer<'m, 'v, 'src> {
    model: &'m AppCompilationContext<'src>,
    state_values: &'v StateValueTypes<'m>,
}

impl<'m, 'v, 'src> FunctionAstLowerer<'m, 'v, 'src> {
    pub(in crate::compiler::codegen) fn new(model: &'m AppCompilationContext<'src>, state_values: &'v StateValueTypes<'m>) -> Self {
        Self { model, state_values }
    }

    pub(in crate::compiler::codegen) fn lower(
        &self,
        owner: DeclId,
        member: Option<usize>,
        function: &FunctionDecl,
        actor: Option<&ActorDecl>,
    ) -> Result<FunctionAst<'m>> {
        let source_body = self.model.resolution.function_body(owner, member)?;
        let global = owner.kind() == SymbolKind::Function;
        let mut actor_fields = BTreeSet::new();
        if actor.is_some() {
            actor_fields.extend(self.model.storage_state_for_actor(owner)?.fields.iter().map(|field| field.name.clone()));
        }
        let equivalent_states = self.state_values.equivalent_state_sources().map(|id| id.as_str().to_string()).collect();
        let shared_constants = if global {
            self.model.consts.iter().map(|(id, _)| self.model.types.display_names[id].clone()).collect::<BTreeSet<_>>()
        } else {
            BTreeSet::new()
        };
        if global {
            for param in &function.params {
                if shared_constants.contains(&param.name) {
                    return Err(ArgentError::new(format!(
                        "global function `{}` binding `{}` shadows a shared constant with the same name",
                        function.name, param.name,
                    )));
                }
            }
        }
        let mut lowerer = HelperExpressionLowerer::new(self.model, owner, member, actor_fields, equivalent_states, shared_constants);
        let callable_id = CallableId { owner, member };
        let signature = self
            .state_values
            .signature_id(callable_id)
            .ok_or_else(|| ArgentError::new(format!("missing contract-local signature for helper `{}`", function.name)))?;
        let resolved = self
            .model
            .types
            .callables
            .get(&callable_id)
            .ok_or_else(|| ArgentError::new(format!("missing resolved signature for helper `{}`", function.name)))?;
        let params = function
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let resolved_param = resolved
                    .params
                    .get(index)
                    .ok_or_else(|| ArgentError::new(format!("missing resolved parameter {index} for helper `{}`", function.name)))?;
                let mut type_ref = if let Some(value) = signature.param(index) {
                    self.state_values.sil_type_ref(value)
                } else if param.ty.name == word::COVENANT_ID
                    || param.ty.is_actor_type()
                    || matches!(resolved_param.base, ResolvedTypeBase::ActorEnum(_))
                {
                    sil::TypeRef { base: sil::TypeBase::Byte, array_dims: vec![sil::ArrayDim::Fixed(32)] }
                } else {
                    lower_bound_type(&param.ty, resolved_param)
                };
                lowerer.lower_type(&mut type_ref, sil::Span::default());
                Ok(ParamAst {
                    type_ref,
                    name: lowerer
                        .parameter_name(index)
                        .ok_or_else(|| ArgentError::new(format!("missing bound parameter {index} for helper `{}`", function.name)))?
                        .to_string(),
                    span: sil::Span::default(),
                    type_span: sil::Span::default(),
                    name_span: sil::Span::default(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let return_types = function
            .return_ty
            .as_ref()
            .map(|ty| {
                let resolved_result = resolved
                    .result
                    .as_ref()
                    .ok_or_else(|| ArgentError::new(format!("missing resolved result for helper `{}`", function.name)))?;
                let mut type_ref = if let Some(value) = signature.result() {
                    self.state_values.sil_type_ref(value)
                } else if ty.name == word::COVENANT_ID
                    || ty.is_actor_type()
                    || matches!(resolved_result.base, ResolvedTypeBase::ActorEnum(_))
                {
                    sil::TypeRef { base: sil::TypeBase::Byte, array_dims: vec![sil::ArrayDim::Fixed(32)] }
                } else {
                    lower_bound_type(ty, resolved_result)
                };
                lowerer.lower_type(&mut type_ref, sil::Span::default());
                Ok::<_, ArgentError>(type_ref)
            })
            .transpose()?
            .into_iter()
            .collect();
        let mut body = source_body.to_vec();
        for statement in &mut body {
            lowerer.visit_statement(statement);
        }
        lowerer.finish()?;
        Ok(FunctionAst {
            name: if global { self.model.types.display_names[&owner].clone() } else { function.name.clone() },
            attributes: Vec::new(),
            params,
            entrypoint: false,
            return_types,
            returns_tuple: false,
            body,
            return_type_spans: Vec::new(),
            span: sil::Span::default(),
            name_span: sil::Span::default(),
            body_span: sil::Span::default(),
        })
    }
}
