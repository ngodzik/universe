//! Rules a project's models get from documents the person keeps outside its
//! repository, as `[[rules]]` in the settings name them.
//!
//! A copy of each is kept on disk and is what the models get: a request
//! never waits on the document's source, which is read again in the
//! background once the copy is older than its period. The copy from the
//! last session serves at once.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use universe_core::{DocumentSource, ExternalTools, SourceError, SourceSpec};

/// How long a copy serves before its source is read again, unless the
/// entry says.
const PERIOD: Duration = Duration::from_secs(600);

/// How long a source that failed is left alone before it is tried again.
const AFTER_FAILURE: Duration = Duration::from_secs(60);

/// How many more times a tool that failed is called, and how long apart.
const TOOL_RETRIES: usize = 4;
const TOOL_RETRY_AFTER: Duration = Duration::from_secs(5);

/// The most of one document a request carries: it goes with every call.
const MAX_BYTES: usize = 32_000;

/// Where the copies are kept: beside the other caches of the settings'
/// folder.
#[must_use]
pub fn cache() -> Option<PathBuf> {
    crate::Defaults::path().map(|p| p.with_file_name("cache").join("rules"))
}

/// One `[[rules]]` entry of the settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesEntry {
    /// The project's folder; `~` stands for the home folder.
    pub project: String,
    /// The files the rules are for, as a glob such as `backend/**`; every
    /// file when unsaid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<String>,
    /// How old the copy may grow, in seconds, before the source is read
    /// again; ten minutes when unsaid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_secs: Option<u64>,
    /// Where the document is.
    pub source: SourceSpec,
}

/// The rules of one project, each with its copy.
#[derive(Debug, Clone, Default)]
pub struct OutsideRules {
    kept: Vec<Arc<Kept>>,
}

#[derive(Debug)]
struct Kept {
    entry: RulesEntry,
    /// The source, or why it cannot be reached.
    source: Result<Arc<dyn DocumentSource>, String>,
    copy: PathBuf,
    every: Duration,
    /// A read runs: another waits for it to end.
    reading: AtomicBool,
    /// When the source last failed, and why.
    failed: Mutex<Option<(Instant, String)>>,
}

impl OutsideRules {
    /// The rules `entries` give the project at `root`, their copies kept
    /// under `cache`. `home` stands for `~`; MCP pages are read through
    /// `external`.
    #[must_use]
    pub fn for_project(
        entries: &[RulesEntry],
        root: &Path,
        home: Option<&Path>,
        cache: Option<&Path>,
        external: Option<&Arc<dyn ExternalTools>>,
    ) -> Self {
        let Some(cache) = cache else {
            return Self::default();
        };
        let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => a == b,
        };
        let kept = entries
            .iter()
            .filter(|e| same(&crate::workspaces::expand(&e.project, home), root))
            .map(|entry| {
                let source = universe_tools::open_source(&entry.source, home, external.cloned())
                    .map_err(|e| e.to_string());
                Arc::new(Kept {
                    copy: cache.join(format!("{:016x}.md", fnv1a(&entry.source.id()))),
                    every: entry.refresh_secs.map_or(PERIOD, Duration::from_secs),
                    entry: entry.clone(),
                    source,
                    reading: AtomicBool::new(false),
                    failed: Mutex::new(None),
                })
            })
            .collect();
        Self { kept }
    }

    /// Whether the project has none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kept.is_empty()
    }

    /// The rules as the models get them, from the copies; `None` while
    /// none is kept yet.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        let parts: Vec<String> = self
            .kept
            .iter()
            .filter_map(|kept| {
                let body = std::fs::read_to_string(&kept.copy).ok()?;
                let body = body.trim();
                (!body.is_empty()).then(|| kept.section(body))
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    /// Reads again, in the background, each source whose copy is missing
    /// or older than its period, unless it is being read or failed a
    /// moment ago. Needs a Tokio runtime.
    pub fn refresh_due(&self) {
        for kept in &self.kept {
            if kept.is_due(SystemTime::now()) && !kept.reading.swap(true, Ordering::SeqCst) {
                let kept = Arc::clone(kept);
                tokio::spawn(async move {
                    kept.refresh().await;
                    kept.reading.store(false, Ordering::SeqCst);
                });
            }
        }
    }

    /// Each document, how old its copy is, and what went wrong last.
    #[must_use]
    pub fn status(&self) -> Vec<String> {
        self.kept
            .iter()
            .map(|kept| {
                let mut line = kept.entry.source.id();
                if let Some(only) = &kept.entry.only {
                    line.push_str(&format!(", for {only}"));
                }
                let age = std::fs::metadata(&kept.copy)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok());
                line.push_str(&match age {
                    Some(age) => format!(": read {} ago", minutes(age)),
                    None => ": not read yet".to_owned(),
                });
                if let Err(why) = &kept.source {
                    line.push_str(&format!("; cannot be reached: {why}"));
                } else if let Ok(failed) = kept.failed.lock()
                    && let Some((_, why)) = &*failed
                {
                    line.push_str(&format!("; last read failed: {why}"));
                }
                line
            })
            .collect()
    }
}

impl Kept {
    fn is_due(&self, now: SystemTime) -> bool {
        if self.source.is_err() {
            return false;
        }
        if let Ok(failed) = self.failed.lock()
            && let Some((at, _)) = &*failed
            && at.elapsed() < AFTER_FAILURE
        {
            return false;
        }
        std::fs::metadata(&self.copy)
            .and_then(|m| m.modified())
            .map_or(true, |at| {
                now.duration_since(at).unwrap_or_default() >= self.every
            })
    }

    async fn refresh(&self) {
        let Ok(source) = &self.source else {
            return;
        };
        let mut read = source.read().await;
        // A server may still be connecting, at startup most of all: its
        // tool is tried a few times before the read counts as failed.
        for _ in 0..TOOL_RETRIES {
            if !matches!(read, Err(SourceError::Tool { .. })) {
                break;
            }
            tokio::time::sleep(TOOL_RETRY_AFTER).await;
            read = source.read().await;
        }
        let outcome = match read {
            Ok(text) => keep(&self.copy, &text).map_err(|e| format!("keeping its copy: {e}")),
            Err(e) => Err(e.to_string()),
        };
        if let Ok(mut failed) = self.failed.lock() {
            *failed = outcome.err().map(|why| (Instant::now(), why));
        }
    }

    /// One document's part of the models' instructions: where it is kept,
    /// what it is for, how to change it, and its text.
    fn section(&self, body: &str) -> String {
        let mut header = format!("## {}", self.entry.source.id());
        if let Some(only) = &self.entry.only {
            header.push_str(&format!(" (only for files matching: {only})"));
        }
        let change = match &self.entry.source {
            SourceSpec::File { path } => format!("They are kept in the file {path}."),
            SourceSpec::Mcp {
                server,
                write: Some(write),
                ..
            } => format!(
                "To change them when the person asks, call `{server}__{}` with {}, `{{text}}` \
                 standing for the whole new text.",
                write.tool,
                serde_json::Value::Object(write.arguments.clone())
            ),
            SourceSpec::Mcp { server, read, .. } => format!(
                "They are read with `{server}__{}`; when the person asks to change them, use \
                 the server's tool that edits that page.",
                read.tool
            ),
        };
        format!("{header}\n{change}\n{}", cut(body))
    }
}

/// Writes `text` as the copy at `path`, whole or not at all.
fn keep(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("part");
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, path)
}

/// `body` within what a request carries of one document.
fn cut(body: &str) -> String {
    if body.len() <= MAX_BYTES {
        return body.to_owned();
    }
    let end = (0..=MAX_BYTES)
        .rev()
        .find(|i| body.is_char_boundary(*i))
        .unwrap_or(0);
    format!("{}\n(the rest was left out: too long)", &body[..end])
}

fn minutes(age: Duration) -> String {
    match age.as_secs() / 60 {
        0 => "less than a minute".to_owned(),
        1 => "a minute".to_owned(),
        n => format!("{n} minutes"),
    }
}

/// FNV-1a, 64 bits: a name for a copy that stays the same across Rust
/// releases, which `DefaultHasher` does not promise.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(project: &str, path: &str) -> RulesEntry {
        RulesEntry {
            project: project.into(),
            only: Some("backend/**".into()),
            refresh_secs: Some(0),
            source: SourceSpec::File { path: path.into() },
        }
    }

    #[test]
    fn fnv1a_matches_its_published_vectors() {
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a("foobar"), 0x8594_4171_f739_67e8);
    }

    #[tokio::test]
    async fn a_projects_rules_come_from_their_copy_kept_fresh_in_the_background() {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("code/app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(home.path().join("rules.md"), "Use tabs.\n").unwrap();
        let cache = home.path().join("cache");
        let entries = [
            entry("~/code/app", "~/rules.md"),
            // Another project's: not this one's.
            entry("~/code/other", "~/other.md"),
        ];
        let rules =
            OutsideRules::for_project(&entries, &project, Some(home.path()), Some(&cache), None);
        assert_eq!(rules.kept.len(), 1);
        // Nothing kept yet: the request goes without, and reading starts.
        assert_eq!(rules.text(), None);
        assert!(rules.status()[0].ends_with("not read yet"));
        rules.refresh_due();
        for _ in 0..200 {
            if rules.text().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            rules.text().unwrap(),
            "## file:~/rules.md (only for files matching: backend/**)\n\
             They are kept in the file ~/rules.md.\nUse tabs."
        );

        // A source gone: the copy keeps serving, and the failure shows.
        std::fs::remove_file(home.path().join("rules.md")).unwrap();
        rules.refresh_due();
        for _ in 0..200 {
            if rules.status()[0].contains("failed") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(rules.status()[0].contains("last read failed"));
        assert!(rules.text().unwrap().ends_with("Use tabs."));
        // Not tried again at once.
        assert!(!rules.kept[0].is_due(SystemTime::now()));
    }

    #[test]
    fn an_mcp_page_says_how_to_change_it_and_a_long_one_is_cut() {
        let spec: SourceSpec = serde_json::from_value(serde_json::json!({
            "kind": "mcp", "server": "docs",
            "read": { "tool": "fetch", "arguments": { "id": "p1" } },
            "write": { "tool": "update", "arguments": { "id": "p1", "text": "{text}" } }
        }))
        .unwrap();
        let kept = Kept {
            entry: RulesEntry {
                project: "/p".into(),
                only: None,
                refresh_secs: None,
                source: spec,
            },
            source: Err("unused".into()),
            copy: PathBuf::new(),
            every: PERIOD,
            reading: AtomicBool::new(false),
            failed: Mutex::new(None),
        };
        let long = "x".repeat(MAX_BYTES + 10);
        let section = kept.section(&long);
        assert!(section.starts_with(
            "## mcp:docs/fetch {\"id\":\"p1\"}\nTo change them when the person asks, call \
             `docs__update` with {\"id\":\"p1\",\"text\":\"{text}\"}, `{text}` standing for the \
             whole new text.\n"
        ));
        assert!(section.ends_with("(the rest was left out: too long)"));
    }
}
