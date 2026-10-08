//! Source-backed actor and entry lookup.

use std::collections::BTreeMap;

use crate::compiler::resolve::ResolvedModules;
use crate::compiler::syntax::body::EntrySuccessor;
use crate::compiler::syntax::node::{DeclId, EntryId};
use crate::compiler::syntax::{ActorDecl, FunctionDecl};
use crate::error::{ArgentError, Result};

use super::{ConstResolver, EntryModel, StaticActorId, TypeTable};

#[cfg(test)]
mod tests;

/// An actor's bound entry models, with declarations iterated in source order.
#[derive(Debug)]
pub(crate) struct ActorModel<'a> {
    pub(crate) id: DeclId,
    pub(crate) state: DeclId,
    source: &'a ActorDecl,
    entries: BTreeMap<EntryId, EntryModel<'a>>,
}

impl<'a> ActorModel<'a> {
    /// Attach the bound expression sites for every constructed entry successor.
    pub(super) fn attach_bound_routes(
        &mut self,
        types: &TypeTable,
        resolution: &ResolvedModules<'_>,
        actor_ids: &BTreeMap<String, StaticActorId>,
    ) -> Result<()> {
        for entry in self.entries.values_mut() {
            entry.bind_actor_targets(resolution, types, actor_ids)?;
            entry.bind_selectors(self.source, resolution, types, actor_ids)?;
            entry.bind_foreign_routes(resolution)?;
            for route in &entry.source().routes {
                if !matches!(route.successor, EntrySuccessor::Constructed { .. }) {
                    continue;
                }
                let value = types.route_values.get(&(entry.id, route.id)).ok_or_else(|| {
                    ArgentError::new(format!(
                        "entry `{}::{}` has an unbound constructed successor",
                        self.source.name,
                        entry.source().name
                    ))
                })?;
                entry.attach_bound_route(route.id, *value);
            }
        }
        Ok(())
    }

    /// Build the function index and entry models for one source actor.
    pub(crate) fn build(id: DeclId, state: DeclId, source: &'a ActorDecl, const_resolver: &ConstResolver) -> Result<Self> {
        let mut functions_by_name = BTreeMap::new();
        for function in &source.functions {
            if functions_by_name.insert(function.name.as_str(), function).is_some() {
                let name = &function.name;
                return Err(ArgentError::new(format!("actor `{}` declares function `{name}` more than once", source.name)));
            }
        }

        let mut entries_by_name = BTreeMap::new();
        let mut entries = BTreeMap::new();
        for (index, entry) in source.entries.iter().enumerate() {
            if functions_by_name.contains_key(entry.name.as_str()) {
                return Err(ArgentError::new(format!(
                    "actor `{}` declares both a function and an entry named `{}`",
                    source.name, entry.name
                )));
            }
            let entry_id = EntryId { actor: id, index };
            let model = EntryModel::build(entry_id, source, entry, const_resolver)?;
            if entries_by_name.insert(entry.name.as_str(), entry_id).is_some() {
                let name = &entry.name;
                return Err(ArgentError::new(format!("actor `{}` declares entry `{name}` more than once", source.name)));
            }
            entries.insert(entry_id, model);
        }
        Ok(Self { id, state, source, entries })
    }

    /// Return the source actor declaration.
    pub(crate) fn source(&self) -> &'a ActorDecl {
        self.source
    }

    /// Iterate actor functions in source declaration order.
    pub(crate) fn functions(&self) -> impl Iterator<Item = &'a FunctionDecl> {
        self.source.functions.iter()
    }

    /// Iterate entry models in source declaration order.
    pub(crate) fn entries(&self) -> impl Iterator<Item = &EntryModel<'a>> {
        self.source
            .entries
            .iter()
            .enumerate()
            .map(|(index, _)| self.entries.get(&EntryId { actor: self.id, index }).expect("source entry has an entry model"))
    }

    /// Look up a selected entry by its bound declaration position.
    pub(crate) fn entry_by_id(&self, id: EntryId) -> Option<&EntryModel<'a>> {
        self.entries.get(&id)
    }
}
