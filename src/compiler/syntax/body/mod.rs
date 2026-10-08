//! Route facts projected from combined source syntax for semantic planning.

pub mod routes;

/// Stable identity for one source route within an entry body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RouteId(pub(crate) usize);

/// A successor's semantic shape; authored expressions remain in the source AST.
#[derive(Debug, Clone)]
pub(crate) enum EntrySuccessor {
    ExactSelf,
    Constructed { arity: RouteArity },
}

/// Whether a constructed `become` successor represents one actor or an actor array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteArity {
    One,
    Many,
}

/// One route in a `become` statement.
#[derive(Debug, Clone)]
pub(crate) struct EntryRoute {
    pub(crate) id: RouteId,
    pub(crate) output: String,
    pub(crate) successor: EntrySuccessor,
}
