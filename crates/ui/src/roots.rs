//! The repositories the window reads, each by its name: one for a project
//! alone, whose files the window names from its root; several for a
//! system, whose files it names from their repository's name
//! (`api/app/main.py`), as the map of the system does.

use std::path::{Path, PathBuf};

/// The repositories the window reads, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    roots: Vec<(String, PathBuf)>,
}

impl Roots {
    /// The repositories `roots`, each with its name.
    pub fn new(roots: Vec<(String, PathBuf)>) -> Self {
        Self { roots }
    }

    /// The project at `root`, alone.
    pub fn alone(root: PathBuf) -> Self {
        let name = root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        Self::new(vec![(name, root)])
    }

    /// Whether the window names files from their repository's name: when
    /// it reads several.
    pub fn prefixed(&self) -> bool {
        self.roots.len() > 1
    }

    /// Each repository, its name and its folder.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.roots.iter().map(|(n, r)| (n.as_str(), r.as_path()))
    }

    /// The folder of the repository a path of the window is in, and the
    /// path from its root.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::{Path, PathBuf};
    /// use universe_ui::roots::Roots;
    ///
    /// let roots = Roots::new(vec![
    ///     ("api".into(), PathBuf::from("/code/api")),
    ///     ("web".into(), PathBuf::from("/code/web")),
    /// ]);
    /// assert_eq!(roots.locate("web/src/App.tsx"), Some((Path::new("/code/web"), "src/App.tsx")));
    /// assert_eq!(roots.file("api/main.py"), Some(PathBuf::from("/code/api/main.py")));
    /// assert_eq!(roots.named(Path::new("/code/api"), "main.py").as_deref(), Some("api/main.py"));
    /// ```
    pub fn locate<'a>(&'a self, path: &'a str) -> Option<(&'a Path, &'a str)> {
        if !self.prefixed() {
            let (_, root) = self.roots.first()?;
            return Some((root.as_path(), path));
        }
        // The longest name first: a repository told apart by its parent
        // (`ops/api`) holds a slash.
        self.roots
            .iter()
            .filter_map(|(name, root)| {
                let rest = if path == name {
                    ""
                } else {
                    path.strip_prefix(name.as_str())?.strip_prefix('/')?
                };
                Some((name.len(), root.as_path(), rest))
            })
            .max_by_key(|(len, _, _)| *len)
            .map(|(_, root, rest)| (root, rest))
    }

    /// The file a path of the window names, on this machine.
    pub fn file(&self, path: &str) -> Option<PathBuf> {
        let (root, rest) = self.locate(path)?;
        Some(if rest.is_empty() {
            root.to_owned()
        } else {
            root.join(rest)
        })
    }

    /// The path of the window that names the file `path` of the repository
    /// at `root`, from its root.
    pub fn named(&self, root: &Path, path: &str) -> Option<String> {
        let (name, _) = self.roots.iter().find(|(_, r)| r == root)?;
        Some(match (self.prefixed(), path.is_empty()) {
            (false, _) => path.to_owned(),
            (true, true) => name.clone(),
            (true, false) => format!("{name}/{path}"),
        })
    }

    /// The path of the window that names the file at `file`, a whole path,
    /// if one of the repositories holds it: the deepest first.
    pub fn relative(&self, file: &Path) -> Option<String> {
        let (root, rest) = self
            .roots
            .iter()
            .filter_map(|(_, root)| Some((root, file.strip_prefix(root).ok()?)))
            .max_by_key(|(root, _)| root.components().count())?;
        self.named(root, &rest.to_string_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_alone_names_its_files_from_its_root() {
        let roots = Roots::alone(PathBuf::from("/code/shop"));
        assert!(!roots.prefixed());
        assert_eq!(
            roots.file("src/main.rs"),
            Some(PathBuf::from("/code/shop/src/main.rs"))
        );
        assert_eq!(
            roots
                .relative(Path::new("/code/shop/src/main.rs"))
                .as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(roots.relative(Path::new("/elsewhere/x")), None);
    }

    #[test]
    fn a_system_names_its_files_from_their_repository() {
        let roots = Roots::new(vec![
            ("api".into(), PathBuf::from("/code/api")),
            ("ops/api".into(), PathBuf::from("/x/ops/api")),
        ]);
        assert_eq!(
            roots.locate("ops/api/main.go"),
            Some((Path::new("/x/ops/api"), "main.go"))
        );
        assert_eq!(roots.locate("api"), Some((Path::new("/code/api"), "")));
        assert_eq!(roots.locate("web/x"), None);
        assert_eq!(
            roots.relative(Path::new("/x/ops/api/main.go")).as_deref(),
            Some("ops/api/main.go")
        );
    }
}
