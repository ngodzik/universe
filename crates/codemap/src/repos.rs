//! The repositories read together as one system: those read as code, its
//! own, and those its settings link (deployment, infrastructure, modules),
//! read for where it runs. A place is a file and a line in one of them.
//!
//! The map of a system of several repositories names each file from the
//! repository's name (`api/app/main.py`); with one, from its root, as a
//! project alone has always been read.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A repository read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    /// Its folder's name, which places name it by.
    pub name: String,
    /// Its folder.
    pub root: PathBuf,
    /// Whether it is one of the system's own, read as code, which is never
    /// changed.
    pub own: bool,
}

/// Where something was read: a file of a repository, and a line.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Place {
    /// The repository, by its name.
    pub repo: String,
    /// The file, from the repository's root.
    pub path: String,
    /// The line, from 1; 0 for the whole file.
    pub line: usize,
}

impl Place {
    /// A place in `repo`.
    #[must_use]
    pub fn new(repo: &str, path: &str, line: usize) -> Self {
        Self {
            repo: repo.to_owned(),
            path: path.to_owned(),
            line,
        }
    }

    /// The same file, at `line`.
    #[must_use]
    pub fn at(&self, line: usize) -> Self {
        Self {
            line,
            ..self.clone()
        }
    }
}

impl std::fmt::Display for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}:{}", self.repo, self.path, self.line)
        } else {
            write!(f, "{}:{}", self.repo, self.path)
        }
    }
}

/// The repositories read together, the system's own first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repos {
    /// Each repository.
    pub repos: Vec<Repo>,
}

impl Repos {
    /// The project at `root`, and the folders `linked` to it.
    #[must_use]
    pub fn new(root: &Path, linked: &[PathBuf]) -> Self {
        Self::system(&[root.to_owned()], linked)
    }

    /// A system's repositories: `code`, its own, read as code, then those
    /// `linked` to it, read for where it runs. A folder named twice is read
    /// once, as its own when it is.
    #[must_use]
    pub fn system(code: &[PathBuf], linked: &[PathBuf]) -> Self {
        let mut out = Self { repos: Vec::new() };
        for (paths, own) in [(code, true), (linked, false)] {
            for path in paths {
                // The project's own folder is kept as given; a linked one
                // is resolved, as tools report it.
                let path = if own {
                    path.clone()
                } else {
                    path.canonicalize().unwrap_or_else(|_| path.clone())
                };
                if out.repos.iter().any(|r| r.root == path) {
                    continue;
                }
                let mut name = name_of(&path);
                // Two folders of the same name are told apart by their parent.
                if out.repos.iter().any(|r| r.name == name) {
                    let parent = path.parent().map(name_of).unwrap_or_default();
                    name = format!("{parent}/{name}");
                }
                out.repos.push(Repo {
                    name,
                    root: path,
                    own,
                });
            }
        }
        out
    }

    /// The repositories read as code: the system's own.
    pub fn code(&self) -> impl Iterator<Item = &Repo> {
        self.repos.iter().filter(|r| r.own)
    }

    /// Whether the map names files from their repository's name: when
    /// several are read as code.
    #[must_use]
    pub fn prefixed(&self) -> bool {
        self.code().nth(1).is_some()
    }

    /// The file a path of the map names, on this machine: from the root of
    /// the one repository read as code, else from the repository its first
    /// part names.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::{Path, PathBuf};
    /// use universe_codemap::Repos;
    ///
    /// let one = Repos::system(&[PathBuf::from("/code/api")], &[]);
    /// assert_eq!(one.path_of("app/main.py"), Some(PathBuf::from("/code/api/app/main.py")));
    /// let two = Repos::system(&[PathBuf::from("/code/api"), PathBuf::from("/code/web")], &[]);
    /// assert_eq!(two.path_of("web/src/App.tsx"), Some(PathBuf::from("/code/web/src/App.tsx")));
    /// assert_eq!(two.path_of("elsewhere/x"), None);
    /// ```
    #[must_use]
    pub fn path_of(&self, path: &str) -> Option<PathBuf> {
        if !self.prefixed() {
            let root = &self.code().next()?.root;
            return Some(if path.is_empty() {
                root.clone()
            } else {
                root.join(path)
            });
        }
        // The longest name first: a repository told apart by its parent
        // (`ops/api`) holds a slash.
        self.code()
            .filter_map(|r| {
                let rest = if path == r.name {
                    ""
                } else {
                    path.strip_prefix(&r.name)?.strip_prefix('/')?
                };
                Some((r, rest))
            })
            .max_by_key(|(r, _)| r.name.len())
            .map(|(r, rest)| {
                if rest.is_empty() {
                    r.root.clone()
                } else {
                    r.root.join(rest)
                }
            })
    }

    /// The path of the map that names `repo`'s file `path`, from its root.
    #[must_use]
    pub fn map_path(&self, repo: &Repo, path: &str) -> String {
        if !self.prefixed() {
            path.to_owned()
        } else if path.is_empty() {
            repo.name.clone()
        } else {
            format!("{}/{path}", repo.name)
        }
    }

    /// The repository named `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Repo> {
        self.repos.iter().find(|r| r.name == name)
    }

    /// The file a place names, on this machine.
    #[must_use]
    pub fn file(&self, place: &Place) -> Option<PathBuf> {
        Some(self.get(&place.repo)?.root.join(&place.path))
    }

    /// The repository holding `path` and the path from its root, the
    /// deepest repository first.
    #[must_use]
    pub fn locate(&self, path: &Path) -> Option<(&Repo, String)> {
        let path = normal(path);
        self.repos
            .iter()
            .filter_map(|r| {
                // A tool may report the real path of a root behind a symlink.
                let real = std::fs::canonicalize(&r.root).ok();
                let rel = path
                    .strip_prefix(normal(&r.root))
                    .ok()
                    .or_else(|| path.strip_prefix(real.as_deref()?).ok())?;
                Some((r, rel.to_string_lossy().replace('\\', "/")))
            })
            .max_by_key(|(r, _)| r.root.components().count())
    }
}

fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || "project".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// `path` with `.` and `..` resolved, without asking the file system.
pub(crate) fn normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The files under `root`, as the repository sees them (ignored files and
/// hidden folders left out), with their path from `root`.
pub(crate) fn files(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = ignore::WalkBuilder::new(root)
        .require_git(false)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| {
            let rel = e
                .path()
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            Some((e.path().to_owned(), rel))
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_systems_own_repositories_name_their_files_from_their_names() {
        let repos = Repos::system(
            &[
                PathBuf::from("/code/api"),
                PathBuf::from("/x/ops/api"),
                PathBuf::from("/code/web"),
            ],
            &[PathBuf::from("/nowhere/deploy")],
        );
        let names: Vec<(&str, bool)> = repos
            .repos
            .iter()
            .map(|r| (r.name.as_str(), r.own))
            .collect();
        assert_eq!(
            names,
            [
                ("api", true),
                ("ops/api", true),
                ("web", true),
                ("deploy", false)
            ]
        );
        assert!(repos.prefixed());
        assert_eq!(
            repos.path_of("ops/api/main.go"),
            Some(PathBuf::from("/x/ops/api/main.go"))
        );
        assert_eq!(repos.path_of("api"), Some(PathBuf::from("/code/api")));
        // A linked repository is not read as code: the map does not name it.
        assert_eq!(repos.path_of("deploy/k.yaml"), None);
        let web = repos.get("web").unwrap();
        assert_eq!(repos.map_path(web, "src/App.tsx"), "web/src/App.tsx");
        let alone = Repos::new(Path::new("/code/api"), &[]);
        assert!(!alone.prefixed());
        let api = alone.get("api").unwrap();
        assert_eq!(alone.map_path(api, "main.py"), "main.py");
    }

    #[test]
    fn a_path_is_found_in_the_deepest_repository() {
        let repos = Repos::new(
            Path::new("/code/app"),
            &[
                PathBuf::from("/code/app/vendor/mods"),
                PathBuf::from("/x/deploy"),
            ],
        );
        let (repo, rel) = repos
            .locate(Path::new("/code/app/vendor/mods/rds/main.tf"))
            .unwrap();
        assert_eq!((repo.name.as_str(), rel.as_str()), ("mods", "rds/main.tf"));
        let (repo, rel) = repos
            .locate(Path::new("/x/deploy/apps/../base/k.yaml"))
            .unwrap();
        assert_eq!(
            (repo.name.as_str(), rel.as_str()),
            ("deploy", "base/k.yaml")
        );
        assert!(repos.locate(Path::new("/elsewhere")).is_none());
        assert_eq!(
            Place::new("deploy", "a.yaml", 3).to_string(),
            "deploy:a.yaml:3"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_real_path_is_found_in_a_repository_behind_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("a/app");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("app");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let repos = Repos::new(&link, &[]);
        let real = real.canonicalize().unwrap();
        let (repo, rel) = repos.locate(&real.join("k8s/api.yaml")).unwrap();
        assert_eq!((repo.name.as_str(), rel.as_str()), ("app", "k8s/api.yaml"));
    }
}
