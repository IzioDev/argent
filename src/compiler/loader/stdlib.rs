//! Resolves standard Argent modules bundled with the compiler.
//!
//! Built-in source uses the same syntax parser as user modules.

use crate::error::{ArgentError, Result};

pub const CORE_MODULE: &str = "std::core";

pub fn is_standard_module(path: &str) -> bool {
    path.starts_with("std::")
}

pub fn standard_source(path: &str) -> Result<&'static str> {
    match path {
        CORE_MODULE => Ok(include_str!("../../../std/core.ag")),
        _ => Err(ArgentError::new(format!("unknown Argent standard module `{path}`"))),
    }
}
