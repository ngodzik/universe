//! The document sources the settings can name: a file of this machine, and
//! a page behind an MCP server's tools, called through universe's own
//! client so that no model is involved.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use universe_core::{
    DocumentRead, DocumentSource, DocumentWrite, ExternalTools, SourceError, SourceSpec, SourceTool,
};

/// How a server's tool is named among every server's: as the MCP client
/// offers it to models.
const SEPARATOR: &str = "__";

/// The source `spec` describes. `home` stands for `~` in a path; an MCP
/// page is reached through `external`.
///
/// # Errors
///
/// When `spec` is an MCP page and no MCP server is declared.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use universe_core::SourceSpec;
///
/// let spec = SourceSpec::File { path: "~/notes/rules.md".into() };
/// let source = universe_tools::open_source(&spec, Some(Path::new("/home/me")), None).unwrap();
/// assert_eq!(source.id(), "file:~/notes/rules.md");
/// ```
pub fn open_source(
    spec: &SourceSpec,
    home: Option<&std::path::Path>,
    external: Option<Arc<dyn ExternalTools>>,
) -> Result<Arc<dyn DocumentSource>, SourceError> {
    match spec {
        SourceSpec::File { path } => {
            let full = match (path.strip_prefix("~/"), home) {
                (Some(rest), Some(home)) => home.join(rest),
                _ => PathBuf::from(path),
            };
            Ok(Arc::new(FileSource {
                id: spec.id(),
                path: full,
            }))
        }
        SourceSpec::Mcp {
            server,
            read,
            write,
        } => {
            let external = external.ok_or_else(|| SourceError::NoServers(spec.id()))?;
            Ok(Arc::new(ToolSource {
                id: spec.id(),
                server: server.clone(),
                read: read.clone(),
                write: write.clone(),
                external,
            }))
        }
    }
}

/// A file of this machine, read and written whole.
#[derive(Debug)]
struct FileSource {
    id: String,
    path: PathBuf,
}

impl FileSource {
    fn failed(&self, e: &std::io::Error) -> SourceError {
        SourceError::File {
            path: self.path.display().to_string(),
            why: e.to_string(),
        }
    }
}

impl DocumentSource for FileSource {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn read(&self) -> DocumentRead<'_> {
        Box::pin(async move {
            tokio::fs::read_to_string(&self.path)
                .await
                .map_err(|e| self.failed(&e))
        })
    }

    fn write<'a>(&'a self, text: &'a str) -> Option<DocumentWrite<'a>> {
        Some(Box::pin(async move {
            tokio::fs::write(&self.path, text)
                .await
                .map_err(|e| self.failed(&e))
        }))
    }
}

/// A page an MCP server's tools read and maybe write.
struct ToolSource {
    id: String,
    server: String,
    read: SourceTool,
    write: Option<SourceTool>,
    external: Arc<dyn ExternalTools>,
}

/// Written by hand: the client has no use in it.
impl std::fmt::Debug for ToolSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolSource").field("id", &self.id).finish()
    }
}

impl ToolSource {
    /// Calls `tool` with `arguments`, as the client names it.
    async fn call(&self, tool: &str, arguments: &Value) -> Result<String, SourceError> {
        let name = format!("{}{SEPARATOR}{tool}", plain(&self.server));
        self.external
            .call(&name, &arguments.to_string())
            .await
            .map_err(|why| SourceError::Tool { tool: name, why })
    }
}

impl DocumentSource for ToolSource {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn read(&self) -> DocumentRead<'_> {
        Box::pin(async move {
            let arguments = Value::Object(self.read.arguments.clone());
            self.call(&self.read.tool, &arguments).await
        })
    }

    fn write<'a>(&'a self, text: &'a str) -> Option<DocumentWrite<'a>> {
        let tool = self.write.as_ref()?;
        Some(Box::pin(async move {
            let arguments = with_text(&Value::Object(tool.arguments.clone()), text);
            self.call(&tool.tool, &arguments).await.map(|_| ())
        }))
    }
}

/// `arguments` with `{text}` replaced by `text` wherever a string holds it.
fn with_text(arguments: &Value, text: &str) -> Value {
    match arguments {
        Value::String(s) => Value::String(s.replace("{text}", text)),
        Value::Array(items) => Value::Array(items.iter().map(|v| with_text(v, text)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), with_text(v, text)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A server's name as the MCP client keeps it: letters, digits, `-` and
/// `_`, anything else made `_`.
fn plain(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;
    use universe_core::{ExternalCall, ToolSpec};

    use super::*;

    /// A server that keeps one page, and the calls made to it.
    #[derive(Debug, Default)]
    struct Pages {
        page: Mutex<String>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl ExternalTools for Pages {
        fn specs(&self) -> Vec<ToolSpec> {
            Vec::new()
        }

        fn reads_only(&self, _: &str) -> Option<bool> {
            None
        }

        fn call<'a>(&'a self, name: &'a str, arguments: &'a str) -> ExternalCall<'a> {
            Box::pin(async move {
                let arguments: Value =
                    serde_json::from_str(arguments).map_err(|e| e.to_string())?;
                self.calls
                    .lock()
                    .unwrap()
                    .push((name.to_owned(), arguments.clone()));
                match name {
                    "my_docs__fetch" => Ok(self.page.lock().unwrap().clone()),
                    "my_docs__update" => {
                        *self.page.lock().unwrap() = arguments["body"]["text"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned();
                        Ok("updated".into())
                    }
                    _ => Err(format!("no tool {name}")),
                }
            })
        }

        fn status(&self) -> Vec<(String, String)> {
            Vec::new()
        }
    }

    fn page_spec(write: bool) -> SourceSpec {
        serde_json::from_value(json!({
            "kind": "mcp",
            "server": "my.docs",
            "read": { "tool": "fetch", "arguments": { "id": "p1" } },
            "write": write.then(|| json!({
                "tool": "update",
                "arguments": { "id": "p1", "body": { "text": "{text}", "note": "was: {text}?" } }
            })),
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn a_page_is_read_and_written_through_its_servers_tools() {
        let pages = Arc::new(Pages::default());
        *pages.page.lock().unwrap() = "Use tabs.".into();
        let external = Arc::clone(&pages) as Arc<dyn ExternalTools>;
        let source = open_source(&page_spec(true), None, Some(external)).unwrap();
        assert_eq!(source.read().await.unwrap(), "Use tabs.");
        source.write("Use spaces.").unwrap().await.unwrap();
        assert_eq!(source.read().await.unwrap(), "Use spaces.");
        let calls = pages.calls.lock().unwrap();
        // The server's name as the client keeps it; the text wherever
        // `{text}` was, the rest as written.
        assert_eq!(calls[0], ("my_docs__fetch".into(), json!({ "id": "p1" })));
        assert_eq!(
            calls[1].1,
            json!({ "id": "p1", "body": { "text": "Use spaces.", "note": "was: Use spaces.?" } })
        );
    }

    #[tokio::test]
    async fn a_page_without_a_write_tool_cannot_be_written_and_a_failed_call_says_its_tool() {
        let pages = Arc::new(Pages::default());
        let external = Arc::clone(&pages) as Arc<dyn ExternalTools>;
        let source = open_source(&page_spec(false), None, Some(external)).unwrap();
        assert!(source.write("x").is_none());

        let spec: SourceSpec = serde_json::from_value(json!({
            "kind": "mcp", "server": "my.docs", "read": { "tool": "gone" }
        }))
        .unwrap();
        let source = open_source(&spec, None, Some(pages as Arc<dyn ExternalTools>)).unwrap();
        let err = source.read().await.unwrap_err();
        assert_eq!(err.to_string(), "my_docs__gone: no tool my_docs__gone");
        assert_eq!(
            open_source(&spec, None, None).unwrap_err(),
            SourceError::NoServers(spec.id())
        );
    }

    #[tokio::test]
    async fn a_file_is_read_and_written_whole_with_the_home_folder_for_its_tilde() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("notes")).unwrap();
        let spec = SourceSpec::File {
            path: "~/notes/rules.md".into(),
        };
        let source = open_source(&spec, Some(home.path()), None).unwrap();
        assert!(matches!(source.read().await, Err(SourceError::File { .. })));
        source.write("Keep it short.").unwrap().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(home.path().join("notes/rules.md")).unwrap(),
            "Keep it short."
        );
        assert_eq!(source.read().await.unwrap(), "Keep it short.");
    }
}
