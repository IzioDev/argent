//! Orders selected source apps after the whole import graph has been resolved.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::compiler::loader::{ModuleId, ResolvedModules};
use crate::error::{ArgentError, Result};

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct SourceApp {
    pub(crate) source: PathBuf,
    pub(crate) app: String,
}

#[derive(Debug)]
pub(crate) struct AppBuildUnit {
    pub(crate) source_app: SourceApp,
    pub(crate) root: ModuleId,
    pub(crate) dependencies: Vec<SourceApp>,
}

#[derive(Debug)]
pub(crate) struct AppGraphPlan<'src> {
    program: ResolvedModules<'src>,
    order: Vec<AppBuildUnit>,
}

#[derive(Clone, Copy)]
enum Visit {
    Active(usize),
    Complete,
}

struct AppGraphPlanner<'a, 'src> {
    program: &'a ResolvedModules<'src>,
    app_sources: BTreeMap<String, PathBuf>,
    visits: BTreeMap<SourceApp, Visit>,
    stack: Vec<SourceApp>,
    order: Vec<AppBuildUnit>,
}

impl<'src> AppGraphPlan<'src> {
    pub(crate) fn new(program: ResolvedModules<'src>, app: &str) -> Result<Self> {
        let root = SourceApp { source: program.root_path().to_path_buf(), app: app.to_string() };
        let order = {
            let mut planner = AppGraphPlanner {
                program: &program,
                app_sources: BTreeMap::new(),
                visits: BTreeMap::new(),
                stack: Vec::new(),
                order: Vec::new(),
            };
            planner.visit(root)?;
            planner.order
        };
        Ok(Self { program, order })
    }

    pub(crate) fn units(&self) -> &[AppBuildUnit] {
        &self.order
    }

    pub(crate) fn program_for(&self, unit: &AppBuildUnit) -> ResolvedModules<'src> {
        self.program.with_root(unit.root)
    }
}

impl AppGraphPlanner<'_, '_> {
    fn visit(&mut self, app: SourceApp) -> Result<()> {
        if let Some(previous) = self.app_sources.insert(app.app.clone(), app.source.clone())
            && previous != app.source
        {
            return Err(ArgentError::new(format!(
                "app `{}` is imported from both `{}` and `{}`",
                app.app,
                previous.display(),
                app.source.display()
            )));
        }
        match self.visits.get(&app).copied() {
            Some(Visit::Complete) => return Ok(()),
            Some(Visit::Active(start)) => {
                let cycle = self.stack[start..]
                    .iter()
                    .chain(std::iter::once(&app))
                    .map(|app| app.app.as_str())
                    .collect::<Vec<_>>()
                    .join(" -> ");
                return Err(ArgentError::new(format!("app import cycle: {cycle}")));
            }
            None => {}
        }

        let root = self
            .program
            .module_id_for_path(&app.source)
            .ok_or_else(|| ArgentError::at(&app.source, "app source is outside the discovered import graph"))?;
        self.visits.insert(app.clone(), Visit::Active(self.stack.len()));
        self.stack.push(app.clone());

        let view = self.program.with_root(root);
        let Some(selected_app) = view.root_app(Some(&app.app))? else {
            return Err(ArgentError::at(&app.source, format!("source does not declare app `{}`", app.app)));
        };
        let dependencies = view
            .referenced_app_members(selected_app)?
            .into_iter()
            .map(|member| {
                let (source, declaration) = view.app_source(member.app);
                SourceApp { source: source.to_path_buf(), app: declaration.name.clone() }
            })
            .collect::<BTreeSet<_>>();
        for dependency in &dependencies {
            self.visit(dependency.clone())?;
        }

        self.stack.pop();
        self.visits.insert(app.clone(), Visit::Complete);
        self.order.push(AppBuildUnit { source_app: app, root, dependencies: dependencies.into_iter().collect() });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
