use std::path::PathBuf;

use silverscript_lang::ast::{ArrayDim as SilArrayDim, TypeBase};

use crate::compiler::loader::SourceSet;
use crate::compiler::syntax::node::{ChildEdge, DeclId, ModuleId, NodeAddress, RootSlot, SymbolKind};
use crate::compiler::syntax::source::{Origin, SourceId};

#[test]
fn declaration_type_uses_keep_sil_carriers_and_actor_state_metadata() {
    let source = "const int[2] VALUES = int[2] {1, 2};\nfn read(actor_type<lib::State> target) -> byte[32] { return target; }";
    let sources = SourceSet::discover_inline(PathBuf::from("types.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let module = &program.modules[0];
    let constant = DeclId::new(ModuleId::new(0), SymbolKind::Const, 0);
    let function = DeclId::new(ModuleId::new(0), SymbolKind::Function, 0);

    let constant_type = program
        .nodes
        .find(&NodeAddress { owner: constant, root: RootSlot::ConstType, children: Vec::new() })
        .expect("constant type site");
    let constant_use = &module.type_uses[&constant_type];
    assert_eq!(constant_use.ty.base, TypeBase::Int);
    assert_eq!(constant_use.ty.array_dims, vec![SilArrayDim::Fixed(2)]);

    let parameter_type = program
        .nodes
        .find(&NodeAddress { owner: function, root: RootSlot::Declaration, children: vec![ChildEdge::ParamType(0)] })
        .expect("parameter type site");
    let parameter_use = &module.type_uses[&parameter_type];
    assert_eq!(parameter_use.ty.base, TypeBase::Custom("actor_type".to_string()));
    assert!(parameter_use.ty.array_dims.is_empty());
    let state = parameter_use.actor_state.as_ref().expect("actor state metadata");
    assert_eq!(state.segments, ["lib", "State"]);
    let start = source.find("lib::State").unwrap();
    assert_eq!(state.origin, Origin::Authored { source: SourceId(0), start, end: start + "lib::State".len() });

    let return_type = program
        .nodes
        .find(&NodeAddress { owner: function, root: RootSlot::Declaration, children: vec![ChildEdge::ReturnType] })
        .expect("return type site");
    assert_eq!(module.type_uses[&return_type].ty.array_dims, vec![SilArrayDim::Fixed(32)]);
}

#[test]
fn entry_binding_type_metadata_uses_its_structural_site() {
    let source = "actor Agent owns State { entry step() emits none { actor_type<State> handle = source; } }";
    let sources = SourceSet::discover_inline(PathBuf::from("entry-type.ag"), source.to_string()).expect("source discovery");
    let program = sources.parse_modules().expect("source parser");
    let actor = DeclId::new(ModuleId::new(0), SymbolKind::Actor, 0);
    let type_site = program
        .nodes
        .find(&NodeAddress {
            owner: actor,
            root: RootSlot::Entry(0),
            children: vec![ChildEdge::Body, ChildEdge::Statement(0), ChildEdge::TypeUse],
        })
        .expect("entry binding type site");
    let type_use = &program.modules[0].type_uses[&type_site];
    assert_eq!(type_use.ty.base, TypeBase::Custom("actor_type".to_string()));
    assert_eq!(type_use.actor_state.as_ref().expect("actor state").segments, ["State"]);
}
