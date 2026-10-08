//! Evaluates bound integer constants for semantic cardinality planning.

use std::collections::{BTreeMap, BTreeSet};

use silverscript_lang::ast::{BinaryOp, Expr, ExprKind, UnaryOp};

use crate::compiler::resolve::{Binding, ResolvedDeclaration, ResolvedModules, ResolvedName};
use crate::compiler::syntax::TypeRef;
use crate::compiler::syntax::node::{ChildEdge, DeclId, RootSlot, SourceNodeCursor, SymbolKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConstIntError {
    Unknown,
    WrongType(String),
    InvalidLiteral,
    Cycle,
    Overflow,
    DivisionByZero,
}

/// Integer values are cached by declaration identity before entry planning.
pub(crate) struct ConstResolver {
    values: BTreeMap<String, std::result::Result<i64, ConstIntError>>,
}

impl ConstResolver {
    pub(crate) fn from_resolved(program: &ResolvedModules<'_>, names: &BTreeMap<DeclId, String>) -> Self {
        let mut cache = BTreeMap::new();
        let mut values = BTreeMap::new();
        for (id, name) in names {
            if id.kind() == SymbolKind::Const {
                values.insert(name.clone(), Self::evaluate(program, *id, &mut BTreeSet::new(), &mut cache));
            }
        }
        Self { values }
    }

    pub(crate) fn resolve_int(&self, name: &str) -> std::result::Result<i64, ConstIntError> {
        self.values.get(name).cloned().ok_or(ConstIntError::Unknown)?
    }

    fn evaluate(
        program: &ResolvedModules<'_>,
        id: DeclId,
        visiting: &mut BTreeSet<DeclId>,
        cache: &mut BTreeMap<DeclId, std::result::Result<i64, ConstIntError>>,
    ) -> std::result::Result<i64, ConstIntError> {
        if let Some(value) = cache.get(&id) {
            return value.clone();
        }
        if !visiting.insert(id) {
            return Err(ConstIntError::Cycle);
        }
        let result = (|| match program.declaration(id) {
            ResolvedDeclaration::Const(ct) if ct.ty == TypeRef::new("int") => {
                let expr = program.const_expression(id).map_err(|_| ConstIntError::InvalidLiteral)?;
                Self::evaluate_expr(program, id, &SourceNodeCursor::new(id, RootSlot::ConstValue), &expr, visiting, cache)
            }
            ResolvedDeclaration::Const(ct) => Err(ConstIntError::WrongType(ct.ty.to_source())),
            _ => Err(ConstIntError::InvalidLiteral),
        })();
        visiting.remove(&id);
        cache.insert(id, result.clone());
        result
    }

    fn evaluate_expr(
        program: &ResolvedModules<'_>,
        owner: DeclId,
        cursor: &SourceNodeCursor,
        expr: &Expr<'_>,
        visiting: &mut BTreeSet<DeclId>,
        cache: &mut BTreeMap<DeclId, std::result::Result<i64, ConstIntError>>,
    ) -> std::result::Result<i64, ConstIntError> {
        match &expr.kind {
            ExprKind::Int(value) => Ok(*value),
            ExprKind::Identifier(_) => {
                match program.nodes().find(&cursor.address).and_then(|site| program.bindings(owner).sites.get(&site)) {
                    Some(Binding::Source(ResolvedName::Declaration(id))) if id.kind() == SymbolKind::Const => {
                        Self::evaluate(program, *id, visiting, cache)
                    }
                    _ => Err(ConstIntError::InvalidLiteral),
                }
            }
            ExprKind::Unary { op: UnaryOp::Neg, expr } => {
                Self::evaluate_expr(program, owner, &cursor.child(ChildEdge::Expression), expr, visiting, cache)?
                    .checked_neg()
                    .ok_or(ConstIntError::Overflow)
            }
            ExprKind::Binary { op, left, right } => {
                let left = Self::evaluate_expr(program, owner, &cursor.child(ChildEdge::ExpressionLeft), left, visiting, cache)?;
                let right = Self::evaluate_expr(program, owner, &cursor.child(ChildEdge::ExpressionRight), right, visiting, cache)?;
                match op {
                    BinaryOp::Add => left.checked_add(right),
                    BinaryOp::Sub => left.checked_sub(right),
                    BinaryOp::Mul => left.checked_mul(right),
                    BinaryOp::Div if right == 0 => return Err(ConstIntError::DivisionByZero),
                    BinaryOp::Div => left.checked_div(right),
                    BinaryOp::Mod if right == 0 => return Err(ConstIntError::DivisionByZero),
                    BinaryOp::Mod => left.checked_rem(right),
                    _ => return Err(ConstIntError::InvalidLiteral),
                }
                .ok_or(ConstIntError::Overflow)
            }
            _ => Err(ConstIntError::InvalidLiteral),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::compiler::loader::load_inline_program;

    #[test]
    fn bound_integer_constants_detect_cycles_overflow_and_zero_division() {
        let program = load_inline_program(
            PathBuf::from("bound-constants.ag"),
            r#"
                const int BASE = 3;
                const int NEXT = BASE * 2 + 1;
                const int CYCLE_A = CYCLE_B;
                const int CYCLE_B = CYCLE_A;
                const int OVER = 9223372036854775807 + 1;
                const int ZERO = 1 / 0;
            "#
            .to_string(),
        )
        .expect("source resolves");
        let names = program.selected_declaration_names(None, &[]).expect("constants are selected");
        let values = ConstResolver::from_resolved(&program, &names);

        assert_eq!(values.resolve_int("NEXT"), Ok(7));
        assert_eq!(values.resolve_int("CYCLE_A"), Err(ConstIntError::Cycle));
        assert_eq!(values.resolve_int("CYCLE_B"), Err(ConstIntError::Cycle));
        assert_eq!(values.resolve_int("OVER"), Err(ConstIntError::Overflow));
        assert_eq!(values.resolve_int("ZERO"), Err(ConstIntError::DivisionByZero));
    }
}
