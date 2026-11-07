//! The `universe` command: a project's code, API, database, services and
//! deployments, or those of a system of several repositories, drawn in a
//! window.

#![deny(unsafe_code)]

use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use universe_codemap::{System, open_system, save_system};
use universe_ui::roots::Roots;
use universe_ui::{App, Defaults};

/// Opens a window on a project, or on a system of several repositories:
/// its files as a universe of stars, its plan, its services, its API, its
/// database and its deployments.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The folders to read. One, the current folder when none is given,
    /// opens the system that holds it, else it alone; several are read
    /// together.
    folders: Vec<PathBuf>,

    /// Declares the folders given as the system NAME, in
    /// ~/.universe/systems/NAME.toml, and opens it: any of them opens it
    /// whole afterwards.
    #[arg(long, value_name = "NAME")]
    save: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // What universe keeps, its settings and the caches of the systems'
    // maps, is the person's alone.
    if let Err(e) = Defaults::keep_home_private() {
        eprintln!("universe could not make its folder private: {e}");
    }
    let given = if cli.folders.is_empty() {
        vec![std::env::current_dir().context("reading the current folder")?]
    } else {
        cli.folders
    };
    let mut folders = Vec::new();
    for folder in given {
        let found = folder
            .canonicalize()
            .with_context(|| format!("{} cannot be opened", folder.display()))?;
        ensure!(found.is_dir(), "{} is not a folder", found.display());
        folders.push(found);
    }
    let base = Defaults::home();
    let home = std::env::var_os("HOME").map_or_else(|| folders[0].clone(), PathBuf::from);
    if let Some(name) = cli.save {
        let base = base
            .as_deref()
            .context("no home folder to keep the system in")?;
        let file = save_system(base, &name, &folders, &home)
            .with_context(|| format!("declaring the system {name}"))?;
        eprintln!("The system {name} is declared in {}", file.display());
        // Opened from any of its repositories, it is found whole.
        folders.truncate(1);
    }
    let mut problems = Vec::new();
    let system = match open_system(base.as_deref(), &folders, &home) {
        Ok(system) => system,
        Err(e) => {
            // The cause says where in the file.
            let cause = std::error::Error::source(&e)
                .map(|c| format!(": {c}"))
                .unwrap_or_default();
            problems.push(format!("{e}{cause}"));
            System::alone(&folders[0])
        }
    };
    // How the window looked when it last closed; a file that cannot be read
    // is said in the window, which then looks as at first.
    let look = match Defaults::path().map(|path| Defaults::load(&path)) {
        Some(Ok(look)) => look,
        Some(Err(e)) => {
            problems.push(e);
            Defaults::default()
        }
        None => Defaults::default(),
    };
    let roots = Roots::new(
        system
            .repos
            .code()
            .map(|r| (r.name.clone(), r.root.clone()))
            .collect(),
    );
    let mut app = App::new(system.name.clone(), roots, look);
    for problem in problems {
        app.report_error(problem);
    }
    universe_gui::run(app, system).context("the window")?;
    Ok(())
}
