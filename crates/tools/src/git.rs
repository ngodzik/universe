//! What git says of the project's files, for the tree to mark them.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

/// What git says of each changed file, relative to `root`: `M` modified,
/// `A` added, `?` not tracked yet, `D` deleted, `R` renamed. Empty outside a
/// repository.
pub fn file_status(root: &Path) -> HashMap<String, char> {
    let Some(prefix) = git(root, &["rev-parse", "--show-prefix"]) else {
        return HashMap::new();
    };
    let prefix = prefix.trim().to_owned();
    let Some(status) = git(root, &["status", "--porcelain=v1", "--untracked-files=all"]) else {
        return HashMap::new();
    };
    status
        .lines()
        .filter_map(|line| {
            let (code, path) = (line.get(..2)?, line.get(3..)?);
            // A rename lists "old -> new"; the file on disk is the new one.
            let path = path.rsplit(" -> ").next()?.trim_matches('"');
            let relative = path.strip_prefix(&prefix)?.to_owned();
            let letter = match code {
                "??" => '?',
                c if c.contains('D') => 'D',
                c if c.contains('R') => 'R',
                c if c.contains('A') => 'A',
                _ => 'M',
            };
            Some((relative, letter))
        })
        .collect()
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        // The repository of the directory given, whatever started universe.
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                // Run from a git hook, these would point at the outer repository.
                .env_remove("GIT_DIR")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_WORK_TREE")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q"]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.py"), "one\ntwo\n").unwrap();
        std::fs::write(dir.path().join("b.py"), "x\n").unwrap();
        run(&["add", "-A"]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "init",
        ]);
        dir
    }

    #[test]
    fn the_status_comes_from_git_and_nothing_outside_it() {
        let dir = repo();
        std::fs::write(dir.path().join("src/a.py"), "one\n2\n").unwrap();
        std::fs::write(dir.path().join("new.py"), "y\n").unwrap();
        let status = file_status(dir.path());
        assert_eq!(status.get("src/a.py"), Some(&'M'));
        assert_eq!(status.get("new.py"), Some(&'?'));
        assert_eq!(status.get("b.py"), None);

        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("a.py"), "x\n").unwrap();
        assert!(file_status(outside.path()).is_empty());
    }
}
