//! Sil-specific lowering used by the current code generator.
//!
//! Structured body lowering and AST materialization live here.

mod body;
mod comments;
mod contract;
mod expr;
mod functions;
mod names;
mod state_boundary;
mod state_types;

pub(in crate::compiler::codegen) use comments::AnnotatedContractAst;
pub(in crate::compiler::codegen) use contract::ContractLowerer;
pub(in crate::compiler::codegen) use names::*;
pub(super) use state_boundary::render_sil_state_type;
