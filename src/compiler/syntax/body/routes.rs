//! Extracts body routes and verifies that every `become` is terminal.

use super::{EntryRoute, EntrySuccessor, RouteArity, RouteId};
use crate::compiler::syntax::source::SourceFile;
use crate::compiler::syntax::{AuthoredEntryRoute, AuthoredEntryStatement, AuthoredSuccessor};
use crate::error::Result;

#[derive(Debug, Clone)]
pub struct RouteAnalysis {
    pub routes: Vec<EntryRoute>,
    pub terminal_route_sets: Vec<Vec<RouteId>>,
}

/// Analyze authored control flow and project only the route facts needed by the model.
pub(crate) fn analyze_authored_entry_routes(
    statements: &[AuthoredEntryStatement<'_>],
    file: &SourceFile,
    end_offset: usize,
) -> Result<RouteAnalysis> {
    let info = analyze_authored_sequence(statements, file, end_offset)?;
    let routes = info.terminal_route_sets.iter().flatten().cloned().collect();
    let terminal_route_sets = info.terminal_route_sets.iter().map(|routes| routes.iter().map(|route| route.id).collect()).collect();
    Ok(RouteAnalysis { routes, terminal_route_sets })
}

impl AuthoredEntryRoute<'_> {
    fn semantic(&self) -> EntryRoute {
        let successor = match &self.successor {
            AuthoredSuccessor::SelfRef { .. } => EntrySuccessor::ExactSelf,
            AuthoredSuccessor::Constructed { many, .. } => {
                EntrySuccessor::Constructed { arity: if *many { RouteArity::Many } else { RouteArity::One } }
            }
        };
        EntryRoute { id: self.id, output: self.output.segments.join("::"), successor }
    }
}

impl AuthoredEntryStatement<'_> {
    fn start(&self) -> usize {
        match self {
            Self::Block { span, .. } | Self::If { span, .. } | Self::Become { span, .. } | Self::ForeignBecome { span, .. } => {
                span.start()
            }
            Self::Sil(statement) => statement.span().start(),
        }
    }

    fn terminal_result(&self, file: &SourceFile) -> Result<TerminalResult> {
        match self {
            Self::If { then_branch, else_branch, .. } => {
                let then_result = then_branch.terminal_result(file)?;
                let else_result =
                    if let Some(else_branch) = else_branch { else_branch.terminal_result(file)? } else { TerminalResult::empty() };
                let contains_become = then_result.info.contains_become || else_result.info.contains_become;
                let all_paths_terminal = then_result.info.all_paths_terminal && else_result.info.all_paths_terminal;
                let mut terminal_route_sets = then_result.terminal_route_sets;
                terminal_route_sets.extend(else_result.terminal_route_sets);
                Ok(TerminalResult { info: TerminalInfo { contains_become, all_paths_terminal }, terminal_route_sets })
            }
            Self::Become { routes, .. } => Ok(TerminalInfo::terminal(routes.iter().map(AuthoredEntryRoute::semantic).collect())),
            Self::Block { statements, span } => analyze_authored_sequence(statements, file, span.end().saturating_sub(1)),
            Self::ForeignBecome { .. } | Self::Sil(_) => Ok(TerminalResult::empty()),
        }
    }
}

fn analyze_authored_sequence(
    statements: &[AuthoredEntryStatement<'_>],
    file: &SourceFile,
    end_offset: usize,
) -> Result<TerminalResult> {
    let mut result = TerminalResult::empty();
    for (index, statement) in statements.iter().enumerate() {
        let statement_result = statement.terminal_result(file)?;
        result.info.contains_become |= statement_result.info.contains_become;
        result.terminal_route_sets.extend(statement_result.terminal_route_sets);

        if statement_result.info.all_paths_terminal {
            if let Some(next) = statements.get(index + 1) {
                return Err(
                    file.error_at(next.start(), "`become` must be terminal; move following code into an explicit `else` branch")
                );
            }
            result.info.all_paths_terminal = true;
            break;
        }
        if statement_result.info.contains_become {
            let next_offset = statements.get(index + 1).map_or(end_offset, AuthoredEntryStatement::start);
            return Err(
                file.error_at(next_offset, "conditional `become` must be terminal on every branch; add an explicit `else` branch")
            );
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy)]
struct TerminalInfo {
    contains_become: bool,
    all_paths_terminal: bool,
}

impl TerminalInfo {
    fn empty() -> Self {
        Self { contains_become: false, all_paths_terminal: false }
    }

    fn terminal(routes: Vec<EntryRoute>) -> TerminalResult {
        TerminalResult { info: Self { contains_become: true, all_paths_terminal: true }, terminal_route_sets: vec![routes] }
    }
}

#[derive(Debug, Clone)]
struct TerminalResult {
    info: TerminalInfo,
    terminal_route_sets: Vec<Vec<EntryRoute>>,
}

impl TerminalResult {
    fn empty() -> Self {
        Self { info: TerminalInfo::empty(), terminal_route_sets: Vec::new() }
    }
}
