//! The `universe` command: a project's code, API, database, services and
//! deployments, drawn in a window.

#![deny(unsafe_code)]

use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use universe_ui::{App, Defaults};

/// Opens a window on a project: its files as a universe of stars, its plan,
/// its services, its API, its database and its deployments.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The project's folder; the current one when not given.
    project: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // What universe keeps, its settings and the caches of the projects'
    // maps, is the person's alone.
    if let Err(e) = Defaults::keep_home_private() {
        eprintln!("universe could not make its folder private: {e}");
    }
    let project = match cli.project {
        Some(project) => project,
        None => std::env::current_dir().context("reading the current folder")?,
    };
    let project = project
        .canonicalize()
        .with_context(|| format!("{} cannot be opened", project.display()))?;
    ensure!(project.is_dir(), "{} is not a folder", project.display());
    // How the window looked when it last closed; a file that cannot be read
    // is said in the window, which then looks as at first.
    let (look, problem) = match Defaults::path().map(|path| Defaults::load(&path)) {
        Some(Ok(look)) => (look, None),
        Some(Err(e)) => (Defaults::default(), Some(e)),
        None => (Defaults::default(), None),
    };
    let mut app = App::new(project, look);
    if let Some(problem) = problem {
        app.report_error(problem);
    }
    universe_gui::run(app).context("the window")?;
    Ok(())
}
