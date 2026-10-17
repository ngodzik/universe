//! Code a change added that repeats code already in the project, found by
//! comparing lines: each run of [`WINDOW`] meaningful lines in a row that
//! the change added is looked for among the project's source files, lines
//! compared with their spacing gone. Comments, imports and lines too short
//! to say anything (`}`, `end`, `return`) do not count. Cheap and the same
//! every time: a model is only told what this finds, to share what does
//! the same thing.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::gitview::{LineMark, line_changes, lines_at};

/// How many meaningful lines in a row make a repeat worth telling.
pub const WINDOW: usize = 5;

/// The most repeats told: past a few, the model has the idea.
const MOST: usize = 12;

/// The largest file read, in bytes: larger ones are data or generated.
const LARGEST: u64 = 400_000;

/// The files read for code, by extension.
const SOURCES: [&str; 26] = [
    "rs", "py", "ts", "tsx", "js", "jsx", "mjs", "go", "java", "kt", "swift", "dart", "rb", "php",
    "c", "h", "cc", "cpp", "hpp", "cs", "scala", "vue", "svelte", "ex", "lua", "sql",
];

/// Folders that hold no code of the project's own.
const NOT_THE_PROJECTS: [&str; 9] = [
    "node_modules",
    ".venv",
    "venv",
    "target",
    ".git",
    "__pycache__",
    "dist",
    "build",
    "vendor",
];

/// Lines a change added that repeat lines already in the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repeat {
    /// The file the change added them to, from the root.
    pub path: String,
    /// Their first and last lines there, from 1.
    pub lines: (usize, usize),
    /// Where the same lines already are, from the root.
    pub found: String,
    /// Their first line there, from 1.
    pub found_line: usize,
}

/// The repeats among the lines the change added to `changed`, files from
/// `root`, since `base` (a commit; `HEAD` for what is not committed yet):
/// a file new since then counts as added whole. Reads files and runs git.
#[must_use]
pub fn repeats(root: &Path, changed: &[String], base: &str) -> Vec<Repeat> {
    let added: Vec<(String, Vec<(usize, String)>)> = changed
        .iter()
        .filter(|path| is_source(path))
        .filter_map(|path| {
            let now: Vec<String> = std::fs::read_to_string(root.join(path))
                .ok()?
                .lines()
                .map(str::to_owned)
                .collect();
            let before = lines_at(root, Path::new(path), base)?;
            let marks = line_changes(&before, &now).marks;
            let lines = now
                .into_iter()
                .zip(marks)
                .enumerate()
                .filter(|(_, (_, mark))| *mark != LineMark::Same)
                .map(|(i, (text, _))| (i + 1, text))
                .collect();
            Some((path.clone(), lines))
        })
        .collect();
    if added.iter().all(|(_, lines)| lines.len() < WINDOW) {
        return Vec::new();
    }
    let files = source_files(root);
    let read = |path: &str| -> Option<String> {
        let full = root.join(path);
        let size = std::fs::metadata(&full).ok()?.len();
        (size <= LARGEST).then(|| std::fs::read_to_string(full).ok())?
    };
    find(&added, &files, read)
}

/// A window of added lines: which file of the change, and its first line.
type Place = (usize, usize);

/// Where it is found: a file, and the first line there.
type Found = (String, usize);

/// The repeats of `added`, each file with the lines added to it (from 1,
/// as written), among `files`, read by `read`.
fn find(
    added: &[(String, Vec<(usize, String)>)],
    files: &[String],
    read: impl Fn(&str) -> Option<String>,
) -> Vec<Repeat> {
    // Each window of added lines, by the hash of its meaningful lines.
    let mut wanted: HashMap<u64, Vec<(usize, usize, usize)>> = HashMap::new();
    for (index, (_, lines)) in added.iter().enumerate() {
        let meaningful: Vec<(usize, String)> = lines
            .iter()
            .filter_map(|(n, text)| meaningful(text).map(|t| (*n, t)))
            .collect();
        for window in meaningful.windows(WINDOW) {
            // In a row in the file, else they are not one piece of code.
            if window[WINDOW - 1].0 - window[0].0 > WINDOW * 3 {
                continue;
            }
            wanted
                .entry(hash(window.iter().map(|(_, t)| t.as_str())))
                .or_default()
                .push((index, window[0].0, window[WINDOW - 1].0));
        }
    }
    if wanted.is_empty() {
        return Vec::new();
    }
    // Where each is found first, its own place aside.
    let mut found: HashMap<Place, Found> = HashMap::new();
    for file in files {
        let Some(text) = read(file) else {
            continue;
        };
        let meaningful: Vec<(usize, String)> = text
            .lines()
            .enumerate()
            .filter_map(|(i, line)| meaningful(line).map(|t| (i + 1, t)))
            .collect();
        for window in meaningful.windows(WINDOW) {
            let Some(places) = wanted.get(&hash(window.iter().map(|(_, t)| t.as_str()))) else {
                continue;
            };
            for (index, first, last) in places {
                let own = added[*index].0 == *file
                    && window[0].0 <= *last
                    && window[WINDOW - 1].0 >= *first;
                if !own {
                    found
                        .entry((*index, *first))
                        .or_insert_with(|| (file.clone(), window[0].0));
                }
            }
        }
    }
    // Windows that follow one another are one repeat.
    let mut repeats: Vec<Repeat> = Vec::new();
    let mut ordered: Vec<(&Place, &Found)> = found.iter().collect();
    ordered.sort_by_key(|((index, first), _)| (*index, *first));
    let ends: HashMap<Place, usize> = wanted
        .values()
        .flatten()
        .map(|(index, first, last)| ((*index, *first), *last))
        .collect();
    for ((index, first), (file, line)) in ordered {
        let last = ends.get(&(*index, *first)).copied().unwrap_or(*first);
        let path = &added[*index].0;
        match repeats.last_mut() {
            Some(previous) if previous.path == *path && *first <= previous.lines.1 + 1 => {
                previous.lines.1 = previous.lines.1.max(last);
            }
            _ => repeats.push(Repeat {
                path: path.clone(),
                lines: (*first, last),
                found: file.clone(),
                found_line: *line,
            }),
        }
    }
    // Two new files with the same lines: told once.
    let mut told: HashSet<(String, String)> = HashSet::new();
    repeats.retain(|r| {
        let pair = if r.path <= r.found {
            (r.path.clone(), r.found.clone())
        } else {
            (r.found.clone(), r.path.clone())
        };
        r.path == r.found || told.insert(pair)
    });
    repeats.truncate(MOST);
    repeats
}

/// What the model is told of `repeats`, or `None` when there are none.
#[must_use]
pub fn repeats_said(repeats: &[Repeat]) -> Option<String> {
    if repeats.is_empty() {
        return None;
    }
    let list: Vec<String> = repeats
        .iter()
        .map(|r| {
            format!(
                "- {}:{}-{} repeats {}:{}",
                r.path, r.lines.0, r.lines.1, r.found, r.found_line
            )
        })
        .collect();
    Some(list.join("\n"))
}

/// A line as compared, its spacing gone; `None` for one that says nothing
/// alone: blank, a comment, an import, or too short.
fn meaningful(line: &str) -> Option<String> {
    let text = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let quiet = [
        "//", "#", "/*", "*", "--", "<!--", "use ", "import ", "from ", "require(",
    ];
    if text.chars().count() < 8 || quiet.iter().any(|q| text.starts_with(q)) {
        return None;
    }
    Some(text)
}

/// FNV-1a over `lines`, each ended: the same for the same lines.
fn hash<'a>(lines: impl Iterator<Item = &'a str>) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for line in lines {
        for byte in line.bytes().chain(std::iter::once(b'\n')) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

fn is_source(path: &str) -> bool {
    let in_project = !path.split('/').any(|part| NOT_THE_PROJECTS.contains(&part));
    in_project
        && Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SOURCES.contains(&e))
}

/// The project's source files, from the root: those git tracks, and the
/// new ones it does not ignore.
fn source_files(root: &Path) -> Vec<String> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|p| is_source(p))
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "fn total(items: &[Item]) -> u64 {\n    let mut sum = 0;\n    for item in items {\n        sum += item.price * item.count;\n    }\n    // done\n    apply_discount(sum, items.len())\n}\n";

    fn lines(text: &str, from: usize) -> Vec<(usize, String)> {
        text.lines()
            .enumerate()
            .map(|(i, l)| (from + i, l.to_owned()))
            .collect()
    }

    #[test]
    fn added_lines_that_repeat_code_elsewhere_are_found_spacing_aside() {
        let respaced = BODY.replace("    ", "  ");
        let added = vec![("src/new.rs".to_owned(), lines(&respaced, 10))];
        let files = vec!["src/old.rs".to_owned(), "src/new.rs".to_owned()];
        let read = |path: &str| match path {
            "src/old.rs" => Some(format!("// header\n\n{BODY}")),
            "src/new.rs" => Some(format!("{}\n{respaced}", "\n".repeat(8))),
            _ => None,
        };
        let found = find(&added, &files, read);
        assert_eq!(
            found,
            [Repeat {
                path: "src/new.rs".into(),
                lines: (10, 16),
                found: "src/old.rs".into(),
                found_line: 3,
            }]
        );
        assert_eq!(
            repeats_said(&found).unwrap(),
            "- src/new.rs:10-16 repeats src/old.rs:3"
        );
    }

    #[test]
    fn short_lines_comments_and_imports_make_no_repeat() {
        let text = "use std::fmt;\n}\n// a comment here\n}\nend\nimport os\n}\n";
        let added = vec![("a.py".to_owned(), lines(text, 1))];
        let files = vec!["b.py".to_owned()];
        assert!(find(&added, &files, |_| Some(text.to_owned())).is_empty());
        // Its own lines are no repeat of themselves.
        let added = vec![("a.rs".to_owned(), lines(BODY, 1))];
        let files = vec!["a.rs".to_owned()];
        assert!(find(&added, &files, |_| Some(BODY.to_owned())).is_empty());
    }

    #[test]
    fn a_change_is_compared_with_the_project_through_git() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
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
        };
        git(&["init", "-q"]);
        std::fs::write(dir.path().join("old.rs"), BODY).unwrap();
        std::fs::write(dir.path().join("new.rs"), "fn other() {}\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "base"]);
        std::fs::write(
            dir.path().join("new.rs"),
            format!("fn other() {{}}\n{BODY}"),
        )
        .unwrap();
        let found = repeats(dir.path(), &["new.rs".to_owned()], "HEAD");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].found.as_str(), found[0].lines.0), ("old.rs", 2));
        // Nothing added: nothing to tell.
        assert!(repeats(dir.path(), &["old.rs".to_owned()], "HEAD").is_empty());
    }
}
