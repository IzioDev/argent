//! Retains each actor contract AST for base and fixed-context compilation.

use std::collections::BTreeMap;

use crate::compiler::syntax::node::{DeclId, SymbolKind};
use crate::error::{ArgentError, Result};
use silverscript_lang::ast::{ContractAst, Expr};
use silverscript_lang::compiler::{CompileOptions, CompiledContract, compile_contract_ast};

use super::sil::AnnotatedContractAst;

pub(super) struct CompiledActors<'src> {
    contracts: BTreeMap<DeclId, AnnotatedContractAst<'src>>,
}

impl<'src> CompiledActors<'src> {
    /// Retain the final actor ASTs used for both source formatting and compilation.
    pub(super) fn from_contracts(
        contracts: BTreeMap<DeclId, AnnotatedContractAst<'src>>,
        names: &BTreeMap<DeclId, String>,
    ) -> Result<Self> {
        for (actor, contract) in &contracts {
            if actor.kind() != SymbolKind::Actor {
                return Err(ArgentError::new(format!("generated Silverscript has non-actor identity `{actor:?}`")));
            }
            let name = names
                .get(actor)
                .ok_or_else(|| ArgentError::new(format!("generated Silverscript has unknown actor identity `{actor:?}`")))?;
            if contract.contract.name != *name {
                return Err(ArgentError::new(format!(
                    "generated Silverscript for actor `{name}` declares contract `{}`",
                    contract.contract.name
                )));
            }
            if contract.comments.iter().any(|comment| comment.text.is_empty()) {
                return Err(ArgentError::new(format!("generated Silverscript for actor `{name}` has an empty comment")));
            }
        }
        Ok(Self { contracts })
    }

    pub(super) fn compile_base<'i>(&'i self, actor: DeclId, args: &[Expr<'i>]) -> Result<CompiledContract<'i>> {
        let contract = self.contract(actor)?;
        compile_contract_ast(contract, args, CompileOptions::default())
            .map_err(|err| ArgentError::new(format!("generated Silverscript for actor `{}` failed to compile: {err}", contract.name)))
    }

    pub(super) fn compile_context<'i>(&'i self, actor: DeclId, args: &[Expr<'i>]) -> Result<CompiledContract<'i>> {
        let contract = self.contract(actor)?;
        compile_contract_ast(contract, args, CompileOptions::default()).map_err(|err| {
            ArgentError::new(format!(
                "generated Silverscript for actor `{}` failed to compile its source-state cut: {err}",
                contract.name
            ))
        })
    }

    fn contract(&self, actor: DeclId) -> Result<&ContractAst<'src>> {
        self.contracts
            .get(&actor)
            .map(|annotated| &annotated.contract)
            .ok_or_else(|| ArgentError::new(format!("missing generated Silverscript for actor identity `{actor:?}`")))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::compiler::syntax::node::{DeclId, ModuleId, SymbolKind};
    use silverscript_lang::ast::{
        ContractAst, Expr, ExprKind, FunctionAst, ParamAst, PragmaDirectiveAst, Statement, TypeBase, TypeRef, format_contract_ast,
        parse_contract_ast,
    };
    use silverscript_lang::compiler::{CompileOptions, compile_contract};
    use silverscript_lang::span::Span;

    use super::{AnnotatedContractAst, CompiledActors};

    #[test]
    fn base_and_context_compilation_match_the_emitted_contract() {
        let source = "pragma silverscript ^0.1.0; contract Counter(int init_count) { int count = init_count; entry check() { require(count >= 0); } }";
        let actor = DeclId::new(ModuleId::new(0), SymbolKind::Actor, 0);
        let contract = parse_contract_ast(source).expect("contract parses");
        let names = BTreeMap::from([(actor, "Counter".to_string())]);
        let actors =
            CompiledActors::from_contracts(BTreeMap::from([(actor, AnnotatedContractAst { contract, comments: Vec::new() })]), &names)
                .expect("contract identity matches");
        let state = DeclId::new(ModuleId::new(0), SymbolKind::State, 0);
        let wrong = CompiledActors::from_contracts(
            BTreeMap::from([(
                state,
                AnnotatedContractAst { contract: parse_contract_ast(source).expect("contract parses again"), comments: Vec::new() },
            )]),
            &BTreeMap::from([(state, "Counter".to_string())]),
        )
        .err()
        .expect("a state identity cannot own an actor contract");
        assert!(wrong.to_string().contains("non-actor identity"));
        let formatted = format_contract_ast(actors.contract(actor).expect("contract retained"));

        for count in [0, 7] {
            let args = [Expr::int(count)];
            let expected = compile_contract(source, &args, CompileOptions::default()).expect("source compiles");
            let actual = if count == 0 {
                actors.compile_base(actor, &args).expect("base compiles")
            } else {
                actors.compile_context(actor, &args).expect("context compiles")
            };
            assert_eq!(actual.bytecode, expected.bytecode);
            assert_eq!(actual.template_hash(), expected.template_hash());
            assert_eq!(actual.state_layout, expected.state_layout);
            let reparsed = compile_contract(&formatted, &args, CompileOptions::default()).expect("formatted source compiles");
            assert_eq!(reparsed.bytecode, actual.bytecode);
            assert_eq!(reparsed.template_hash(), actual.template_hash());
        }
    }

    #[test]
    fn constructed_contract_compiles_without_a_generated_source_buffer() {
        let actor = DeclId::new(ModuleId::new(0), SymbolKind::Actor, 0);
        let span = Span::default();
        let int_type = TypeRef { base: TypeBase::Int, array_dims: Vec::new() };
        let contract = ContractAst {
            pragma: Some(PragmaDirectiveAst {
                name: "silverscript".to_string(),
                value: "^0.1.0".to_string(),
                span,
                name_span: span,
                value_span: span,
            }),
            name: "Counter".to_string(),
            params: vec![ParamAst { type_ref: int_type, name: "init_count".to_string(), span, type_span: span, name_span: span }],
            structs: Vec::new(),
            fields: Vec::new(),
            constants: Vec::new(),
            functions: vec![FunctionAst {
                name: "check".to_string(),
                attributes: Vec::new(),
                params: Vec::new(),
                entrypoint: true,
                return_types: Vec::new(),
                returns_tuple: false,
                body: vec![Statement::Require {
                    expr: Expr::new(ExprKind::Bool(true), span),
                    message: None,
                    span,
                    message_span: None,
                }],
                return_type_spans: Vec::new(),
                span,
                name_span: span,
                body_span: span,
            }],
            span,
            name_span: span,
        };
        let names = BTreeMap::from([(actor, "Counter".to_string())]);
        let actors =
            CompiledActors::from_contracts(BTreeMap::from([(actor, AnnotatedContractAst { contract, comments: Vec::new() })]), &names)
                .expect("constructed actor identity matches");
        let other = DeclId::new(ModuleId::new(0), SymbolKind::Actor, 1);
        assert!(actors.contract(other).is_err(), "another actor identity cannot borrow the retained contract");
        assert!(actors.compile_context(other, &[Expr::int(7)]).is_err());
        let compiled = actors.compile_base(actor, &[Expr::int(7)]).expect("constructed AST compiles");
        let formatted = format_contract_ast(actors.contract(actor).expect("contract retained"));
        let reparsed = compile_contract(&formatted, &[Expr::int(7)], CompileOptions::default()).expect("formatted AST compiles");
        assert_eq!(compiled.bytecode, reparsed.bytecode);
        assert_eq!(compiled.template_hash(), reparsed.template_hash());
    }
}
