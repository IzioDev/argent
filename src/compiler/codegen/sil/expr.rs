//! Lowers bound authored helper expressions without editing source text.

use std::collections::BTreeSet;

use silverscript_lang::ast::visit::{AstVisitorMut, NameKind, walk_expr_mut, walk_statement_mut};
use silverscript_lang::ast::{ArrayDim, BinaryOp, Expr, ExprKind, Statement, TypeBase, TypeRef, UnaryOp};
use silverscript_lang::span::Span;

use crate::compiler::model::AppCompilationContext;
use crate::compiler::syntax::node::SymbolKind;
use crate::compiler::syntax::node::{DeclId, RootSlot};
use crate::compiler::syntax::word;
use crate::error::{ArgentError, Result};

use super::names::SilNames;

pub(super) struct HelperExpressionLowerer<'m, 'src> {
    names: SilNames<'m, 'src>,
    actor_fields: BTreeSet<String>,
    equivalent_states: Vec<String>,
    shared_constants: BTreeSet<String>,
    function_name: String,
    error: Option<ArgentError>,
}

/// Lowers a co-spend call already validated by the completed model.
pub(super) struct CoSpentLowerer<'n, 'm, 'src> {
    names: &'n SilNames<'m, 'src>,
    actor_fields: &'n BTreeSet<String>,
}

impl<'n, 'm, 'src> CoSpentLowerer<'n, 'm, 'src> {
    pub(super) fn new(names: &'n SilNames<'m, 'src>, actor_fields: &'n BTreeSet<String>) -> Self {
        Self { names, actor_fields }
    }

    pub(super) fn lower_call(&self, expr: &mut Expr<'_>) -> Result<()> {
        let ExprKind::Call { name, args, name_span } = &expr.kind else { return Ok(()) };
        if name != word::CO_SPENT {
            return Ok(());
        }
        if !self.names.is_builtin(*name_span) {
            return Err(ArgentError::new("co-spend call has no bound builtin identity"));
        }
        if args.len() != 1 || !self.names.co_spent_approved(*name_span) {
            return Err(ArgentError::new("`.co_spent()` requires one `cov_id` receiver"));
        }
        let ExprKind::Call { mut args, .. } = std::mem::replace(&mut expr.kind, ExprKind::Bool(false)) else {
            unreachable!("matched co-spend call above")
        };
        let operand = match args.remove(0) {
            Expr { kind: ExprKind::Call { name, mut args, .. }, .. } if name == word::COVENANT_ID && args.len() == 1 => args.remove(0),
            operand => operand,
        };
        let count = Expr::new(
            ExprKind::Call { name: "OpCovInputCount".to_string(), args: vec![operand], name_span: Span::default() },
            expr.span,
        );
        // Sil's AST formatter preserves parentheses for a negated comparison
        // when it appears inside another comparison or a boolean expression.
        let absent = Expr::new(ExprKind::Binary { op: BinaryOp::Eq, left: Box::new(count), right: Box::new(Expr::int(0)) }, expr.span);
        expr.kind = ExprKind::Unary { op: UnaryOp::Not, expr: Box::new(absent) };
        Ok(())
    }

    pub(super) fn lower_entry_statements(&self, statements: &mut [Statement<'src>]) -> Result<()> {
        let mut visitor = EntryCoSpentAstLowerer { lowerer: self, error: None };
        for statement in statements {
            visitor.visit_statement(statement);
        }
        visitor.error.map_or(Ok(()), Err)
    }
}

struct EntryCoSpentAstLowerer<'c, 'n, 'm, 'src> {
    lowerer: &'c CoSpentLowerer<'n, 'm, 'src>,
    error: Option<ArgentError>,
}

impl<'ast> AstVisitorMut<'ast> for EntryCoSpentAstLowerer<'_, '_, '_, '_> {
    fn visit_expr(&mut self, expr: &mut Expr<'ast>) {
        if let ExprKind::FieldAccess { source, field, .. } = &expr.kind
            && matches!(&source.kind, ExprKind::Identifier(root) if root == "self")
            && self.lowerer.actor_fields.contains(field)
        {
            expr.kind = ExprKind::Identifier(field.clone());
            return;
        }
        if matches!(&expr.kind, ExprKind::Call { name, .. } if name == word::CO_SPENT)
            && let Err(err) = self.lowerer.lower_call(expr)
        {
            self.error.get_or_insert(err);
            return;
        }
        if let ExprKind::Call { name, args, .. } = &mut expr.kind
            && name == word::COVENANT_ID
            && args.len() == 1
        {
            *name = "byte[32]".to_string();
        }
        walk_expr_mut(self, expr);
    }
}

impl<'m, 'src> HelperExpressionLowerer<'m, 'src> {
    pub(super) fn new(
        model: &'m AppCompilationContext<'src>,
        owner: DeclId,
        member: Option<usize>,
        actor_fields: BTreeSet<String>,
        equivalent_states: Vec<String>,
        shared_constants: BTreeSet<String>,
    ) -> Self {
        Self {
            names: SilNames::new(model, owner, member.map_or(RootSlot::Declaration, RootSlot::ActorFunction)),
            actor_fields,
            equivalent_states,
            shared_constants,
            function_name: model.resolution.declaration(owner).name().to_string(),
            error: None,
        }
    }

    pub(super) fn finish(self) -> Result<()> {
        self.error.map_or(Ok(()), Err)
    }

    pub(super) fn parameter_name(&self, index: usize) -> Option<&str> {
        self.names.parameter_name(index)
    }

    pub(super) fn lower_type(&self, ty: &mut TypeRef, span: Span<'_>) {
        match &mut ty.base {
            TypeBase::Custom(name)
                if name == word::COVENANT_ID
                    || name == word::ACTOR_TYPE
                    || self.names.type_target(span).is_some_and(|id| id.kind() == SymbolKind::ActorEnum) =>
            {
                ty.base = TypeBase::Byte;
                ty.array_dims.insert(0, ArrayDim::Fixed(32));
            }
            TypeBase::Custom(name) => {
                let bound = self.names.type_name(name, span);
                *name = if self.equivalent_states.contains(&bound) { "State".to_string() } else { bound };
            }
            TypeBase::Tuple(elements) => {
                for element in elements {
                    self.lower_type(element, span);
                }
            }
            _ => {}
        }
    }

    fn lower_struct_name(&self, name: &mut String, span: Span<'_>) {
        let bound = self.names.type_name(name, span);
        *name = if self.equivalent_states.contains(&bound) { "State".to_string() } else { bound };
    }
}

impl<'ast> AstVisitorMut<'ast> for HelperExpressionLowerer<'_, '_> {
    fn visit_name(&mut self, name: &mut String, kind: NameKind, span: Span<'ast>) {
        if matches!(kind, NameKind::LocalBinding | NameKind::LoopBinding | NameKind::StateBinding)
            && self.shared_constants.contains(name)
        {
            self.error.get_or_insert_with(|| {
                ArgentError::new(format!(
                    "global function `{}` binding `{name}` shadows a shared constant with the same name",
                    self.function_name,
                ))
            });
        }
        *name = self.names.name(name, kind, span);
    }

    fn visit_expr(&mut self, expr: &mut Expr<'ast>) {
        if let ExprKind::FieldAccess { source, field, .. } = &expr.kind
            && matches!(&source.kind, ExprKind::Identifier(root) if root == "self")
            && self.actor_fields.contains(field)
        {
            expr.kind = ExprKind::Identifier(field.clone());
            return;
        }
        if matches!(expr.kind, ExprKind::Identifier(_))
            && let Some(ordinal) = self.names.enum_ordinal(expr.span)
        {
            expr.kind = ExprKind::Int(ordinal);
            return;
        }
        if matches!(&expr.kind, ExprKind::Call { name, .. } if name == word::CO_SPENT) {
            let lowerer = CoSpentLowerer::new(&self.names, &self.actor_fields);
            if let Err(err) = lowerer.lower_call(expr) {
                self.error.get_or_insert(err);
                return;
            }
        }
        match &mut expr.kind {
            ExprKind::Call { name, args, .. } if name == word::COVENANT_ID && args.len() == 1 => {
                *name = "byte[32]".to_string();
            }
            ExprKind::Array { type_ref, type_span, .. } => self.lower_type(type_ref, *type_span),
            ExprKind::New { name, name_span, .. } | ExprKind::StructLiteral { name, name_span, .. } => {
                self.lower_struct_name(name, *name_span);
            }
            _ => {}
        }
        walk_expr_mut(self, expr);
    }

    fn visit_statement(&mut self, statement: &mut Statement<'ast>) {
        if let Statement::For { ident, ident_span, start, end, max_iterations, body, .. } = statement {
            self.visit_expr(start);
            self.visit_expr(end);
            self.visit_expr(max_iterations);
            self.visit_name(ident, NameKind::LoopBinding, *ident_span);
            for nested in body {
                self.visit_statement(nested);
            }
            return;
        }
        match statement {
            Statement::VariableDefinition { type_ref, type_span, .. } => {
                self.lower_type(type_ref, *type_span);
            }
            Statement::TupleAssignment { left_type_ref, left_type_span, right_type_ref, right_type_span, .. } => {
                self.lower_type(left_type_ref, *left_type_span);
                self.lower_type(right_type_ref, *right_type_span);
            }
            Statement::FunctionCallAssign { bindings, .. } => {
                for binding in bindings {
                    self.lower_type(&mut binding.type_ref, binding.type_span);
                }
            }
            Statement::StateFunctionCallAssign { target_struct, target_struct_span, bindings, .. } => {
                self.lower_struct_name(target_struct, *target_struct_span);
                for binding in bindings {
                    self.lower_type(&mut binding.type_ref, binding.type_span);
                }
            }
            Statement::StructDestructure { struct_name, struct_name_span, bindings, .. } => {
                self.lower_struct_name(struct_name, *struct_name_span);
                for binding in bindings {
                    self.lower_type(&mut binding.type_ref, binding.type_span);
                }
            }
            _ => {}
        }
        walk_statement_mut(self, statement);
    }
}
