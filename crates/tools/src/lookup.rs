//! Where a name of an open file is defined or used: as the language server
//! for its file says, else as `git grep` reads it.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::definitions::{Definition, definitions, uses};
use crate::lsp::Servers;

/// A name to find, where it was asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lookup {
    /// The name.
    pub name: String,
    /// The repository it is looked for in.
    pub root: PathBuf,
    /// The file it is in, from the repository's root.
    pub path: PathBuf,
    /// Its line, from 0.
    pub line: usize,
    /// Its column, in characters.
    pub column: usize,
    /// The file's text.
    pub text: String,
    /// Where it is used, rather than where it is defined.
    pub uses: bool,
}

/// What a lookup found, and who found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The name looked up.
    pub name: String,
    /// The repository it was looked for in, whose root the places are
    /// from.
    pub root: PathBuf,
    /// Whether its uses were looked for, rather than its definition.
    pub uses: bool,
    /// The places, the likeliest first.
    pub places: Vec<Definition>,
    /// What found them: a language server, or `git grep`.
    pub by: String,
}

/// Where `lookup`'s name is defined or used in its repository, whose
/// language servers are `servers`: as the language server for its file
/// says, or, when none answers or it only points at an import of the file
/// itself (an alias it could not follow), as `git grep` reads it. Blocks
/// while the server answers: to be called from a thread of its own.
pub fn find(servers: &Servers, lookup: &Lookup) -> Found {
    let root = lookup.root.as_path();
    let asked = if lookup.uses {
        servers.references(&lookup.path, &lookup.text, lookup.line, lookup.column)
    } else {
        servers.definition(&lookup.path, &lookup.text, lookup.line, lookup.column)
    };
    let program = Servers::program_for(&lookup.path).unwrap_or("the language server");
    let mut files: HashMap<String, Vec<String>> = HashMap::new();
    let mut line_of = |path: &str, line: usize| -> String {
        let lines = files.entry(path.to_owned()).or_insert_with(|| {
            std::fs::read_to_string(root.join(path))
                .map(|t| t.lines().map(str::to_owned).collect())
                .unwrap_or_default()
        });
        lines
            .get(line.saturating_sub(1))
            .map(|l| l.trim().to_owned())
            .unwrap_or_default()
    };
    let here = lookup.path.to_string_lossy().into_owned();
    if let Ok(places) = asked {
        let places: Vec<Definition> = places
            .into_iter()
            .map(|p| Definition {
                text: line_of(&p.path, p.line),
                path: p.path,
                line: p.line,
            })
            .collect();
        let only_imports = places.iter().all(|p| {
            p.path == here && (p.text.starts_with("import ") || p.text.starts_with("from "))
        });
        if !places.is_empty() && (lookup.uses || !only_imports) {
            return Found {
                name: lookup.name.clone(),
                root: lookup.root.clone(),
                uses: lookup.uses,
                places,
                by: program.to_owned(),
            };
        }
    }
    let lines: Vec<String> = lookup.text.lines().map(str::to_owned).collect();
    let places = if lookup.uses {
        uses(root, &lookup.name)
    } else {
        definitions(root, &lookup.name, &lookup.path, &lines)
    };
    Found {
        name: lookup.name.clone(),
        root: lookup.root.clone(),
        uses: lookup.uses,
        places,
        by: "git grep".to_owned(),
    }
}
