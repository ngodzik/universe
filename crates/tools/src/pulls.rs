//! A pull request of GitHub, reviewed without touching the branch or the
//! working tree: read with `gh pr view`, which only reads, its commits
//! fetched as objects only, no branch moved, and checked out in a detached
//! worktree of its own beside universe's settings.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A pull request, as `gh pr view` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    /// Its number.
    pub number: u64,
    /// Its title.
    pub title: String,
    /// What its author wrote of it.
    pub body: String,
    /// Its author's login.
    pub author: String,
    /// Its page.
    pub url: String,
    /// The branch it would be merged into.
    pub base: String,
    /// The commit its branch is at.
    pub head: String,
}

/// A pull request checked out for review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedOut {
    /// The pull request.
    pub pull: PullRequest,
    /// The folder its head is checked out in.
    pub copy: PathBuf,
    /// Where its branch left the one it would be merged into.
    pub merge_base: String,
}

/// The number of the pull request `argument` names: `123`, `#123`, or its
/// page's link.
///
/// # Examples
///
/// ```
/// use universe_tools::pull_number;
///
/// assert_eq!(pull_number("#42"), Some(42));
/// assert_eq!(pull_number("https://github.com/owner/repo/pull/42/files"), Some(42));
/// assert_eq!(pull_number("main"), None);
/// ```
#[must_use]
pub fn pull_number(argument: &str) -> Option<u64> {
    let argument = argument.trim();
    if let Some(rest) = argument.split("/pull/").nth(1) {
        return rest.split(['/', '#', '?']).next()?.parse().ok();
    }
    argument.trim_start_matches('#').parse().ok()
}

/// Checks out pull request `number` of the project at `root` for review,
/// in a folder of its own under `reviews`: read, fetched, and checked out
/// at its head, the folder made again or moved on when it is there.
///
/// # Errors
///
/// What `gh` or `git` said when they could not.
pub fn check_out_pull(root: &Path, reviews: &Path, number: u64) -> Result<CheckedOut, String> {
    let pull = read_pull(root, number)?;
    let base = fetch_pull(root, &pull)?;
    let merge_base = git(root, &["merge-base", &base, &pull.head])
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| format!("#{number} shares no history with {}", pull.base))?;
    let copy = review_folder(root, reviews, number);
    if copy.join(".git").exists() {
        git(&copy, &["checkout", "--detach", "--quiet", &pull.head])
            .ok_or_else(|| format!("{} could not be moved to #{number}'s head", copy.display()))?;
    } else {
        if let Some(parent) = copy.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        // A folder left by a worktree git forgot is cleared first.
        git(root, &["worktree", "prune"]);
        let shown = copy.to_string_lossy().into_owned();
        git(
            root,
            &["worktree", "add", "--detach", "--quiet", &shown, &pull.head],
        )
        .ok_or_else(|| format!("#{number} could not be checked out in {shown}"))?;
    }
    Ok(CheckedOut {
        pull,
        copy,
        merge_base,
    })
}

/// Where pull request `number` of the project at `root` is checked out:
/// under `reviews`, in a folder named for the project and a hash of its
/// path, so that two projects of one name keep apart.
#[must_use]
pub fn review_folder(root: &Path, reviews: &Path, number: u64) -> PathBuf {
    let name = root.file_name().map_or_else(
        || "project".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let hash = stable_hash(&root.to_string_lossy());
    reviews
        .join(format!("{name}-{:08x}", hash & 0xffff_ffff))
        .join(format!("pr-{number}"))
}

/// The diff from `from` to `to` in the repository at `dir`, unified with
/// three lines around each change, at most `limit` bytes, said so when cut;
/// `None` when git cannot tell.
#[must_use]
pub fn diff_between(dir: &Path, from: &str, to: &str, limit: usize) -> Option<String> {
    let mut diff = git(dir, &["diff", "--no-color", "-U3", from, to])?;
    if diff.len() > limit {
        let end = (0..=limit)
            .rev()
            .find(|i| diff.is_char_boundary(*i))
            .unwrap_or(0);
        diff.truncate(end);
        diff.push_str("\n(the rest of the diff was left out: too long)\n");
    }
    Some(diff)
}

/// FNV-1a, 64 bits: a name that stays the same across Rust releases, which
/// `DefaultHasher` does not promise.
#[must_use]
pub fn stable_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Pull request `number`, read with `gh`, which only reads here.
fn read_pull(root: &Path, number: u64) -> Result<PullRequest, String> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--json",
            "number,title,body,author,url,baseRefName,headRefOid",
        ])
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("gh could not be started: {e}"))?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh could not read #{number}: {}", why.trim()));
    }
    parse_pull(&String::from_utf8_lossy(&output.stdout))
}

/// A pull request from what `gh pr view --json` printed.
fn parse_pull(text: &str) -> Result<PullRequest, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("gh's answer is not JSON: {e}"))?;
    let field = |name: &str| value[name].as_str().unwrap_or_default().to_owned();
    let pull = PullRequest {
        number: value["number"].as_u64().ok_or("gh gave no number")?,
        title: field("title"),
        body: field("body"),
        author: value["author"]["login"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        url: field("url"),
        base: field("baseRefName"),
        head: field("headRefOid"),
    };
    if pull.head.is_empty() || pull.base.is_empty() {
        return Err(format!("gh did not say #{}'s branches", pull.number));
    }
    Ok(pull)
}

/// Fetches the pull request's head and the branch it would be merged into,
/// as objects only, one at a time: only `FETCH_HEAD` says where each is,
/// no branch or remote branch moves, and no prompt for a password hangs.
/// Returns the commit that branch is at.
fn fetch_pull(root: &Path, pull: &PullRequest) -> Result<String, String> {
    let head = fetch(root, &format!("refs/pull/{}/head", pull.number))?;
    if head != pull.head {
        return Err(format!(
            "#{}'s head moved while it was read: try again",
            pull.number
        ));
    }
    fetch(root, &format!("refs/heads/{}", pull.base))
}

/// Fetches `reference` from `origin`, and returns the commit it is at.
fn fetch(root: &Path, reference: &str) -> Result<String, String> {
    let output = Command::new("git")
        .args(["fetch", "--no-tags", "--quiet", "origin", reference])
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git could not be started: {e}"))?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git could not fetch {reference}: {}", why.trim()));
    }
    git(root, &["rev-parse", "FETCH_HEAD"])
        .map(|sha| sha.trim().to_owned())
        .ok_or_else(|| format!("{reference} was fetched, but git cannot say where it is"))
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_TERMINAL_PROMPT", "0")
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

    #[test]
    fn a_pull_request_is_named_by_its_number_or_its_link() {
        assert_eq!(pull_number("123"), Some(123));
        assert_eq!(pull_number(" #7 "), Some(7));
        assert_eq!(pull_number("https://github.com/o/r/pull/31"), Some(31));
        assert_eq!(
            pull_number("https://github.com/o/r/pull/31#discussion"),
            Some(31)
        );
        assert_eq!(pull_number("feature/x"), None);
        assert_eq!(pull_number("a1b2c3d"), None);
    }

    #[test]
    fn what_gh_says_of_a_pull_request_is_read() {
        let text = r#"{"number":31,"title":"Window only","body":"What changes","author":{"login":"someone"},"url":"https://github.com/o/r/pull/31","baseRefName":"main","headRefOid":"abc123"}"#;
        let pull = parse_pull(text).unwrap();
        assert_eq!(
            (pull.number, pull.base.as_str(), pull.head.as_str()),
            (31, "main", "abc123")
        );
        assert_eq!(pull.author, "someone");
        assert!(parse_pull(r#"{"number":1}"#).is_err());
        assert!(parse_pull("nope").is_err());
    }

    #[test]
    fn two_projects_of_one_name_review_in_folders_apart() {
        let reviews = Path::new("/r");
        let a = review_folder(Path::new("/home/a/app"), reviews, 3);
        let b = review_folder(Path::new("/home/b/app"), reviews, 3);
        assert_ne!(a, b);
        assert!(a.starts_with("/r") && a.ends_with("pr-3"));
        assert!(a.parent().unwrap().to_string_lossy().contains("/app-"));
        assert_eq!(stable_hash("a"), 0xaf63_dc4c_8601_ec8c);
    }

    /// A repository with a pull request's head kept as GitHub keeps it,
    /// and a clone of it, as the person's project.
    fn pulled() -> (tempfile::TempDir, PathBuf, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let run = |at: &Path, args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(at)
                .env_remove("GIT_DIR")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_WORK_TREE")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {output:?}");
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        let upstream = dir.path().join("upstream");
        std::fs::create_dir(&upstream).unwrap();
        run(&upstream, &["init", "-q", "-b", "main"]);
        std::fs::write(upstream.join("a.txt"), "one\n").unwrap();
        run(&upstream, &["add", "a.txt"]);
        run(&upstream, &["commit", "-qm", "one"]);
        let base = run(&upstream, &["rev-parse", "HEAD"]);
        run(&upstream, &["checkout", "-qb", "feature"]);
        std::fs::write(upstream.join("a.txt"), "one\ntwo\n").unwrap();
        run(&upstream, &["commit", "-qam", "two"]);
        let head = run(&upstream, &["rev-parse", "HEAD"]);
        run(&upstream, &["update-ref", "refs/pull/5/head", &head]);
        run(&upstream, &["checkout", "-q", "main"]);
        let project = dir.path().join("project");
        run(
            dir.path(),
            &["clone", "-q", upstream.to_str().unwrap(), "project"],
        );
        (dir, project, base, head)
    }

    #[test]
    fn a_pull_requests_commits_are_fetched_and_checked_out_apart_from_the_project() {
        let (dir, project, base, head) = pulled();
        let pull = PullRequest {
            number: 5,
            title: "Two".into(),
            body: String::new(),
            author: "t".into(),
            url: String::new(),
            base: "main".into(),
            head: head.clone(),
        };
        assert_eq!(fetch_pull(&project, &pull).unwrap(), base);
        // No branch of the project moved, nor was one added.
        let branches = git(&project, &["branch", "--list"]).unwrap();
        assert_eq!(branches.trim(), "* main");
        let copy = review_folder(&project, &dir.path().join("reviews"), 5);
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        git(
            &project,
            &[
                "worktree",
                "add",
                "--detach",
                "--quiet",
                copy.to_str().unwrap(),
                &head,
            ],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(copy.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
        assert_eq!(
            std::fs::read_to_string(project.join("a.txt")).unwrap(),
            "one\n"
        );
        // A head that moved since gh read it is said so.
        let moved = PullRequest {
            head: base.clone(),
            ..pull
        };
        assert!(fetch_pull(&project, &moved).unwrap_err().contains("moved"));
    }
}
