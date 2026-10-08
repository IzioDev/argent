//! Stable source identities and locations for one compilation graph.

use std::path::PathBuf;

use super::node::NodeId;
use crate::error::ArgentError;

/// Index into a frozen source set. It is only stable for that compilation.
#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SourceId(pub(crate) usize);

#[derive(Debug)]
pub(crate) struct SourceFile {
    pub(crate) id: SourceId,
    /// User-facing path; canonical identity is maintained separately by the loader.
    pub(crate) display_path: PathBuf,
    pub(crate) text: String,
}

/// Source locations are byte ranges into the original, unmodified source text.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[allow(dead_code)] // Generated origins acquire source-node causes in C02.
pub(crate) enum Origin {
    Authored { source: SourceId, start: usize, end: usize },
    Generated { cause: Option<NodeId> },
}

impl SourceFile {
    pub(crate) fn error_at(&self, byte_offset: usize, message: impl Into<String>) -> ArgentError {
        ArgentError::at_source(&self.display_path, &self.text, byte_offset, message)
    }
}

#[cfg(test)]
mod tests;
