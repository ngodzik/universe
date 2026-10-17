//! Documents universe itself reads, and sometimes writes, without knowing
//! what holds them: a file, or a page behind an MCP server's tools. The
//! settings describe where one is; a crate that does the I/O reaches it.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

/// What reading a document comes to.
pub type DocumentRead<'a> = Pin<Box<dyn Future<Output = Result<String, SourceError>> + Send + 'a>>;

/// What writing a document comes to.
pub type DocumentWrite<'a> = Pin<Box<dyn Future<Output = Result<(), SourceError>> + Send + 'a>>;

/// A document universe reads in the background, such as rules a project's
/// models get, wherever it is kept.
pub trait DocumentSource: Send + Sync + fmt::Debug {
    /// A stable name, for the copy kept of it and for display, such as
    /// `file:~/notes/rules.md`.
    fn id(&self) -> String;

    /// The document's current text.
    fn read(&self) -> DocumentRead<'_>;

    /// Replaces the document's text; `None` when it cannot be written.
    fn write<'a>(&'a self, text: &'a str) -> Option<DocumentWrite<'a>>;
}

/// Why a document could not be read or written.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SourceError {
    /// The file could not be read or written.
    #[error("{path}: {why}")]
    File {
        /// The file.
        path: String,
        /// What the system said.
        why: String,
    },
    /// The tool it is read or written with failed, or is not there.
    #[error("{tool}: {why}")]
    Tool {
        /// The tool, as `<server>__<tool>`.
        tool: String,
        /// What the call came to.
        why: String,
    },
    /// The source needs MCP servers and none is declared.
    #[error("no MCP server is declared for {0}")]
    NoServers(String),
}

/// Where a document is, as the settings say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum SourceSpec {
    /// A file of this machine, read and written whole. `~` stands for the
    /// home folder.
    File {
        /// The file.
        path: String,
    },
    /// A page behind an MCP server's tools, called by universe through its
    /// own client.
    Mcp {
        /// The server, as named under `[mcp.<name>]`.
        server: String,
        /// The tool that returns the text.
        read: SourceTool,
        /// The tool that replaces it, `{text}` standing for the text in its
        /// arguments.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        write: Option<SourceTool>,
    },
}

/// A tool of an MCP server, with the arguments it is called with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceTool {
    /// The tool's name, as the server gives it.
    pub tool: String,
    /// Its arguments.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub arguments: Map<String, Value>,
}

impl SourceSpec {
    /// The source's stable name: `file:<path>` as written, or
    /// `mcp:<server>/<tool> <arguments>` for what reads it.
    #[must_use]
    pub fn id(&self) -> String {
        match self {
            Self::File { path } => format!("file:{path}"),
            Self::Mcp { server, read, .. } => {
                let arguments = Value::Object(read.arguments.clone());
                format!("mcp:{server}/{} {arguments}", read.tool)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_is_written_as_the_settings_write_it_and_named_by_what_reads_it() {
        let mcp: SourceSpec = serde_json::from_value(serde_json::json!({
            "kind": "mcp",
            "server": "docs",
            "read": { "tool": "fetch", "arguments": { "id": "page-1" } },
            "write": { "tool": "update", "arguments": { "id": "page-1", "text": "{text}" } }
        }))
        .unwrap();
        assert_eq!(mcp.id(), r#"mcp:docs/fetch {"id":"page-1"}"#);
        let file: SourceSpec =
            serde_json::from_value(serde_json::json!({ "kind": "file", "path": "~/rules.md" }))
                .unwrap();
        assert_eq!(file.id(), "file:~/rules.md");
        // A misspelt key is refused rather than read as nothing.
        assert!(
            serde_json::from_value::<SourceSpec>(
                serde_json::json!({ "kind": "file", "path": "a", "pth": "b" })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<SourceSpec>(serde_json::json!({ "kind": "web", "url": "x" }))
                .is_err()
        );
    }
}
