//! The instructions a project already has for coding agents, as other tools
//! read them: CLAUDE.md, CLAUDE.local.md, AGENTS.md and the rules under
//! .claude/rules. Read only, never written: a model does not know they
//! exist unless it is given them.

use std::fs;
use std::path::{Path, PathBuf};

/// The most that is passed on, in bytes: instructions are paid for on every
/// call, and long ones are followed less well anyway.
const MAX_BYTES: usize = 40_000;

/// How deep `@path` imports are followed, as Claude Code does.
const MAX_IMPORT_DEPTH: usize = 4;

/// The project's instructions for coding agents, each file under its name,
/// `None` when it has none.
pub fn project_instructions(root: &Path) -> Option<String> {
    let mut files: Vec<PathBuf> = [
        "CLAUDE.md",
        ".claude/CLAUDE.md",
        "CLAUDE.local.md",
        "AGENTS.md",
    ]
    .iter()
    .map(|name| root.join(name))
    .filter(|path| path.is_file())
    .collect();
    let mut rules = Vec::new();
    collect_markdown(&root.join(".claude/rules"), &mut rules);
    rules.sort();
    files.extend(rules);

    let mut out = String::new();
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let name = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let (scope, body) = split_paths(&text);
        let body = expand_imports(body, path.parent().unwrap_or(root), root, 0);
        let header = match scope {
            Some(paths) => format!("## {name} (only for files matching: {paths})"),
            None => format!("## {name}"),
        };
        out.push_str(&format!("{header}\n{}\n\n", body.trim()));
        if out.len() > MAX_BYTES {
            let cut = (0..=MAX_BYTES)
                .rev()
                .find(|i| out.is_char_boundary(*i))
                .unwrap_or(0);
            out.truncate(cut);
            out.push_str("\n(the rest of the project's instructions was left out: too long)");
            break;
        }
    }
    (!out.trim().is_empty()).then(|| out.trim_end().to_owned())
}

/// The files a folder's instructions for coding agents are in, as other
/// tools read them.
const GUIDES: [&str; 3] = ["CLAUDE.md", "AGENTS.md", "CLAUDE.local.md"];

/// Folders built or downloaded, never searched for instructions: theirs
/// are a dependency's, not the project's.
const NOT_THE_PROJECTS: [&str; 7] = [
    "node_modules",
    ".venv",
    "venv",
    "target",
    ".git",
    "__pycache__",
    "worktrees",
];

/// The most of one folder's file that is passed on, in bytes.
const MAX_FOLDER_BYTES: usize = 20_000;

/// The instructions of the folders between `root` and `path`, a file or a
/// folder of the project, outermost first: each file's path from the root
/// and its text. The root's own are left out, as every request already
/// carries them; so is anything under a folder built or downloaded.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::create_dir_all(dir.path().join("api/routes")).unwrap();
/// std::fs::write(dir.path().join("api/CLAUDE.md"), "Use async handlers.").unwrap();
/// let found = universe_tools::folder_instructions(dir.path(), "api/routes/users.py".as_ref());
/// assert_eq!(found, [("api/CLAUDE.md".to_owned(), "Use async handlers.".to_owned())]);
/// ```
#[must_use]
pub fn folder_instructions(root: &Path, path: &Path) -> Vec<(String, String)> {
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    let Ok(relative) = full.strip_prefix(root) else {
        return Vec::new();
    };
    // The folders from the root's first one down to the path's own.
    let mut folders: Vec<PathBuf> = Vec::new();
    let mut walked = PathBuf::new();
    let components: Vec<_> = relative.components().collect();
    let last = if full.is_dir() {
        components.len()
    } else {
        components.len().saturating_sub(1)
    };
    for component in &components[..last] {
        let name = component.as_os_str().to_string_lossy();
        if NOT_THE_PROJECTS.contains(&name.as_ref()) {
            return Vec::new();
        }
        walked.push(component);
        folders.push(walked.clone());
    }
    let mut out = Vec::new();
    for folder in folders {
        for name in GUIDES {
            let file = folder.join(name);
            let Ok(text) = fs::read_to_string(root.join(&file)) else {
                continue;
            };
            let text = expand_imports(&text, &root.join(&folder), root, 0);
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            out.push((file.display().to_string(), cut(text, MAX_FOLDER_BYTES)));
        }
    }
    out
}

/// Every file of the project that holds instructions for coding agents, the
/// root's too, at most `depth` folders down, from the root: for an agent to
/// be told where they are. Folders built or downloaded, and those git
/// ignores, are left out.
#[must_use]
pub fn folder_guides(root: &Path, depth: usize) -> Vec<String> {
    let mut walk = ignore::WalkBuilder::new(root);
    walk.hidden(false)
        .git_ignore(true)
        .require_git(false)
        .max_depth(Some(depth + 1))
        .filter_entry(|entry| {
            !NOT_THE_PROJECTS.contains(&entry.file_name().to_string_lossy().as_ref())
        });
    let mut found: Vec<String> = walk
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter(|e| GUIDES.contains(&e.file_name().to_string_lossy().as_ref()))
        .filter_map(|e| {
            e.path()
                .strip_prefix(root)
                .ok()
                .map(|p| p.display().to_string())
        })
        .collect();
    found.sort();
    found
}

/// `text` within `max` bytes, said so when cut.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let end = (0..=max)
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0);
    format!("{}\n(the rest was left out: too long)", &text[..end])
}

/// The Markdown files under `dir`, in its subdirectories too.
fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// The `paths:` a rule is scoped to, from its front matter, and its body.
fn split_paths(text: &str) -> (Option<String>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    let Some(end) = rest.find("\n---") else {
        return (None, text);
    };
    let front = &rest[..end];
    let body = rest[end + 4..].trim_start_matches(['-', '\n']);
    let mut paths = Vec::new();
    let mut in_paths = false;
    for line in front.lines() {
        if let Some(value) = line.strip_prefix("paths:") {
            in_paths = true;
            let value = value.trim().trim_matches(['[', ']']);
            paths.extend(
                value
                    .split(',')
                    .map(|p| p.trim().trim_matches(['"', '\'']).to_owned())
                    .filter(|p| !p.is_empty()),
            );
        } else if in_paths && line.trim_start().starts_with('-') {
            let p = line.trim_start().trim_start_matches('-').trim();
            paths.push(p.trim_matches(['"', '\'']).to_owned());
        } else {
            in_paths = false;
        }
    }
    let scope = (!paths.is_empty()).then(|| paths.join(", "));
    (scope, body)
}

/// `text` with each `@path` replaced by that file, when it is inside the
/// project and holds no secrets; paths in backticks or code blocks are left
/// as written.
fn expand_imports(text: &str, base: &Path, root: &Path, depth: usize) -> String {
    let mut out = String::new();
    let mut in_block = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_block = !in_block;
        }
        out.push_str(line);
        out.push('\n');
        if in_block || depth >= MAX_IMPORT_DEPTH {
            continue;
        }
        let mut in_code = false;
        for word in line.split_whitespace() {
            if word.matches('`').count() % 2 == 1 {
                in_code = !in_code;
            }
            let Some(target) = word.strip_prefix('@').filter(|_| !in_code) else {
                continue;
            };
            let target = target.trim_end_matches(['.', ',', ';', ')']);
            let path = base.join(target);
            let real = path.canonicalize().ok();
            let inside = real
                .as_ref()
                .zip(root.canonicalize().ok())
                .filter(|(p, r)| p.starts_with(r))
                .is_some_and(|(p, _)| {
                    !crate::secrets::secret_file(target)
                        && !crate::secrets::secret_file(&p.to_string_lossy())
                });
            if !inside {
                continue;
            }
            if let Ok(imported) = fs::read_to_string(&path) {
                let nested =
                    expand_imports(&imported, path.parent().unwrap_or(base), root, depth + 1);
                out.push_str(&format!("(from {target})\n{}\n", nested.trim()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A monorepo: the root's guide, one per service, one deeper, and a
    /// dependency's that is not the project's.
    fn monorepo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for folder in [
            "api/routes",
            "web/src",
            "web/node_modules/lib",
            "build/out",
            "a/b/c/d/e",
        ] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        fs::write(root.join("CLAUDE.md"), "Root rules.").unwrap();
        fs::write(root.join("api/CLAUDE.md"), "Use async handlers. @notes.md").unwrap();
        fs::write(root.join("api/notes.md"), "Notes of the api.").unwrap();
        fs::write(root.join("api/routes/AGENTS.md"), "One route per file.").unwrap();
        fs::write(root.join("web/AGENTS.md"), "Components are functions.").unwrap();
        fs::write(root.join("web/node_modules/lib/CLAUDE.md"), "Not ours.").unwrap();
        fs::write(root.join("build/out/CLAUDE.md"), "Generated.").unwrap();
        fs::write(root.join("a/b/c/d/e/CLAUDE.md"), "Too deep to list.").unwrap();
        fs::write(root.join(".gitignore"), "build/\n").unwrap();
        dir
    }

    #[test]
    fn a_files_folders_give_their_instructions_outermost_first_the_roots_apart() {
        let dir = monorepo();
        let root = dir.path();
        let found = folder_instructions(root, Path::new("api/routes/users.py"));
        let names: Vec<&str> = found.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["api/CLAUDE.md", "api/routes/AGENTS.md"]);
        // Imports are followed, as in the root's.
        assert!(found[0].1.contains("(from notes.md)\nNotes of the api."));
        // A folder gives its own; an absolute path inside the project too.
        assert_eq!(
            folder_instructions(root, &root.join("web"))[0].0,
            "web/AGENTS.md"
        );
        // A dependency's are not the project's, nor is anything outside.
        assert!(folder_instructions(root, Path::new("web/node_modules/lib/x.js")).is_empty());
        assert!(folder_instructions(root, Path::new("/elsewhere/a.rs")).is_empty());
        assert!(folder_instructions(root, Path::new("README.md")).is_empty());
    }

    #[test]
    fn the_guides_listed_for_agents_skip_what_is_built_downloaded_or_too_deep() {
        let dir = monorepo();
        assert_eq!(
            folder_guides(dir.path(), 3),
            [
                "CLAUDE.md",
                "api/CLAUDE.md",
                "api/routes/AGENTS.md",
                "web/AGENTS.md"
            ]
        );
    }

    #[test]
    fn the_projects_own_instructions_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(project_instructions(root), None);

        fs::write(
            root.join("CLAUDE.md"),
            "Use tabs.\nSee @docs/style.md for more.\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/style.md"), "Name things plainly.").unwrap();
        fs::write(root.join("AGENTS.md"), "Run pytest.").unwrap();
        fs::create_dir_all(root.join(".claude/rules/frontend")).unwrap();
        fs::write(root.join(".claude/rules/testing.md"), "Test every route.").unwrap();
        fs::write(
            root.join(".claude/rules/frontend/react.md"),
            "---\npaths:\n  - \"app/frontend/**\"\n---\nUse function components.",
        )
        .unwrap();

        let text = project_instructions(root).unwrap();
        assert!(text.starts_with("## CLAUDE.md\nUse tabs."));
        assert!(text.contains("(from docs/style.md)\nName things plainly."));
        assert!(text.contains("## AGENTS.md\nRun pytest."));
        assert!(text.contains("## .claude/rules/testing.md\nTest every route."));
        assert!(text.contains(
            "## .claude/rules/frontend/react.md (only for files matching: app/frontend/**)\nUse function components."
        ));
    }

    #[test]
    fn imports_outside_the_project_or_in_code_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        fs::create_dir_all(&root).unwrap();
        fs::write(dir.path().join("secret.md"), "outside").unwrap();
        fs::write(root.join("x.md"), "inside").unwrap();
        fs::write(root.join(".env"), "API_KEY=sk-live-0123").unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "@../secret.md and `@x.md`, with @.env\n",
        )
        .unwrap();
        let text = project_instructions(&root).unwrap();
        assert!(!text.contains("outside") && !text.contains("(from x.md)"));
        assert!(!text.contains("sk-live"), "{text}");
    }
}
