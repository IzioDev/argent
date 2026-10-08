use super::*;
use silverscript_lang::compiler::{CompileOptions, compile_contract, sil_abi_artifact_from_compiled};

#[test]
fn canonicalizes_state_references_inside_array_types() {
    let mut fixed = TypeArtifact::FixedArray { item: Box::new(TypeArtifact::Struct { name: "State".to_string() }), len: 2 };
    let mut dynamic = TypeArtifact::DynamicArray { item: Box::new(TypeArtifact::Struct { name: "State".to_string() }) };

    assert!(type_references_state(&fixed));
    assert!(type_references_state(&dynamic));
    replace_state_type_ref(&mut fixed, "Details");
    replace_state_type_ref(&mut dynamic, "Details");

    assert!(!type_references_state(&fixed));
    assert!(!type_references_state(&dynamic));
    assert_eq!(fixed, TypeArtifact::FixedArray { item: Box::new(TypeArtifact::Struct { name: "Details".to_string() }), len: 2 });
    assert_eq!(dynamic, TypeArtifact::DynamicArray { item: Box::new(TypeArtifact::Struct { name: "Details".to_string() }) });
}

#[test]
fn sil_abi_merge_rejects_conflicting_structs_and_duplicate_contracts() {
    use crate::compiler::loader::SymbolKind;
    use crate::compiler::model::link::DeclarationOrigin;

    let compile = |contract: &str, field_type: &str| {
        let source = format!(
            r#"pragma silverscript ^0.1.0;
contract {contract}() {{
    struct Shared {{
        {field_type} value;
    }}

    entry hold() {{
        require(true);
    }}
}}
"#
        );
        let compiled = compile_contract(&source, &[], CompileOptions::default()).expect("test Sil compiles");
        sil_abi_artifact_from_compiled(&compiled, &[]).expect("test Sil ABI builds")
    };

    let left = compile("Left", "int");
    let source = AbiStructIdentity::Source(SourceStateId::from_origin(
        "Shared",
        DeclarationOrigin::Dependency { app: "App".to_string(), name: "Shared".to_string(), kind: SymbolKind::State },
    ));
    let sources = BTreeMap::from([("Shared".to_string(), source)]);
    let merged = merge_sil_abi_artifacts(left.clone(), compile("Right", "int"), &mut sources.clone(), sources.clone())
        .expect("identical shared structs merge");
    assert_eq!(merged.contracts.len(), 2);
    assert_eq!(merged.structs.len(), 1);

    let right = compile("Right", "bool");
    let err = merge_sil_abi_artifacts(left.clone(), right, &mut sources.clone(), sources.clone())
        .expect_err("different definitions of Shared must not merge");
    assert!(err.to_string().contains("conflicting Sil struct `Shared`"), "unexpected error: {err}");

    let err = merge_sil_abi_artifacts(left.clone(), left, &mut sources.clone(), sources)
        .expect_err("the same contract must not merge twice");
    assert!(err.to_string().contains("duplicate Sil contract `Left`"), "unexpected error: {err}");
}

#[test]
fn sil_abi_merge_requires_shared_source_for_equal_structs() {
    use crate::compiler::loader::SymbolKind;
    use crate::compiler::model::link::DeclarationOrigin;

    let compile = |contract: &str| {
        let source = format!(
            "pragma silverscript ^0.1.0; contract {contract}() {{ struct Shared {{ int value; }} entry hold() {{ require(true); }} }}"
        );
        let compiled = compile_contract(&source, &[], CompileOptions::default()).expect("test Sil compiles");
        sil_abi_artifact_from_compiled(&compiled, &[]).expect("test Sil ABI builds")
    };
    let source = |app: &str| {
        AbiStructIdentity::Source(SourceStateId::from_origin(
            "Shared",
            DeclarationOrigin::Dependency { app: app.to_string(), name: "Shared".to_string(), kind: SymbolKind::State },
        ))
    };
    let left = compile("Left");
    let right = compile("Right");
    let mut left_sources = BTreeMap::from([("Shared".to_string(), source("First"))]);
    let right_sources = BTreeMap::from([("Shared".to_string(), source("Second"))]);
    let err = merge_sil_abi_artifacts(left.clone(), right.clone(), &mut left_sources, right_sources)
        .expect_err("equal layouts from distinct sources must not merge");
    assert!(err.to_string().contains("distinct semantic identities"), "unexpected error: {err}");

    let err = merge_sil_abi_artifacts(left.clone(), right.clone(), &mut left_sources, BTreeMap::new())
        .expect_err("an unclassified struct must not merge by spelling and layout");
    assert!(err.to_string().contains("no semantic identity"), "unexpected error: {err}");

    let shared_source = SourceStateId::from_origin(
        "Shared",
        DeclarationOrigin::Dependency { app: "First".to_string(), name: "Shared".to_string(), kind: SymbolKind::State },
    );
    let physical = |family: &str| {
        BTreeMap::from([(
            "Shared".to_string(),
            AbiStructIdentity::Physical {
                source: shared_source.clone(),
                fields: vec![PhysicalFieldId::Generated(GeneratedFieldId::RouteFamilyDigest {
                    app: "First".to_string(),
                    family: family.to_string(),
                })],
            },
        )])
    };
    let err = merge_sil_abi_artifacts(left.clone(), right.clone(), &mut physical("one"), physical("two"))
        .expect_err("equal physical field layouts with different generated meaning must not merge");
    assert!(err.to_string().contains("distinct semantic identities"), "unexpected error: {err}");

    let shared = BTreeMap::from([("Shared".to_string(), source("First"))]);
    let merged = merge_sil_abi_artifacts(left, right, &mut left_sources, shared).expect("one source used by two contracts can merge");
    assert_eq!(merged.contracts.len(), 2);
    assert_eq!(merged.structs.len(), 1);
}
