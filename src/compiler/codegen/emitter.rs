//! Emits one selected Argent app as Sil, artifact, and manifest files.
//! The build path compiles retained contract ASTs and formats those same nodes.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use silverscript_lang::ast::format_contract_ast;

use super::abi::sil_abi_artifact;
use super::compile::CompiledActors;
use super::manifest_path;
use crate::artifact::*;
use crate::compiler::loader::ResolvedModules;
use crate::compiler::model::{AppCompilationContext, InteractionSource, ResolvedSuccessor};
use crate::compiler::syntax::body::RouteArity;
use crate::compiler::syntax::word;
use crate::compiler::syntax::*;
use crate::error::{ArgentError, Result};

use super::sil::*;

use super::artifact;
#[cfg(test)]
mod leader_delegate_tests;
#[cfg(test)]
mod tests;

pub(crate) fn emit_build_model(program: &ResolvedModules, model: &AppCompilationContext<'_>, out_dir: impl AsRef<Path>) -> Result<()> {
    let out_dir = out_dir.as_ref();
    let sil_dir = out_dir.join("sil");
    let mut contracts = BTreeMap::new();
    let mut formatted_sil = BTreeMap::new();
    for (id, actor) in model.app_actors.iter_with_ids() {
        let lowerer = ContractLowerer::new(id, model)?;
        let contract = lowerer.annotate_actor(lowerer.lower_actor()?)?;
        formatted_sil.insert(actor.to_string(), format_contract_ast(&contract.contract));
        contracts.insert(id, contract);
    }
    let compiled_actors = CompiledActors::from_contracts(contracts, &model.types.display_names)?;
    let manifest = emit_manifest(program, model)?;
    let artifact = emit_artifact_json(program, model, &compiled_actors)?;

    if sil_dir.exists() {
        fs::remove_dir_all(&sil_dir).map_err(|err| ArgentError::at(&sil_dir, err.to_string()))?;
    }
    fs::create_dir_all(&sil_dir).map_err(|err| ArgentError::at(&sil_dir, err.to_string()))?;
    for (actor, sil) in &formatted_sil {
        let path = sil_dir.join(format!("{actor}.sil"));
        fs::write(&path, sil).map_err(|err| ArgentError::at(path, err.to_string()))?;
    }

    fs::write(out_dir.join("manifest.json"), manifest)
        .map_err(|err| ArgentError::at(out_dir.join("manifest.json"), err.to_string()))?;

    fs::write(out_dir.join("artifact.json"), artifact)
        .map_err(|err| ArgentError::at(out_dir.join("artifact.json"), err.to_string()))?;
    Ok(())
}

fn emit_manifest(program: &ResolvedModules, model: &AppCompilationContext<'_>) -> Result<String> {
    let mut actors = Vec::new();
    for (actor_id, _) in model.app_actors.iter_with_ids() {
        let actor = model.actor_by_decl(actor_id)?;
        let mut entries = Vec::new();
        for (entry_idx, entry) in actor.entries.iter().enumerate() {
            let entry_model = model.entry_model_by_id(crate::compiler::syntax::node::EntryId { actor: actor_id, index: entry_idx })?;
            let emits = match &entry_model.source().emits {
                EmitSpec::None => json!({ "kind": "none" }),
                EmitSpec::Outputs(_) => {
                    let outputs = entry_model
                        .current()
                        .outputs()
                        .iter()
                        .map(|interaction| {
                            let InteractionSource::CurrentOutput(output) = interaction.source() else {
                                unreachable!("current entry outputs are emits outputs");
                            };
                            let mut value = json!({
                                "name": &output.name,
                                "auth_index": artifact::fixed_interaction_index(interaction.location(), 0),
                                "actors": &output.actors,
                            });
                            if let Some((minimum, maximum)) = interaction.cardinality().range_bounds() {
                                value["cardinality"] = json!({ "kind": "range", "minimum": minimum, "maximum": maximum });
                            }
                            value
                        })
                        .collect::<Vec<_>>();
                    json!({ "kind": "outputs", "outputs": outputs })
                }
            };
            let mut consumes = Vec::new();
            for interaction in entry_model.current().inputs() {
                let InteractionSource::Consume(consume) = interaction.source() else {
                    unreachable!("current entry inputs are consumes");
                };
                let mut value = json!({ "name": &consume.name, "actor": &consume.actor });
                if let Some((minimum, maximum)) = interaction.cardinality().range_bounds() {
                    value["cardinality"] = json!({ "kind": "range", "minimum": minimum, "maximum": maximum });
                }
                consumes.push(value);
            }
            let mut routes = Vec::new();
            for route in entry_model.routes() {
                let successor = match &route.successor {
                    ResolvedSuccessor::ExactSelf => json!({ "kind": "exact_self" }),
                    ResolvedSuccessor::Constructed { actor, arity, .. } => {
                        let (_, state) = model.resolution.route_texts(entry_model.id, route.id)?;
                        let mut value = json!({
                            "kind": "constructed",
                            "actor": actor.display(entry, entry_model.id, route.id, model)?,
                            "state": compact_expr(state),
                        });
                        if *arity == RouteArity::Many {
                            value["arity"] = json!("many");
                        }
                        value
                    }
                };
                routes.push(json!({ "output": &route.output, "successor": successor }));
            }
            entries.push(json!({
                "name": &entry.name,
                "kind": match entry.kind {
                    EntryKind::Leader => word::LEADER,
                    EntryKind::Delegate => word::DELEGATE,
                },
                "emits": emits,
                "consumes": consumes,
                "routes": routes,
            }));
        }
        actors.push(json!({
            "name": &actor.name,
            "state": &actor.state,
            "sil": format!("sil/{}.sil", actor.name),
            "entries": entries,
        }));
    }
    let manifest = json!({
        "app": &model.app_name,
        "root": manifest_path(program.root_path()),
        "modules": program.module_paths().map(manifest_path).collect::<Vec<_>>(),
        "templates": model.app_actors.iter().map(|actor| json!({
            "actor": actor,
            "symbol": hidden_template_name(actor),
            "hash": Value::Null,
        })).collect::<Vec<_>>(),
        "actors": actors,
    });
    let mut out = serde_json::to_string_pretty(&manifest).map_err(|err| ArgentError::new(err.to_string()))?;
    out.push('\n');
    Ok(out)
}

fn emit_artifact_json(
    program: &ResolvedModules,
    model: &AppCompilationContext<'_>,
    compiled_actors: &CompiledActors<'_>,
) -> Result<String> {
    let artifact = project_compiled_artifact(program, model, compiled_actors)?;
    let mut json = silverscript_abi::to_pretty_json(&artifact).map_err(|err| ArgentError::new(err.to_string()))?;
    json.push('\n');
    Ok(json)
}

fn project_compiled_artifact(
    program: &ResolvedModules,
    model: &AppCompilationContext<'_>,
    compiled_actors: &CompiledActors<'_>,
) -> Result<Artifact> {
    let sil_abi = sil_abi_artifact(model, compiled_actors)?;
    let draft = artifact::TemplatePlanDraft::new(model, &sil_abi.contracts)?;
    let requests = draft.context_requests(model)?;
    let contexts = requests
        .iter()
        .map(|(actor, request)| Ok((*actor, compiled_actors.compile_context(*actor, &request.args)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    artifact::emit_artifact_compiled(program, model, sil_abi, draft, &requests, &contexts)
}
