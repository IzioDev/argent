//! Generates Sil contracts and portable build products from a validated model.
//!
//! The emitter formats and compiles retained contract ASTs.

use std::path::Path;

mod abi;
mod artifact;
mod compile;
mod emitter;
mod sil;

fn manifest_path(path: &Path) -> String {
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(relative) = path.strip_prefix(&cwd)
    {
        return display_path(relative);
    }
    display_path(path)
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) use emitter::emit_build_model;
