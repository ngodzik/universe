//! Tickets linked to a branch, from whatever tracker the settings declare
//! (`[trackers.<name>]`): an MCP server's tools read an issue and its
//! comments, a model lists the work as short items, and a check judges
//! which items the branch's changes serve.
//!
//! The model does what only a model can: list the work, and say whether
//! each item is done and which pieces of the diff serve it. Everything else
//! is counted: each item's share of the changed lines, the files with the
//! items they serve and what in them serves none, the generated code set
//! apart and never sent.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use universe_tools::Hunk;

/// A tracker the settings declare: which MCP server reads its tickets, with
/// which tools, and how its ids and links look.
///
/// ```toml
/// [trackers.issues]
/// server = "issues"
/// issue = "get_issue"
/// comments = "list_comments"
/// comments_argument = "issueId"
/// ids = "[A-Z][A-Z0-9]+-[0-9]+"
/// hosts = ["issues.example.com"]
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tracker {
    /// The MCP server, as named under `[mcp.<name>]`.
    pub server: String,
    /// Its tool that reads a ticket, which only reads.
    pub issue: String,
    /// Its tool that lists a ticket's comments, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<String>,
    /// The argument both tools take the ticket's id in.
    #[serde(default = "id_argument")]
    pub argument: String,
    /// The one its comments tool takes it in, when not the same: `issueId`
    /// where the issue's tool takes `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments_argument: Option<String>,
    /// What its ids look like, as a regular expression.
    pub ids: String,
    /// The hosts of its links: a link to one names one of its tickets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
}

fn id_argument() -> String {
    "id".to_owned()
}

/// Which tracker a ticket is from, and its id there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// The tracker's name in the settings.
    pub tracker: String,
    /// The ticket's id.
    pub id: String,
}

/// The tracker and id that `text`, an id or a link, names: a link by its
/// host, then the id in it; an id by the first tracker whose ids it looks
/// like.
///
/// # Errors
///
/// A sentence saying none matched, or no tracker is declared.
pub fn recognize(trackers: &BTreeMap<String, Tracker>, text: &str) -> Result<Named, String> {
    let text = text.trim();
    if trackers.is_empty() {
        return Err(
            "No tracker declared: add [trackers.<name>] to ~/.universe/config.toml, \
             with the MCP server that reads its tickets"
                .to_owned(),
        );
    }
    let link = text.contains("://");
    for (name, tracker) in trackers {
        let Ok(ids) = regex::Regex::new(&tracker.ids) else {
            continue;
        };
        if link && !tracker.hosts.iter().any(|h| text.contains(h.as_str())) {
            continue;
        }
        if let Some(found) = ids.find(text)
            && (link || found.as_str() == text)
        {
            return Ok(Named {
                tracker: name.clone(),
                id: found.as_str().to_owned(),
            });
        }
    }
    Err(format!(
        "{text} is not a ticket of the trackers declared: {}",
        trackers.keys().cloned().collect::<Vec<_>>().join(", ")
    ))
}

/// A ticket linked to the branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    /// Its tracker, by name.
    pub tracker: String,
    /// Its id.
    pub id: String,
    /// Its title, the first line of what was read.
    pub title: String,
    /// What was read of it: the issue and its comments, for the judge.
    pub text: String,
    /// The work, as short checkable items.
    pub items: Vec<String>,
}

impl Tracker {
    /// The argument the comments tool takes the ticket's id in.
    #[must_use]
    pub fn comments_argument(&self) -> &str {
        self.comments_argument.as_deref().unwrap_or(&self.argument)
    }
}

/// What a check found of one item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemProgress {
    /// Whether it is done.
    pub done: bool,
    /// Why, in a line.
    pub why: String,
    /// The pieces of the diff that serve it, by their place.
    pub hunks: Vec<usize>,
    /// Its share of the changed lines, from 0 to 1.
    pub share: f64,
}

/// A changed file, as a check sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileProgress {
    /// The file.
    pub path: String,
    /// The items it serves, as (ticket, item).
    pub items: Vec<(usize, usize)>,
    /// Its changed lines that serve no item.
    pub unattached: usize,
    /// Whether some of it is common to several items.
    #[serde(default)]
    pub common: bool,
    /// Its changed lines.
    pub lines: usize,
    /// Whether it is generated: set apart, never sent.
    pub generated: bool,
}

/// What the last check found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// For each ticket, for each item.
    pub items: Vec<Vec<Option<ItemProgress>>>,
    /// What several items need alike, given to none of them: shared code,
    /// a type, a migration they rely on. Never done or not.
    #[serde(default)]
    pub common: Option<ItemProgress>,
    /// The pieces of the diff, as they were numbered.
    pub hunks: Vec<HunkRef>,
    /// Every changed file.
    pub files: Vec<FileProgress>,
    /// The share of changed lines that serve no item.
    pub unattached: f64,
    /// The share that is generated code.
    pub generated: f64,
    /// Lines the branch added that repeat code already in the project.
    #[serde(default)]
    pub repeats: Vec<RepeatRef>,
}

/// Lines the branch added that repeat code already in the project, as
/// kept with a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeatRef {
    /// The file they were added to.
    pub path: String,
    /// Their first and last lines there, from 1.
    pub lines: (usize, usize),
    /// Where the same lines already are.
    pub found: String,
    /// Their first line there, from 1.
    pub found_line: usize,
}

impl From<&universe_tools::Repeat> for RepeatRef {
    fn from(repeat: &universe_tools::Repeat) -> Self {
        Self {
            path: repeat.path.clone(),
            lines: repeat.lines,
            found: repeat.found.clone(),
            found_line: repeat.found_line,
        }
    }
}

/// A piece of the diff, as kept with a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HunkRef {
    /// The file.
    pub path: String,
    /// Where it starts now, from 1; for a deleted file, where it was.
    pub start: usize,
    /// How many lines it changes.
    pub lines: usize,
    /// Whether its file is generated.
    pub generated: bool,
}

/// Everything kept of a branch's tickets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tickets {
    /// The tickets, in the order linked.
    pub tickets: Vec<Ticket>,
    /// What was checked elsewhere, told the judge at every check.
    pub notes: Vec<String>,
    /// The files already looked at, kept across checks.
    pub looked_at: BTreeSet<String>,
    /// The last check.
    pub progress: Option<Progress>,
}

/// Lock files: generated, and long.
const LOCK_FILES: [&str; 11] = [
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "uv.lock",
    "Pipfile.lock",
    "composer.lock",
    "Gemfile.lock",
    "go.sum",
    "flake.lock",
];

/// Whether the file at `path` is generated: a lock file, one whose first
/// lines say generated or not to edit, one `.gitattributes` marks
/// `linguist-generated`, or one of a package whose `__init__.py` starts as
/// openapi-python-client writes it (its files carry no header). An OpenAPI
/// spec is never generated: it is the API's contract.
#[must_use]
pub fn generated(path: &str, head: &str, package_init: Option<&str>, attributes: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    if is_spec(path) {
        return false;
    }
    if LOCK_FILES.iter().any(|l| l.eq_ignore_ascii_case(&name)) {
        return true;
    }
    let first: String = head
        .lines()
        .take(5)
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    if first.contains("generated") || first.contains("do not edit") || first.contains("don't edit")
    {
        return true;
    }
    if package_init.is_some_and(|text| {
        text.trim_start()
            .trim_start_matches(['"', '\''])
            .starts_with("A client library for accessing")
    }) {
        return true;
    }
    attributes.lines().any(|line| {
        let mut words = line.split_whitespace();
        let (Some(pattern), rest) = (words.next(), words.collect::<Vec<_>>()) else {
            return false;
        };
        rest.iter()
            .any(|w| *w == "linguist-generated" || *w == "linguist-generated=true")
            && glob(pattern, path)
    })
}

/// Whether the file at `path` is an OpenAPI spec, by its name: the API's
/// contract, whose pieces belong to the item whose endpoint they describe.
#[must_use]
pub fn is_spec(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    name.contains("openapi") || name.contains("swagger")
}

/// Whether `path` matches a `.gitattributes` pattern: `*` within a name,
/// `**` across folders, a pattern without `/` matching a name anywhere.
fn glob(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches('/');
    let target = if pattern.contains('/') {
        path
    } else {
        path.rsplit('/').next().unwrap_or(path)
    };
    let regex = format!(
        "^{}$",
        regex::escape(pattern)
            .replace(r"\*\*/", "(.*/)?")
            .replace(r"\*\*", ".*")
            .replace(r"\*", "[^/]*")
            .replace(r"\?", "[^/]")
    );
    regex::Regex::new(&regex).is_ok_and(|r| r.is_match(target))
}

/// What a model says of one item: whether it is done, why, which pieces
/// serve it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// Whether it is the common point's, what several items need alike,
    /// rather than an item's: then `ticket`, `item` and `done` say nothing.
    pub common: bool,
    /// The ticket, from 0.
    pub ticket: usize,
    /// The item, from 0.
    pub item: usize,
    /// Whether it is done.
    pub done: bool,
    /// Why.
    pub why: String,
    /// The pieces, by number from 1 as they were shown.
    pub hunks: Vec<usize>,
}

/// The items a model listed: a JSON array of strings, or lines.
#[must_use]
pub fn parse_items(answer: &str) -> Vec<String> {
    let text = strip_fence(answer);
    if let Ok(items) = serde_json::from_str::<Vec<String>>(text) {
        return items
            .into_iter()
            .map(|i| i.trim().to_owned())
            .filter(|i| !i.is_empty())
            .collect();
    }
    text.lines()
        .map(|l| l.trim().trim_start_matches(['-', '*', '•']).trim())
        .map(|l| {
            l.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
                .trim()
        })
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// What a model judged: a JSON array of objects with `ticket`, `item`,
/// `done`, `why` and `hunks`, each hunk `3` or `"H3"`; and the common
/// point's, `common` true with `why` and `hunks`.
///
/// # Errors
///
/// When the answer is not such an array.
pub fn parse_verdicts(answer: &str) -> Result<Vec<Verdict>, String> {
    let values: Vec<serde_json::Value> = serde_json::from_str(strip_fence(answer))
        .map_err(|e| format!("the judge's answer is not the JSON asked for: {e}"))?;
    let number = |v: &serde_json::Value| -> Option<usize> {
        match v {
            serde_json::Value::Number(n) => n.as_u64().map(|n| n as usize),
            serde_json::Value::String(s) => s.trim().trim_start_matches(['H', 'h']).parse().ok(),
            _ => None,
        }
    };
    Ok(values
        .iter()
        .filter_map(|v| {
            let common = v["common"].as_bool() == Some(true);
            Some(Verdict {
                common,
                ticket: number(&v["ticket"]).unwrap_or(1).saturating_sub(1),
                item: if common {
                    0
                } else {
                    number(&v["item"])?.saturating_sub(1)
                },
                done: v["done"].as_bool().unwrap_or(false),
                why: v["why"].as_str().unwrap_or("").to_owned(),
                hunks: v["hunks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(number)
                    .collect(),
            })
        })
        .collect())
}

/// `text` without a fence around it.
fn strip_fence(text: &str) -> &str {
    let text = text.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text);
    text.strip_suffix("```").unwrap_or(text).trim()
}

/// What a check found, counted: each piece of the diff given to one item,
/// or to the common point when the judge put it there or several items
/// claim it; each item's share of the changed lines; the files with the
/// items they serve and what in them serves none; the generated code
/// apart. The shares of the items, of the common point, of the lines that
/// serve none and of generated code add up to 1.
#[must_use]
pub fn progress(tickets: &[Ticket], hunks: &[(Hunk, bool)], verdicts: &[Verdict]) -> Progress {
    let total: usize = hunks.iter().map(|(h, _)| h.lines()).sum();
    let total = total.max(1) as f64;
    let mut items: Vec<Vec<Option<ItemProgress>>> =
        tickets.iter().map(|t| vec![None; t.items.len()]).collect();
    // The pieces as numbered from 1 when shown; generated pieces never are.
    let shown: Vec<usize> = (0..hunks.len()).filter(|i| !hunks[*i].1).collect();
    let pieces_of = |verdict: &Verdict| -> Vec<usize> {
        verdict
            .hunks
            .iter()
            .filter_map(|n| shown.get(n.wrapping_sub(1)).copied())
            .collect()
    };
    let mut claims: Vec<Vec<(usize, usize)>> = vec![Vec::new(); hunks.len()];
    let mut common_pieces: BTreeSet<usize> = BTreeSet::new();
    let mut common_why = String::new();
    for verdict in verdicts {
        let pieces = pieces_of(verdict);
        if verdict.common {
            common_pieces.extend(pieces);
            if common_why.is_empty() {
                common_why.clone_from(&verdict.why);
            }
            continue;
        }
        let Some(slot) = items
            .get_mut(verdict.ticket)
            .and_then(|t| t.get_mut(verdict.item))
        else {
            continue;
        };
        for piece in pieces {
            if !claims[piece].contains(&(verdict.ticket, verdict.item)) {
                claims[piece].push((verdict.ticket, verdict.item));
            }
        }
        *slot = Some(ItemProgress {
            done: verdict.done,
            why: verdict.why.clone(),
            hunks: Vec::new(),
            share: 0.0,
        });
    }
    // One item per piece: what several claim is common to them.
    for (piece, claimed) in claims.iter_mut().enumerate() {
        if claimed.len() > 1 || (!claimed.is_empty() && common_pieces.contains(&piece)) {
            common_pieces.insert(piece);
            claimed.clear();
        }
    }
    let mut common = (!common_pieces.is_empty()).then(|| ItemProgress {
        done: false,
        why: common_why,
        hunks: common_pieces.iter().copied().collect(),
        share: 0.0,
    });
    let mut unattached = 0.0;
    let mut generated = 0.0;
    let mut files: BTreeMap<String, FileProgress> = BTreeMap::new();
    for (i, (hunk, is_generated)) in hunks.iter().enumerate() {
        let lines = hunk.lines();
        let share = lines as f64 / total;
        let file = files
            .entry(hunk.path.clone())
            .or_insert_with(|| FileProgress {
                path: hunk.path.clone(),
                items: Vec::new(),
                unattached: 0,
                common: false,
                lines: 0,
                generated: *is_generated,
            });
        file.lines += lines;
        if *is_generated {
            generated += share;
            continue;
        }
        if common_pieces.contains(&i) {
            if let Some(common) = &mut common {
                common.share += share;
            }
            file.common = true;
            continue;
        }
        let Some(&(ticket, item)) = claims[i].first() else {
            unattached += share;
            file.unattached += lines;
            continue;
        };
        if let Some(Some(progress)) = items.get_mut(ticket).and_then(|t| t.get_mut(item)) {
            progress.share += share;
            progress.hunks.push(i);
        }
        if !file.items.contains(&(ticket, item)) {
            file.items.push((ticket, item));
        }
    }
    Progress {
        items,
        common,
        hunks: hunks
            .iter()
            .map(|(h, g)| HunkRef {
                path: h.path.clone(),
                start: if h.new_start > 0 {
                    h.new_start
                } else {
                    h.old_start
                },
                lines: h.lines(),
                generated: *g,
            })
            .collect(),
        files: files.into_values().collect(),
        unattached,
        generated,
        // Found by the host, which reads the project.
        repeats: Vec::new(),
    }
}

/// What the model that lists a ticket's work is told.
pub const ITEMS_PROMPT: &str = "You read a ticket of a software project, with its comments, and \
group the work it asks for into a few checkable items, six at most, each a part a developer can \
tick as a whole: what must be built or changed, not how, in a few words. Leave out what the \
comments settled as not to do. Answer with a JSON array of strings only.";

/// What the judge of the work is told.
pub const JUDGE_PROMPT: &str = "You judge how far a branch's work is against the items of its \
tickets. You get the tickets with their numbered items, notes of what was verified elsewhere, \
the generated files the branch changed, named only, and its diff cut into numbered pieces (H1, \
H2...). For each item, say whether it is done, why in one short line, and which pieces serve \
it. Give each piece to one item only, the one it serves most directly. A piece several items \
need alike (shared code, a common type, a migration they all rely on) goes to the common point \
instead, and a piece that serves no item to none. A piece of an OpenAPI spec belongs to the \
item whose endpoint or schema it describes; a regenerated client serves the item that changed \
the API. Answer with a JSON array only: one object per item, {\"ticket\": 1, \"item\": 1, \
\"done\": true, \"why\": \"...\", \"hunks\": [3, 5]}, and at most one for the common point, \
{\"common\": true, \"why\": \"...\", \"hunks\": [2]}.";

/// The judge's question: the tickets and their items, the notes, the
/// generated files named, and the pieces of the diff numbered, an OpenAPI
/// spec's marked, generated ones left out.
#[must_use]
pub fn judge_question(tickets: &[Ticket], notes: &[String], hunks: &[(Hunk, bool)]) -> String {
    let mut out = String::new();
    for (t, ticket) in tickets.iter().enumerate() {
        out.push_str(&format!(
            "Ticket {} ({}): {}\n",
            t + 1,
            ticket.id,
            ticket.title
        ));
        for (i, item) in ticket.items.iter().enumerate() {
            out.push_str(&format!("  item {}: {item}\n", i + 1));
        }
    }
    if !notes.is_empty() {
        out.push_str("\nVerified elsewhere:\n");
        for note in notes {
            out.push_str(&format!("- {note}\n"));
        }
    }
    let mut generated: Vec<&str> = hunks
        .iter()
        .filter(|(_, generated)| *generated)
        .map(|(hunk, _)| hunk.path.as_str())
        .collect();
    generated.dedup();
    if !generated.is_empty() {
        out.push_str(&format!(
            "\nGenerated files changed, not shown: {}\n",
            generated.join(", ")
        ));
    }
    out.push_str("\nThe branch's diff:\n");
    let mut number = 0;
    for (hunk, generated) in hunks {
        if *generated {
            continue;
        }
        number += 1;
        let spec = if is_spec(&hunk.path) {
            " (OpenAPI spec)"
        } else {
            ""
        };
        out.push_str(&format!(
            "\nH{number} {} line {}{spec}\n",
            hunk.path,
            hunk.new_start.max(hunk.old_start)
        ));
        for line in hunk.removed.iter().take(60) {
            out.push_str(&format!("-{line}\n"));
        }
        for line in hunk.added.iter().take(120) {
            out.push_str(&format!("+{line}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trackers() -> BTreeMap<String, Tracker> {
        BTreeMap::from([(
            "issues".to_owned(),
            Tracker {
                server: "issues".into(),
                issue: "get_issue".into(),
                comments: Some("list_comments".into()),
                argument: "id".into(),
                comments_argument: Some("issueId".into()),
                ids: "[A-Z][A-Z0-9]+-[0-9]+".into(),
                hosts: vec!["issues.example.com".into()],
            },
        )])
    }

    #[test]
    fn a_ticket_is_named_by_its_id_or_a_link_to_it() {
        let named = recognize(&trackers(), "ENG-42").unwrap();
        assert_eq!(
            (named.tracker.as_str(), named.id.as_str()),
            ("issues", "ENG-42")
        );
        let linked = recognize(
            &trackers(),
            "https://issues.example.com/team/issue/ENG-7/fix-it",
        )
        .unwrap();
        assert_eq!(linked.id, "ENG-7");
        assert!(recognize(&trackers(), "https://elsewhere.example.com/ENG-7").is_err());
        assert!(recognize(&trackers(), "not a ticket").is_err());
        assert!(
            recognize(&BTreeMap::new(), "ENG-1")
                .unwrap_err()
                .contains("[trackers")
        );
    }

    #[test]
    fn generated_code_is_told_apart_and_a_spec_never_is() {
        assert!(generated("Cargo.lock", "", None, ""));
        assert!(generated(
            "api/client.py",
            "# Code generated by a tool. DO NOT EDIT.\n",
            None,
            ""
        ));
        assert!(generated(
            "client/models/user.py",
            "from attrs import define\n",
            Some("\"\"\"A client library for accessing Shop\"\"\"\n"),
            ""
        ));
        assert!(generated(
            "web/src/api.gen.ts",
            "",
            None,
            "*.gen.ts linguist-generated\n"
        ));
        assert!(generated(
            "proto/out/a.py",
            "",
            None,
            "proto/out/** linguist-generated=true\n"
        ));
        assert!(!generated(
            "docs/openapi.yaml",
            "# generated by the server\n",
            None,
            ""
        ));
        assert!(!generated(
            "src/app.py",
            "import os\n",
            None,
            "*.gen.ts linguist-generated\n"
        ));
    }

    fn hunk(path: &str, added: usize) -> Hunk {
        Hunk {
            path: path.into(),
            old_start: 0,
            new_start: 1,
            removed: Vec::new(),
            added: vec!["x".into(); added],
        }
    }

    #[test]
    fn shares_of_items_unattached_lines_and_generated_code_add_up_to_all() {
        let tickets = vec![Ticket {
            tracker: "issues".into(),
            id: "ENG-1".into(),
            title: "Upload".into(),
            text: String::new(),
            items: vec!["accept files".into(), "store them".into()],
        }];
        let hunks = vec![
            (hunk("src/upload.py", 40), false),
            (hunk("src/store.py", 20), false),
            (hunk("README.md", 10), false),
            (hunk("Cargo.lock", 20), true),
            (hunk("src/routes.py", 5), false),
            (hunk("docs/openapi.yaml", 5), false),
        ];
        let answer = r#"```json
[{"ticket": 1, "item": 1, "done": true, "why": "the route takes files", "hunks": ["H1", "H4", "H5"]},
 {"ticket": 1, "item": 2, "done": false, "why": "half", "hunks": [1, 2]},
 {"common": true, "why": "the route both need", "hunks": ["H4"]}]
```"#;
        let verdicts = parse_verdicts(answer).unwrap();
        let progress = progress(&tickets, &hunks, &verdicts);
        let first = progress.items[0][0].as_ref().unwrap();
        let second = progress.items[0][1].as_ref().unwrap();
        let common = progress.common.as_ref().unwrap();
        // H1 (40 lines), claimed by both, is common to them, as H4 (5),
        // which the judge put there; H2 (20) is the second's, the spec's H5
        // (5) the first's.
        assert!((first.share - 0.05).abs() < 1e-9);
        assert!((second.share - 0.2).abs() < 1e-9);
        assert!((common.share - 0.45).abs() < 1e-9);
        assert_eq!(
            (first.hunks.as_slice(), common.hunks.as_slice()),
            (&[5][..], &[0, 4][..])
        );
        assert_eq!(common.why, "the route both need");
        assert!((progress.unattached - 0.1).abs() < 1e-9);
        assert!((progress.generated - 0.2).abs() < 1e-9);
        assert!(
            (first.share + second.share + common.share + progress.unattached + progress.generated
                - 1.0)
                .abs()
                < 1e-9
        );
        let readme = progress
            .files
            .iter()
            .find(|f| f.path == "README.md")
            .unwrap();
        assert_eq!((readme.unattached, readme.items.len()), (10, 0));
        let upload = progress
            .files
            .iter()
            .find(|f| f.path == "src/upload.py")
            .unwrap();
        assert!(upload.items.is_empty() && upload.common);
        // The generated file is named to the judge, never shown; the spec
        // is marked.
        let question = judge_question(&tickets, &["checked on staging".into()], &hunks);
        assert!(question.contains("Generated files changed, not shown: Cargo.lock"));
        assert!(question.contains("H3 README.md") && !question.contains("H6"));
        assert!(question.contains("H5 docs/openapi.yaml line 1 (OpenAPI spec)"));
        assert!(question.contains("checked on staging"));
    }

    #[test]
    fn items_are_read_as_json_or_as_lines() {
        assert_eq!(parse_items("[\"a\", \" b \"]"), ["a", "b"]);
        assert_eq!(
            parse_items("- accept files\n2. store them\n"),
            ["accept files", "store them"]
        );
    }
}
