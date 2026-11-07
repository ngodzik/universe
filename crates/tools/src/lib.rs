//! Deterministic tools for universe.
//!
//! Everything here gives the same answer for the same input and asks no
//! model: where a name is defined or used, as the project's language
//! servers say or as `git grep` reads it, and what git says of each file.
//! Nothing here writes to the project.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod definitions;
mod git;
mod lookup;
mod lsp;

pub use definitions::{Definition, definitions, uses};
pub use git::file_status;
pub use lookup::{Found, Lookup, find};
pub use lsp::{Location, LspError, Servers};
