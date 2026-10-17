//! Every changed file's diff as GitHub shows a pull request's: in one
//! stream, each line numbered in the old file and the new, coloured by its
//! language, the runs of unchanged lines folded but for a few around each
//! change. And the same as a model reads it: unified, a few lines around.

use std::ops::Range;
use std::path::Path;

use universe_tools::{Aligned, Change, ChangedFile};

use crate::highlight::Highlighter;
use crate::style::Rgb;

/// How many unchanged lines frame a change.
pub const CONTEXT: usize = 3;

/// How a row of a diff differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// The same in both.
    Same,
    /// Only in the new file.
    Added,
    /// Only in the old file.
    Removed,
}

/// One row of a file's diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// How it differs.
    pub kind: RowKind,
    /// Its line in the old file, from 1.
    pub old: Option<usize>,
    /// Its line in the new file, from 1.
    pub new: Option<usize>,
    /// Its text, as coloured runs.
    pub runs: Vec<(Rgb, String)>,
}

impl Row {
    /// Its text, plain.
    #[must_use]
    pub fn text(&self) -> String {
        self.runs.iter().map(|(_, t)| t.as_str()).collect()
    }
}

/// One file's diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Where it is, from the root.
    pub path: String,
    /// How it changed.
    pub change: Change,
    /// How many lines it gained.
    pub added: usize,
    /// How many it lost.
    pub removed: usize,
    /// Its rows, the whole file.
    pub rows: Vec<Row>,
    /// Not text: no row is shown.
    pub binary: bool,
}

impl FileDiff {
    /// The runs of unchanged rows a view folds, as ranges of `rows`: those
    /// more than [`CONTEXT`] rows from any change.
    #[must_use]
    pub fn folds(&self) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut start = None;
        let n = self.rows.len();
        for (i, row) in self.rows.iter().enumerate().chain([(n, &SENTINEL)]) {
            match (row.kind == RowKind::Same && i < n, start) {
                (true, None) => start = Some(i),
                (false, Some(first)) => {
                    // The file's start and end need no lines before or after.
                    let from = if first == 0 { 0 } else { first + CONTEXT };
                    let to = if i == n { n } else { i.saturating_sub(CONTEXT) };
                    if to > from + 1 {
                        out.push(from..to);
                    }
                    start = None;
                }
                _ => {}
            }
        }
        out
    }

    /// The diff as a model reads it: unified, [`CONTEXT`] lines around each
    /// change, every line marked ` `, `+` or `-`, each change led by where
    /// it is in the new file.
    #[must_use]
    pub fn unified(&self) -> String {
        if self.binary {
            return format!("{} (not text)\n", self.path);
        }
        let folds = self.folds();
        let shown = |i: usize| !folds.iter().any(|f| f.contains(&i));
        let mut out = format!("--- {}\n", self.path);
        let mut last_shown = None;
        for (i, row) in self.rows.iter().enumerate() {
            if !shown(i) {
                continue;
            }
            if last_shown != i.checked_sub(1) || last_shown.is_none() {
                let line = row.new.or(row.old).unwrap_or(1);
                out.push_str(&format!("@@ line {line}\n"));
            }
            last_shown = Some(i);
            let mark = match row.kind {
                RowKind::Same => ' ',
                RowKind::Added => '+',
                RowKind::Removed => '-',
            };
            out.push(mark);
            out.push_str(&row.text());
            out.push('\n');
        }
        out
    }
}

/// What ends the rows when their folds are found.
const SENTINEL: Row = Row {
    kind: RowKind::Added,
    old: None,
    new: None,
    runs: Vec::new(),
};

/// Each of `files`' diffs, from its text at `base` to the files in `root`,
/// coloured by its language. Reads files and runs git: on a thread.
#[must_use]
pub fn read(root: &Path, base: &str, files: &[ChangedFile]) -> Vec<FileDiff> {
    let highlighter = Highlighter::new();
    files
        .iter()
        .map(|file| file_diff(&highlighter, root, base, file))
        .collect()
}

fn file_diff(highlighter: &Highlighter, root: &Path, base: &str, file: &ChangedFile) -> FileDiff {
    let old_path = match &file.change {
        Change::Renamed { from } => from.clone(),
        _ => file.path.clone(),
    };
    let old = match file.change {
        Change::Added => Some(Vec::new()),
        _ => universe_tools::lines_at(root, Path::new(&old_path), base),
    };
    let new = match file.change {
        Change::Deleted => Some(Vec::new()),
        _ => std::fs::read_to_string(root.join(&file.path))
            .ok()
            .map(|t| t.lines().map(str::to_owned).collect()),
    };
    let (Some(old), Some(new)) = (old, new) else {
        return FileDiff {
            path: file.path.clone(),
            change: file.change.clone(),
            added: file.added,
            removed: file.removed,
            rows: Vec::new(),
            binary: true,
        };
    };
    let colour = |path: &str, lines: &[String]| -> Vec<Vec<(Rgb, String)>> {
        if let Some(mut highlighted) = highlighter.highlight(Path::new(path), lines)
            && highlighted.colour_to(highlighter, lines.len())
        {
            return highlighted.lines().to_vec();
        }
        lines
            .iter()
            .map(|l| vec![(Rgb(192, 197, 206), l.clone())])
            .collect()
    };
    let (old_runs, new_runs) = (colour(&old_path, &old), colour(&file.path, &new));
    let rows = universe_tools::aligned(&old, &new)
        .into_iter()
        .map(|aligned| match aligned {
            Aligned::Same(o, n) => Row {
                kind: RowKind::Same,
                old: Some(o + 1),
                new: Some(n + 1),
                runs: new_runs[n].clone(),
            },
            Aligned::Removed(o) => Row {
                kind: RowKind::Removed,
                old: Some(o + 1),
                new: None,
                runs: old_runs[o].clone(),
            },
            Aligned::Added(n) => Row {
                kind: RowKind::Added,
                old: None,
                new: Some(n + 1),
                runs: new_runs[n].clone(),
            },
        })
        .collect();
    FileDiff {
        path: file.path.clone(),
        change: file.change.clone(),
        added: file.added,
        removed: file.removed,
        rows,
        binary: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(kinds: &str) -> FileDiff {
        let rows = kinds
            .chars()
            .enumerate()
            .map(|(i, k)| Row {
                kind: match k {
                    '+' => RowKind::Added,
                    '-' => RowKind::Removed,
                    _ => RowKind::Same,
                },
                old: Some(i + 1),
                new: Some(i + 1),
                runs: vec![(Rgb(0, 0, 0), format!("l{}", i + 1))],
            })
            .collect();
        FileDiff {
            path: "a.rs".into(),
            change: Change::Modified,
            added: 0,
            removed: 0,
            rows,
            binary: false,
        }
    }

    #[test]
    fn unchanged_runs_fold_but_for_three_lines_around_each_change() {
        // Ten the same, a change, ten the same, a change, two the same.
        let diff = rows("..........+..........-..");
        assert_eq!(diff.folds(), [0..7, 14..18]);
        // Short runs do not fold.
        assert!(rows("...+...").folds().is_empty());
    }

    #[test]
    fn a_model_reads_each_change_with_three_lines_around() {
        let diff = rows("..........+...");
        let unified = diff.unified();
        assert_eq!(
            unified,
            "--- a.rs\n@@ line 8\n l8\n l9\n l10\n+l11\n l12\n l13\n l14\n"
        );
    }

    #[test]
    fn a_file_is_read_numbered_old_and_new_and_coloured() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(dir.path())
                .env_remove("GIT_DIR")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_WORK_TREE")
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}");
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        git(&["init", "-q"]);
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\nfn c() {}\nfn d() {}\n").unwrap();
        let files = universe_tools::changed_files(dir.path(), &base).unwrap();
        let diffs = read(dir.path(), &base, &files);
        let shown: Vec<(RowKind, Option<usize>, Option<usize>, String)> = diffs[0]
            .rows
            .iter()
            .map(|r| (r.kind, r.old, r.new, r.text()))
            .collect();
        assert_eq!(
            shown,
            [
                (RowKind::Same, Some(1), Some(1), "fn a() {}".to_owned()),
                (RowKind::Removed, Some(2), None, "fn b() {}".to_owned()),
                (RowKind::Added, None, Some(2), "fn c() {}".to_owned()),
                (RowKind::Added, None, Some(3), "fn d() {}".to_owned()),
            ]
        );
        // Coloured by its language: more than one colour on a line.
        assert!(diffs[0].rows[0].runs.len() > 1);
    }
}
