//! The project's files of instructions for coding agents, as agents are
//! told of them: walking a large repository takes a moment, so the list is
//! kept, and walked again in the background at most once a minute, a
//! request taking the list as it stands.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a list serves before the project is walked again.
const FRESH_FOR: Duration = Duration::from_secs(60);

/// How many folders down they are looked for.
const DEPTH: usize = 4;

/// The list, and when it was made.
type Listed = (Option<Instant>, Vec<String>);

/// The list as last made, shared with the walk that makes it again.
#[derive(Debug, Clone, Default)]
pub(crate) struct Guides {
    listed: Arc<Mutex<Listed>>,
    walking: Arc<AtomicBool>,
}

impl Guides {
    /// The list as it stands, the project at `root` walked again in the
    /// background when the list is old. Needs a Tokio runtime.
    pub(crate) fn get(&self, root: &Path) -> Vec<String> {
        let (at, list) = self.listed.lock().map(|l| l.clone()).unwrap_or_default();
        let old = at.is_none_or(|at| at.elapsed() >= FRESH_FOR);
        if old && !self.walking.swap(true, Ordering::SeqCst) {
            let (listed, walking) = (Arc::clone(&self.listed), Arc::clone(&self.walking));
            let root: PathBuf = root.to_owned();
            tokio::task::spawn_blocking(move || {
                let found = universe_tools::folder_guides(&root, DEPTH);
                if let Ok(mut listed) = listed.lock() {
                    *listed = (Some(Instant::now()), found);
                }
                walking.store(false, Ordering::SeqCst);
            });
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_list_is_made_in_the_background_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("api")).unwrap();
        std::fs::write(dir.path().join("api/AGENTS.md"), "Async.").unwrap();
        let guides = Guides::default();
        // Not walked yet: nothing, and the walk starts.
        assert!(guides.get(dir.path()).is_empty());
        for _ in 0..200 {
            if !guides.get(dir.path()).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(guides.get(dir.path()), ["api/AGENTS.md"]);
        // Kept for a while: a file added does not show at once.
        std::fs::write(dir.path().join("CLAUDE.md"), "Root.").unwrap();
        assert_eq!(guides.get(dir.path()), ["api/AGENTS.md"]);
    }
}
