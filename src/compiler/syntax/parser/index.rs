//! Structural source sites for parsed Sil and entry nodes.

use silverscript_lang::ast::{self as sil, ExprKind, Statement};

use super::{ArgentTypeUse, AuthoredEntryStatement, AuthoredSuccessor, ChildEdge, Origin, Parser, SourceNodeCursor};
use crate::compiler::syntax::node::{EntryId, ReferenceRole, RootSlot, SourceOperation};
use crate::compiler::syntax::word;

impl<'src> Parser<'src> {
    fn index_site(&mut self, cursor: &SourceNodeCursor, span: sil::Span<'src>, role: Option<ReferenceRole>) {
        let origin = Origin::Authored { source: self.file.id, start: span.start(), end: span.end() };
        let id = if let Some(id) = self.nodes.find(&cursor.address) {
            debug_assert!(
                matches!(self.nodes.node_origin(id), Origin::Authored { source, start, end } if source == self.file.id && start <= span.start() && end >= span.end())
            );
            if let Some(role) = role {
                self.nodes.set_role(id, role);
            }
            id
        } else {
            self.nodes.insert_with_role(cursor.address.clone(), origin, role)
        };
        if role == Some(ReferenceRole::Type)
            && let Some(type_use) = self.pending_type_uses.get(&(span.start(), span.end()))
        {
            self.type_uses.insert(id, type_use.clone());
        }
    }

    /// Retain type facts from an already parsed Sil node when no Argent type token was recorded.
    fn index_type_site(&mut self, cursor: &SourceNodeCursor, span: sil::Span<'src>, ty: &sil::TypeRef) {
        self.index_site(cursor, span, Some(ReferenceRole::Type));
        let id = self.nodes.find(&cursor.address).expect("type site was just indexed");
        self.type_uses.entry(id).or_insert_with(|| ArgentTypeUse { ty: ty.clone(), actor_state: None });
    }

    pub(super) fn index_expression(&mut self, cursor: &SourceNodeCursor, expr: &sil::Expr<'src>, role: Option<ReferenceRole>) {
        let node_role = role.or_else(|| matches!(expr.kind, ExprKind::Identifier(_)).then_some(ReferenceRole::Value));
        self.index_site(cursor, expr.span, node_role);
        match &expr.kind {
            ExprKind::Array { type_ref, values, type_span } => {
                self.index_type_site(&cursor.child(ChildEdge::TypeUse), *type_span, type_ref);
                for (index, value) in values.iter().enumerate() {
                    self.index_expression(&cursor.child(ChildEdge::Element(index)), value, None);
                }
            }
            ExprKind::Call { name, args, name_span } => {
                let role = if name.starts_with("__as_cast_") { ReferenceRole::Type } else { ReferenceRole::Call };
                let target = cursor.child(ChildEdge::CallTarget);
                self.index_site(&target, *name_span, Some(role));
                if name == word::STATE {
                    let id = self.nodes.find(&target.address).expect("indexed call target");
                    self.nodes.set_operation(id, SourceOperation::InputStateCall);
                }
                for (index, arg) in args.iter().enumerate() {
                    self.index_expression(&cursor.child(ChildEdge::Argument(index)), arg, None);
                }
            }
            ExprKind::New { args, name_span, .. } => {
                self.index_site(&cursor.child(ChildEdge::TypeUse), *name_span, Some(ReferenceRole::Type));
                for (index, arg) in args.iter().enumerate() {
                    self.index_expression(&cursor.child(ChildEdge::Argument(index)), arg, None);
                }
            }
            ExprKind::Split { source, index, .. } | ExprKind::ArrayIndex { source, index } => {
                self.index_expression(&cursor.child(ChildEdge::ExpressionSource), source, None);
                self.index_expression(&cursor.child(ChildEdge::ExpressionIndex), index, None);
            }
            ExprKind::Slice { source, start, end, .. } => {
                self.index_expression(&cursor.child(ChildEdge::ExpressionSource), source, None);
                self.index_expression(&cursor.child(ChildEdge::ExpressionStart), start, None);
                self.index_expression(&cursor.child(ChildEdge::ExpressionEnd), end, None);
            }
            ExprKind::Append { source, args, .. } => {
                self.index_expression(&cursor.child(ChildEdge::ExpressionSource), source, None);
                for (index, arg) in args.iter().enumerate() {
                    self.index_expression(&cursor.child(ChildEdge::Argument(index)), arg, None);
                }
            }
            ExprKind::Unary { expr, .. } => self.index_expression(&cursor.child(ChildEdge::Expression), expr, None),
            ExprKind::Binary { left, right, .. } => {
                self.index_expression(&cursor.child(ChildEdge::ExpressionLeft), left, None);
                self.index_expression(&cursor.child(ChildEdge::ExpressionRight), right, None);
            }
            ExprKind::Ternary { condition, then_expr, else_expr } => {
                self.index_expression(&cursor.child(ChildEdge::ExpressionCondition), condition, None);
                self.index_expression(&cursor.child(ChildEdge::ExpressionThen), then_expr, None);
                self.index_expression(&cursor.child(ChildEdge::ExpressionElse), else_expr, None);
            }
            ExprKind::IndexedIntrospection { index, .. } => {
                self.index_expression(&cursor.child(ChildEdge::ExpressionIndex), index, None);
            }
            ExprKind::StructLiteral { name, fields, name_span } => {
                if !name.is_empty() {
                    let target = cursor.child(ChildEdge::TypeUse);
                    self.index_type_site(
                        &target,
                        *name_span,
                        &sil::TypeRef { base: sil::TypeBase::Custom(name.clone()), array_dims: Vec::new() },
                    );
                    if name == "State" {
                        let id = self.nodes.find(&target.address).expect("indexed constructor type");
                        self.nodes.set_operation(id, SourceOperation::PhysicalStateConstructor);
                    }
                }
                for (index, field) in fields.iter().enumerate() {
                    let field_cursor = cursor.child(ChildEdge::Field(index));
                    if !field.name.is_empty() {
                        self.index_site(&field_cursor.child(ChildEdge::FieldLabel), field.name_span, Some(ReferenceRole::FieldLabel));
                    }
                    self.index_expression(&field_cursor.child(ChildEdge::Expression), &field.expr, None);
                }
            }
            ExprKind::FieldAccess { source, field_span, .. } => {
                self.index_expression(&cursor.child(ChildEdge::ExpressionSource), source, None);
                self.index_site(&cursor.child(ChildEdge::FieldLabel), *field_span, Some(ReferenceRole::FieldLabel));
            }
            ExprKind::UnarySuffix { source, .. } => self.index_expression(&cursor.child(ChildEdge::ExpressionSource), source, None),
            ExprKind::Int(_)
            | ExprKind::Temporal(_)
            | ExprKind::Bool(_)
            | ExprKind::Byte(_)
            | ExprKind::String(_)
            | ExprKind::DateLiteral(_)
            | ExprKind::Identifier(_)
            | ExprKind::Introspection(_)
            | ExprKind::NumberWithUnit { .. } => {}
        }
    }

    pub(super) fn index_statement_sequence(&mut self, cursor: &SourceNodeCursor, statements: &[Statement<'src>]) {
        for (index, statement) in statements.iter().enumerate() {
            self.index_statement(&cursor.child(ChildEdge::Statement(index)), statement);
        }
    }

    fn index_statement(&mut self, cursor: &SourceNodeCursor, statement: &Statement<'src>) {
        self.index_site(cursor, statement.span(), None);
        match statement {
            Statement::VariableDefinition { type_span, name_span, expr, .. } => {
                self.index_site(&cursor.child(ChildEdge::TypeUse), *type_span, Some(ReferenceRole::Type));
                self.index_site(&cursor.child(ChildEdge::BindingName), *name_span, Some(ReferenceRole::Binding));
                if let Some(expr) = expr {
                    self.index_expression(&cursor.child(ChildEdge::Expression), expr, None);
                }
            }
            Statement::TupleAssignment { left_type_span, left_name_span, right_type_span, right_name_span, expr, .. } => {
                let left = cursor.child(ChildEdge::ExpressionLeft);
                self.index_site(&left.child(ChildEdge::TypeUse), *left_type_span, Some(ReferenceRole::Type));
                self.index_site(&left.child(ChildEdge::BindingName), *left_name_span, Some(ReferenceRole::Binding));
                let right = cursor.child(ChildEdge::ExpressionRight);
                self.index_site(&right.child(ChildEdge::TypeUse), *right_type_span, Some(ReferenceRole::Type));
                self.index_site(&right.child(ChildEdge::BindingName), *right_name_span, Some(ReferenceRole::Binding));
                self.index_expression(&cursor.child(ChildEdge::Expression), expr, None);
            }
            Statement::FunctionCall { name, args, name_span, .. } => {
                let target = cursor.child(ChildEdge::CallTarget);
                self.index_site(&target, *name_span, Some(ReferenceRole::Call));
                if name == word::STATE {
                    let id = self.nodes.find(&target.address).expect("indexed call target");
                    self.nodes.set_operation(id, SourceOperation::InputStateCall);
                }
                for (index, arg) in args.iter().enumerate() {
                    self.index_expression(&cursor.child(ChildEdge::Argument(index)), arg, None);
                }
            }
            Statement::FunctionCallAssign { name, bindings, args, name_span, .. } => {
                let target = cursor.child(ChildEdge::CallTarget);
                self.index_site(&target, *name_span, Some(ReferenceRole::Call));
                if name == word::STATE {
                    let id = self.nodes.find(&target.address).expect("indexed call target");
                    self.nodes.set_operation(id, SourceOperation::InputStateCall);
                }
                for (index, binding) in bindings.iter().enumerate() {
                    let binding_cursor = cursor.child(ChildEdge::Element(index));
                    self.index_site(&binding_cursor.child(ChildEdge::TypeUse), binding.type_span, Some(ReferenceRole::Type));
                    self.index_site(&binding_cursor.child(ChildEdge::BindingName), binding.name_span, Some(ReferenceRole::Binding));
                }
                for (index, arg) in args.iter().enumerate() {
                    self.index_expression(&cursor.child(ChildEdge::Argument(index)), arg, None);
                }
            }
            Statement::StateFunctionCallAssign { bindings, args, target_struct_span, name_span, .. } => {
                self.index_site(&cursor.child(ChildEdge::TypeUse), *target_struct_span, Some(ReferenceRole::Type));
                self.index_site(&cursor.child(ChildEdge::CallTarget), *name_span, Some(ReferenceRole::Call));
                self.index_struct_bindings(cursor, bindings);
                for (index, arg) in args.iter().enumerate() {
                    self.index_expression(&cursor.child(ChildEdge::Argument(index)), arg, None);
                }
            }
            Statement::StructDestructure { bindings, expr, struct_name, struct_name_span, .. } => {
                if !struct_name.is_empty() {
                    self.index_site(&cursor.child(ChildEdge::TypeUse), *struct_name_span, Some(ReferenceRole::Type));
                }
                self.index_struct_bindings(cursor, bindings);
                self.index_expression(&cursor.child(ChildEdge::Expression), expr, None);
            }
            Statement::Assign { expr, name_span, .. } => {
                self.index_site(&cursor.child(ChildEdge::AssignmentTarget), *name_span, Some(ReferenceRole::AssignmentTarget));
                self.index_expression(&cursor.child(ChildEdge::Expression), expr, None);
            }
            Statement::RequireAgeDaa { expr, .. }
            | Statement::RequireTxDaa { expr, .. }
            | Statement::RequireTxTime { expr, .. }
            | Statement::Require { expr, .. } => self.index_expression(&cursor.child(ChildEdge::Expression), expr, None),
            Statement::Block { body, .. } => self.index_statement_sequence(cursor, body),
            Statement::If { condition, then_branch, else_branch, .. } => {
                self.index_expression(&cursor.child(ChildEdge::ExpressionCondition), condition, None);
                self.index_statement_sequence(&cursor.child(ChildEdge::ThenBranch), then_branch);
                if let Some(else_branch) = else_branch {
                    self.index_statement_sequence(&cursor.child(ChildEdge::ElseBranch), else_branch);
                }
            }
            Statement::For { ident_span, start, end, max_iterations, body, .. } => {
                self.index_site(&cursor.child(ChildEdge::LoopBinding), *ident_span, Some(ReferenceRole::Binding));
                self.index_expression(&cursor.child(ChildEdge::ExpressionStart), start, None);
                self.index_expression(&cursor.child(ChildEdge::ExpressionEnd), end, None);
                self.index_expression(&cursor.child(ChildEdge::ExpressionLimit), max_iterations, None);
                self.index_statement_sequence(cursor, body);
            }
            Statement::Return { exprs, .. } | Statement::Console { args: exprs, .. } => {
                for (index, expr) in exprs.iter().enumerate() {
                    self.index_expression(&cursor.child(ChildEdge::Argument(index)), expr, None);
                }
            }
        }
    }

    fn index_struct_bindings(&mut self, cursor: &SourceNodeCursor, bindings: &[sil::StructBindingAst<'src>]) {
        for (index, binding) in bindings.iter().enumerate() {
            let binding_cursor = cursor.child(ChildEdge::Field(index));
            self.index_site(&binding_cursor.child(ChildEdge::FieldLabel), binding.field_span, Some(ReferenceRole::FieldLabel));
            self.index_site(&binding_cursor.child(ChildEdge::TypeUse), binding.type_span, Some(ReferenceRole::Type));
            self.index_site(&binding_cursor.child(ChildEdge::BindingName), binding.name_span, Some(ReferenceRole::Binding));
        }
    }

    pub(super) fn index_entry_sequence(&mut self, cursor: &SourceNodeCursor, statements: &[AuthoredEntryStatement<'src>]) {
        for (index, statement) in statements.iter().enumerate() {
            self.index_entry_statement(&cursor.child(ChildEdge::Statement(index)), statement);
        }
    }

    fn index_entry_statement(&mut self, cursor: &SourceNodeCursor, statement: &AuthoredEntryStatement<'src>) {
        match statement {
            AuthoredEntryStatement::Block { statements, span } => {
                self.index_site(cursor, *span, None);
                self.index_entry_sequence(cursor, statements);
            }
            AuthoredEntryStatement::If { condition, then_branch, else_branch, span, .. } => {
                self.index_site(cursor, *span, None);
                self.index_expression(&cursor.child(ChildEdge::ExpressionCondition), condition, None);
                self.index_entry_statement(&cursor.child(ChildEdge::ThenBranch), then_branch);
                if let Some(else_branch) = else_branch {
                    self.index_entry_statement(&cursor.child(ChildEdge::ElseBranch), else_branch);
                }
            }
            AuthoredEntryStatement::Become { routes, span } | AuthoredEntryStatement::ForeignBecome { routes, span, .. } => {
                self.index_site(cursor, *span, None);
                if let AuthoredEntryStatement::ForeignBecome { group, .. } = statement {
                    self.index_origin(&cursor.child(ChildEdge::ForeignGroup), group.origin, ReferenceRole::ClauseTarget);
                }
                for (index, route) in routes.iter().enumerate() {
                    let route_cursor = cursor.child(ChildEdge::Route(index));
                    self.index_origin(&route_cursor.child(ChildEdge::RouteOutput), route.output.origin, ReferenceRole::RouteOutput);
                    match &route.successor {
                        AuthoredSuccessor::SelfRef { origin } => {
                            self.index_origin(&route_cursor.child(ChildEdge::RouteActor), *origin, ReferenceRole::ActorTarget);
                        }
                        AuthoredSuccessor::Constructed { actor, state, .. } => {
                            let actor_cursor = route_cursor.child(ChildEdge::RouteActor);
                            let state_cursor = route_cursor.child(ChildEdge::RouteState);
                            self.index_expression(&actor_cursor, actor, Some(ReferenceRole::ActorTarget));
                            self.index_expression(&state_cursor, state, None);
                            let RootSlot::Entry(entry_index) = cursor.address.root else {
                                unreachable!("entry route has an entry root");
                            };
                            let actor_site = self.nodes.find(&actor_cursor.address).expect("indexed route actor");
                            let state_site = self.nodes.find(&state_cursor.address).expect("indexed route state");
                            self.nodes.insert_route_sites(
                                EntryId { actor: cursor.address.owner, index: entry_index },
                                route.id,
                                actor_site,
                                state_site,
                            );
                        }
                    }
                }
            }
            AuthoredEntryStatement::Sil(statement) => self.index_statement(cursor, statement),
        }
    }

    fn index_origin(&mut self, cursor: &SourceNodeCursor, origin: Origin, role: ReferenceRole) {
        if let Origin::Authored { start, end, .. } = origin {
            self.index_site(cursor, self.sil_span(start, end), Some(role));
        }
    }
}
