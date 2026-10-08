//! Export fixed point, namespace traversal, and ambiguity rules.

use super::*;
use crate::compiler::naming::is_identifier;

#[derive(Debug, Clone)]
pub(super) struct NamespaceTable {
    pub(super) exports: Vec<BTreeMap<String, ResolvedName>>,
}

impl NamespaceTable {
    /// Collects names available in each module and rejects names that refer to different things.
    pub(super) fn new(modules: &SourceStorage<'_>, imports: &[Vec<ResolvedImport>]) -> Result<Self> {
        let mut candidates = vec![BTreeMap::<String, BTreeSet<ResolvedName>>::new(); modules.module_count()];

        for module_index in 0..modules.module_count() {
            let module = modules.module(ModuleId::new(module_index));
            let module_id = ModuleId::new(module_index);
            let mut insert_declaration = |name: &str, kind, index| {
                candidates[module_index]
                    .entry(name.to_string())
                    .or_default()
                    .insert(ResolvedName::Declaration(DeclId::new(module_id, kind, index)));
            };
            for (index, item) in module.consts.iter().enumerate() {
                insert_declaration(&item.name, SymbolKind::Const, index);
            }
            for (index, item) in module.states.iter().enumerate() {
                insert_declaration(&item.name, SymbolKind::State, index);
            }
            for (index, item) in module.functions.iter().enumerate() {
                insert_declaration(&item.name, SymbolKind::Function, index);
            }
            for (index, item) in module.actors.iter().enumerate() {
                insert_declaration(&item.name, SymbolKind::Actor, index);
            }
            for (index, item) in module.actor_enums.iter().enumerate() {
                insert_declaration(&item.name, SymbolKind::ActorEnum, index);
            }
            for (index, item) in module.apps.iter().enumerate() {
                insert_declaration(&item.name, SymbolKind::App, index);
            }

            let mut aliases = BTreeSet::new();
            for import in &imports[module_index] {
                if let Some(alias) = &import.alias {
                    if !aliases.insert(alias.as_str()) {
                        return Err(ArgentError::at(&module.path, format!("module alias `{alias}` is imported more than once")));
                    }
                    candidates[module_index].entry(alias.clone()).or_default().insert(ResolvedName::Module(import.target));
                }
            }
        }

        // Repeat until imports of imports have also been included.
        loop {
            let mut next_candidates = candidates.clone();
            for (module_index, module_imports) in imports.iter().enumerate() {
                // For each unaliased import, add the imported module's resolved names to this module.
                for import in module_imports.iter().filter(|import| import.alias.is_none()) {
                    for (name, possible_targets) in &candidates[import.target.index()] {
                        next_candidates[module_index].entry(name.clone()).or_default().extend(possible_targets);
                    }
                }
            }
            if next_candidates == candidates {
                break;
            }
            candidates = next_candidates;
        }

        let mut exports = Vec::with_capacity(candidates.len());
        for (module_index, module_candidates) in candidates.into_iter().enumerate() {
            let mut module_exports = BTreeMap::new();
            for (name, possible_targets) in module_candidates {
                let Some(target) = possible_targets.iter().next().copied() else {
                    continue;
                };
                if possible_targets.len() > 1 {
                    return Err(ArgentError::at(
                        &modules.module(ModuleId::new(module_index)).path,
                        format!("ambiguous export `{name}` in module namespace"),
                    ));
                }
                module_exports.insert(name, target);
            }
            exports.push(module_exports);
        }
        Ok(Self { exports })
    }

    /// Follows a name such as `library::app::Actor` from the module where it is used.
    pub(super) fn bind_export_path(
        &self,
        program: &ResolvedModules,
        from_module: ModuleId,
        segments: &[&str],
        reference: &str,
    ) -> Result<ResolvedName> {
        let mut current_module = from_module;
        for (index, segment) in segments.iter().enumerate() {
            if !is_identifier(segment) {
                return Err(ArgentError::at(&program.module(from_module).path, format!("invalid qualified reference `{reference}`")));
            }
            let target = self.exports[current_module.index()].get(*segment).copied().ok_or_else(|| {
                ArgentError::at(&program.module(from_module).path, format!("unknown export `{segment}` while resolving `{reference}`"))
            })?;
            if index + 1 == segments.len() {
                return Ok(target);
            }
            match target {
                ResolvedName::Module(imported_module) => current_module = imported_module,
                ResolvedName::Declaration(app) if app.kind == SymbolKind::App && index + 2 == segments.len() => {
                    return self.resolve_app_member(program, from_module, app, segments[index + 1], reference);
                }
                ResolvedName::Declaration(_) | ResolvedName::AppMember(_) => {
                    return Err(ArgentError::at(
                        &program.module(from_module).path,
                        format!("export `{segment}` is not a namespace while resolving `{reference}`"),
                    ));
                }
            }
        }
        Err(ArgentError::at(&program.module(from_module).path, format!("invalid qualified reference `{reference}`")))
    }

    fn resolve_app_member(
        &self,
        program: &ResolvedModules,
        from_module: ModuleId,
        app: DeclId,
        actor_name: &str,
        reference: &str,
    ) -> Result<ResolvedName> {
        let ResolvedDeclaration::App(app_decl) = program.declaration(app) else {
            unreachable!("app declaration ID resolves to an app");
        };
        let mut matching_actor = None;
        for actor_reference in &app_decl.actors {
            let actor_segments = actor_reference.split("::").collect::<Vec<_>>();
            let ResolvedName::Declaration(actor) = self.bind_export_path(program, app.module, &actor_segments, actor_reference)?
            else {
                return Err(ArgentError::at(
                    &program.module(app.module).path,
                    format!("app `{}` member `{actor_reference}` does not name a local actor", app_decl.name),
                ));
            };
            if actor.kind != SymbolKind::Actor {
                continue;
            }
            let ResolvedDeclaration::Actor(actor_decl) = program.declaration(actor) else {
                unreachable!("actor declaration ID resolves to an actor");
            };
            if actor_decl.name == actor_name && matching_actor.replace(actor).is_some() {
                return Err(ArgentError::at(
                    &program.module(app.module).path,
                    format!("app `{}` exports actor name `{actor_name}` more than once", app_decl.name),
                ));
            }
        }
        matching_actor.map(|actor| ResolvedName::AppMember(AppMember { app, actor })).ok_or_else(|| {
            ArgentError::at(
                &program.module(from_module).path,
                format!("app `{}` has no actor member `{actor_name}` while resolving `{reference}`", app_decl.name),
            )
        })
    }

    pub(super) fn wrong_kind(
        &self,
        program: &ResolvedModules,
        module: ModuleId,
        reference: &str,
        expected: &[SymbolKind],
        actual: SymbolKind,
    ) -> ArgentError {
        let expected = expected.iter().map(|kind| kind.description()).collect::<Vec<_>>().join(" or ");
        ArgentError::at(
            &program.module(module).path,
            format!("reference `{reference}` names {}, expected {expected}", actual.description()),
        )
    }
}
