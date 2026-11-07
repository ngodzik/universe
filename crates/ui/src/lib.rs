//! The universe interface's state, whatever draws it.
//!
//! [`App`] holds everything on screen and decides what each key and click
//! does. It performs no I/O but reading the files it opens: it returns an
//! [`Effect`] for the loop that drives it to carry out through the
//! [`Host`], which is what makes it testable without a screen. A view reads
//! it and draws; [`keymap`] turns keys into actions.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod app;
pub mod backdrop;
pub mod clipboard;
mod command;
pub mod defaults;
mod highlight;
mod host;
pub mod input;
pub mod keymap;
pub mod reader;
mod search;
pub mod style;
pub mod tree;

pub use app::{App, DEFAULT_OPACITY, DefinitionChoice, Effect, LineEditor, MapView};
pub use command::HELP;
pub use defaults::Defaults;
pub use host::Host;
pub use search::search_matches;
