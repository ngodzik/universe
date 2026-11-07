//! What the state asks for and only a loop can do: finding where a name is
//! defined, on a thread of its own, and keeping how the window looks. The
//! window drives the [`Host`] rather than carrying out an effect its own
//! way, so that effects are tested without it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use universe_tools::{Found, Servers};

use crate::app::{App, Effect};
use crate::defaults::Defaults;

/// Carries out the state's effects, and hands it what they found.
pub struct Host {
    /// The language servers of each repository looked in, each started the
    /// first time a file of its language is looked up there, and kept.
    servers: HashMap<PathBuf, Arc<Servers>>,
    found: (Sender<Found>, Receiver<Found>),
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("repositories", &self.servers.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    /// A host, with no language server started yet.
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
            found: mpsc::channel(),
        }
    }

    /// Carries out `effect`, without waiting: a lookup runs on a thread of
    /// its own, and what it finds comes through [`Host::receive`].
    pub fn carry_out(&mut self, app: &mut App, effect: Effect) {
        match effect {
            Effect::Find(lookup) => {
                let servers = Arc::clone(
                    self.servers
                        .entry(lookup.root.clone())
                        .or_insert_with(|| Arc::new(Servers::new(&lookup.root))),
                );
                let tx = self.found.0.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(universe_tools::find(&servers, &lookup));
                });
            }
            Effect::SaveDefaults(look) => {
                if let Some(path) = Defaults::path()
                    && let Err(e) = look.save(&path)
                {
                    app.report_error(format!(
                        "How the window looks could not be kept in {}: {e}",
                        path.display()
                    ));
                }
            }
        }
    }

    /// Hands the state what the lookups found since the last call, without
    /// waiting. Returns whether anything came.
    pub fn receive(&self, app: &mut App) -> bool {
        let mut came = false;
        while let Ok(found) = self.found.1.try_recv() {
            app.on_found(found);
            came = true;
        }
        came
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lookup_runs_apart_and_its_places_reach_the_state() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "nothing here\n").unwrap();
        let mut app = App::new(
            "p".into(),
            crate::roots::Roots::alone(dir.path().to_owned()),
            Defaults::default(),
        );
        let mut host = Host::new();
        app.open_path(PathBuf::from("a.txt"));
        let effect = universe_tools::Lookup {
            name: "missing".into(),
            root: dir.path().to_owned(),
            path: PathBuf::from("a.txt"),
            line: 0,
            column: 0,
            text: "nothing here".into(),
            uses: false,
        };
        host.carry_out(&mut app, Effect::Find(effect));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !host.receive(&mut app) {
            assert!(std::time::Instant::now() < deadline, "the lookup came back");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            app.notice()
                .is_some_and(|(said, _)| said.starts_with("No definition of missing"))
        );
    }
}
