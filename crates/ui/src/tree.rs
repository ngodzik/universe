//! The files of the repositories read as a tree, beside the file open:
//! a project's from its root, a system's under each repository's name.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::roots::Roots;

/// Directories that hold what a build produced or downloaded, never sources.
const SKIPPED: [&str; 3] = ["target", "node_modules", "__pycache__"];

/// One visible line of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// As the window names it: from the root of a project alone, from the
    /// repository's name in a system.
    pub path: PathBuf,
    /// The file or folder's name.
    pub name: String,
    /// How deep it is, the top level being 0.
    pub depth: usize,
    /// Whether it is a folder.
    pub is_dir: bool,
}

/// The files as a tree, with the directories the person opened, each file
/// marked with what git says of it.
#[derive(Debug)]
pub struct FileTree {
    roots: Roots,
    expanded: BTreeSet<PathBuf>,
    rows: Vec<Row>,
    selected: usize,
    /// The first row on screen, written by the view so that a mouse click
    /// can be mapped back to a row.
    offset: Cell<usize>,
    /// What git says of files not committed, by path as the window names
    /// it.
    git: HashMap<String, char>,
}

impl FileTree {
    pub(crate) fn new(roots: Roots) -> Self {
        let mut tree = Self {
            roots,
            expanded: BTreeSet::new(),
            rows: Vec::new(),
            selected: 0,
            offset: Cell::new(0),
            git: HashMap::new(),
        };
        tree.refresh();
        tree
    }

    /// The rows shown, folders unfolded.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The row selected, by its place in the rows.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether this folder is unfolded.
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// Git's letter for a file (`M`, `A`, `?`, `D`, `R`), or for a folder the
    /// strongest of its files': changes to tracked files before new files.
    pub fn git_status(&self, path: &Path, is_dir: bool) -> Option<char> {
        let path = path.to_string_lossy();
        let git = &self.git;
        if !is_dir {
            return git.get(path.as_ref()).copied();
        }
        let prefix = format!("{path}/");
        let mut found = None;
        for (file, letter) in git {
            if file.starts_with(&prefix) {
                if *letter != '?' {
                    return Some('M');
                }
                found = Some('?');
            }
        }
        found
    }

    /// The first row in view.
    pub fn offset(&self) -> usize {
        self.offset.get()
    }

    /// Records the first row in view, as the view scrolled it.
    pub fn set_offset(&self, offset: usize) {
        self.offset.set(offset);
    }

    /// Reads the directories again, keeping the selection on the same path
    /// when it still exists.
    pub(crate) fn refresh(&mut self) {
        let keep = self.rows.get(self.selected).map(|r| r.path.clone());
        let roots = self.roots.clone();
        self.git = roots
            .iter()
            .flat_map(|(_, root)| {
                universe_tools::file_status(root)
                    .into_iter()
                    .filter_map(|(path, letter)| Some((roots.named(root, &path)?, letter)))
            })
            .collect();
        self.rows.clear();
        if roots.prefixed() {
            // Each repository a folder of the top level, by its name.
            for (name, root) in roots.iter() {
                let path = PathBuf::from(name);
                let open = self.expanded.contains(&path);
                self.rows.push(Row {
                    path: path.clone(),
                    name: name.to_owned(),
                    depth: 0,
                    is_dir: true,
                });
                if open {
                    self.read_dir(root, &path, 1);
                }
            }
        } else if let Some((_, root)) = roots.iter().next() {
            self.read_dir(root, Path::new(""), 0);
        }
        if let Some(path) = keep
            && let Some(i) = self.rows.iter().position(|r| r.path == path)
        {
            self.selected = i;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn read_dir(&mut self, dir: &Path, relative: &Path, depth: usize) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() && !SKIPPED.contains(&name.as_str()) => dirs.push(name),
                Ok(t) if t.is_file() => files.push(name),
                _ => {}
            }
        }
        dirs.sort();
        files.sort();
        // Directories first, as in most file trees.
        for name in dirs {
            let path = relative.join(&name);
            let open = self.expanded.contains(&path);
            self.rows.push(Row {
                path: path.clone(),
                name,
                depth,
                is_dir: true,
            });
            if open {
                self.read_dir(
                    &dir.join(path.file_name().unwrap_or_default()),
                    &path,
                    depth + 1,
                );
            }
        }
        for name in files {
            self.rows.push(Row {
                path: relative.join(&name),
                name,
                depth,
                is_dir: false,
            });
        }
    }

    pub(crate) fn move_by(&mut self, delta: i32) {
        let last = self.rows.len().saturating_sub(1) as i64;
        self.selected = (self.selected as i64 + i64::from(delta)).clamp(0, last) as usize;
    }

    pub(crate) fn select(&mut self, index: usize) {
        if index < self.rows.len() {
            self.selected = index;
        }
    }

    pub(crate) fn select_last(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
    }

    /// Opens the selected row: a directory is expanded or collapsed, a file
    /// is returned for the caller to show.
    pub(crate) fn open(&mut self) -> Option<PathBuf> {
        let row = self.rows.get(self.selected)?.clone();
        if row.is_dir {
            if !self.expanded.remove(&row.path) {
                self.expanded.insert(row.path);
            }
            self.refresh();
            None
        } else {
            Some(row.path)
        }
    }

    /// Collapses the selected directory, or moves to the parent directory.
    pub(crate) fn close(&mut self) {
        let Some(row) = self.rows.get(self.selected).cloned() else {
            return;
        };
        if row.is_dir && self.expanded.remove(&row.path) {
            self.refresh();
            return;
        }
        if let Some(parent) = row.path.parent().filter(|p| !p.as_os_str().is_empty())
            && let Some(i) = self.rows.iter().position(|r| r.path == parent)
        {
            self.selected = i;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/bin")).unwrap();
        fs::create_dir_all(dir.path().join("target")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "fn a() {}\nfn b() {}").unwrap();
        fs::write(dir.path().join("src/bin/x.rs"), "").unwrap();
        fs::write(dir.path().join("README.md"), "").unwrap();
        fs::write(dir.path().join(".env"), "").unwrap();
        dir
    }

    fn names(tree: &FileTree) -> Vec<String> {
        tree.rows()
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    #[test]
    fn lists_directories_first_and_skips_noise() {
        let dir = project();
        let tree = FileTree::new(Roots::alone(dir.path().to_owned()));
        assert_eq!(names(&tree), ["src", "README.md"]);
    }

    /// `project()` as a repository, everything committed.
    fn repository() -> tempfile::TempDir {
        let dir = project();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        fs::write(dir.path().join("docs/guide.md"), "").unwrap();
        fs::write(dir.path().join("docs/faq.md"), "").unwrap();
        for args in [
            &["init", "-q"][..],
            &["add", "-A"],
            &["commit", "-q", "-m", "base"],
        ] {
            let ok = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(dir.path())
                .env_remove("GIT_DIR")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_WORK_TREE")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        }
        dir
    }

    #[test]
    fn the_tree_shows_everything_and_marks_what_is_not_committed() {
        let dir = repository();
        fs::write(dir.path().join("src/lib.rs"), "fn a() {}").unwrap();
        fs::write(dir.path().join("docs/new.md"), "").unwrap();
        let mut tree = FileTree::new(Roots::alone(dir.path().to_owned()));
        assert_eq!(names(&tree), ["docs", "src", "README.md"]);
        assert_eq!(tree.git_status(Path::new("src/lib.rs"), false), Some('M'));
        // A folder takes the strongest of its files' letters.
        assert_eq!(tree.git_status(Path::new("src"), true), Some('M'));
        assert_eq!(tree.git_status(Path::new("docs"), true), Some('?'));
        assert_eq!(tree.git_status(Path::new("README.md"), false), None);
        tree.select(0);
        assert_eq!(tree.open(), None);
        assert_eq!(
            names(&tree),
            [
                "docs",
                "  faq.md",
                "  guide.md",
                "  new.md",
                "src",
                "README.md"
            ]
        );
    }

    #[test]
    fn opening_a_directory_expands_it_and_h_goes_back() {
        let dir = project();
        let mut tree = FileTree::new(Roots::alone(dir.path().to_owned()));
        assert_eq!(tree.open(), None);
        assert_eq!(names(&tree), ["src", "  bin", "  lib.rs", "README.md"]);

        tree.move_by(2);
        assert_eq!(tree.open(), Some(PathBuf::from("src/lib.rs")));
        tree.close();
        assert_eq!(tree.selected(), 0);
        tree.close();
        assert_eq!(names(&tree), ["src", "README.md"]);
    }

    #[test]
    fn a_system_shows_each_repository_under_its_name() {
        let (api, web) = (project(), project());
        let roots = Roots::new(vec![
            ("api".into(), api.path().to_owned()),
            ("web".into(), web.path().to_owned()),
        ]);
        let mut tree = FileTree::new(roots);
        assert_eq!(names(&tree), ["api", "web"]);
        tree.select(1);
        assert_eq!(tree.open(), None);
        assert_eq!(names(&tree), ["api", "web", "  src", "  README.md"]);
        tree.select(3);
        assert_eq!(tree.open(), Some(PathBuf::from("web/README.md")));
    }
}
