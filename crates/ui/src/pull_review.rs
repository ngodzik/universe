//! What a model says of a pull request reviewed: a summary of the whole,
//! opening with a small Mermaid diagram, one per area, and a word per file;
//! asked once per head commit, and kept beside its copy so that opening it
//! again costs nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use universe_tools::PullRequest;

/// The most of one area's diff a model reads, in bytes.
const AREA_BYTES: usize = 30_000;

/// What the model is asked.
pub const SUMMARY_PROMPT: &str = "You are given a pull request: its title, its description, then \
its diff grouped by area (API, database, back end, front end, tests, generated, ...). Write, for \
a reviewer who has not read it yet, a JSON object and nothing else, with: \"overall\": a summary \
of the whole in Markdown, a few short paragraphs, opening with a small Mermaid diagram (```mermaid, \
at most eight nodes) of what changes and how the parts relate; \"areas\": an object giving each \
area's name a short paragraph of what it changes there; \"files\": an object giving each file's \
path a few words. Say what the code does, not what the description claims, and point out what \
looks risky or unfinished.";

/// One area of a pull request's diff, as the model is given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AreaDiff {
    /// The area's name.
    pub area: String,
    /// Its files.
    pub files: Vec<String>,
    /// Generated code: its files are named, not shown.
    pub generated: bool,
    /// Its diff, unified.
    pub diff: String,
}

/// What a model said of a pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summaries {
    /// The head commit it was said of.
    pub head: String,
    /// Of the whole, in Markdown.
    pub overall: String,
    /// Of each area, by its name.
    #[serde(default)]
    pub areas: BTreeMap<String, String>,
    /// Of each file, by its path, in a few words.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

/// The question about `pull`: its title, its description, and its diff by
/// area, generated code listed only, each area within what a model reads.
#[must_use]
pub fn summary_question(pull: &PullRequest, areas: &[AreaDiff]) -> String {
    let mut out = format!(
        "Pull request #{}: {}\nBy {}\n\n{}\n",
        pull.number,
        pull.title,
        pull.author,
        pull.body.trim()
    );
    for area in areas {
        out.push_str(&format!("\n## {}\n", area.area));
        if area.generated {
            out.push_str(&format!(
                "Generated files, not shown: {}\n",
                area.files.join(", ")
            ));
            continue;
        }
        if area.diff.len() > AREA_BYTES {
            let end = (0..=AREA_BYTES)
                .rev()
                .find(|i| area.diff.is_char_boundary(*i))
                .unwrap_or(0);
            out.push_str(&area.diff[..end]);
            out.push_str("\n(the rest of this area's diff was left out: too long)\n");
        } else {
            out.push_str(&area.diff);
        }
    }
    out
}

/// What the model answered, read: its JSON object, wherever it stands in
/// the answer; the whole answer as the overall summary when there is none.
#[must_use]
pub fn parse_summaries(head: &str, answer: &str) -> Summaries {
    let object = answer
        .find('{')
        .zip(answer.rfind('}'))
        .filter(|(start, end)| start < end)
        .and_then(|(start, end)| {
            serde_json::from_str::<serde_json::Value>(&answer[start..=end]).ok()
        });
    let text = |value: &serde_json::Value| -> BTreeMap<String, String> {
        value
            .as_object()
            .map(|map| {
                map.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default()
    };
    match object {
        Some(value) if value["overall"].is_string() => Summaries {
            head: head.to_owned(),
            overall: value["overall"].as_str().unwrap_or_default().to_owned(),
            areas: text(&value["areas"]),
            files: text(&value["files"]),
        },
        _ => Summaries {
            head: head.to_owned(),
            overall: answer.trim().to_owned(),
            ..Summaries::default()
        },
    }
}

/// Where the summaries of the pull request checked out in `copy` are kept,
/// for its head commit `head`: beside the copy.
#[must_use]
pub fn kept_at(copy: &Path, head: &str) -> PathBuf {
    let name = copy
        .file_name()
        .map_or_else(|| "pr".to_owned(), |n| n.to_string_lossy().into_owned());
    copy.with_file_name(format!("{name}-{head}.summaries.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull() -> PullRequest {
        PullRequest {
            number: 7,
            title: "Uploads".into(),
            body: "Adds uploads.".into(),
            author: "someone".into(),
            url: String::new(),
            base: "main".into(),
            head: "abc".into(),
        }
    }

    #[test]
    fn the_model_reads_each_area_generated_code_named_only_and_long_diffs_cut() {
        let areas = [
            AreaDiff {
                area: "API".into(),
                files: vec!["api/routes.py".into()],
                generated: false,
                diff: "--- api/routes.py\n+def upload(): ...\n".into(),
            },
            AreaDiff {
                area: "Generated".into(),
                files: vec!["client/a.ts".into(), "client/b.ts".into()],
                generated: true,
                diff: "--- client/a.ts\n+secret of nothing\n".into(),
            },
            AreaDiff {
                area: "Tests".into(),
                files: vec!["tests/t.py".into()],
                generated: false,
                diff: "x".repeat(AREA_BYTES + 10),
            },
        ];
        let question = summary_question(&pull(), &areas);
        assert!(question.starts_with("Pull request #7: Uploads\nBy someone\n\nAdds uploads.\n"));
        assert!(question.contains("## API\n--- api/routes.py\n+def upload()"));
        assert!(question.contains("Generated files, not shown: client/a.ts, client/b.ts"));
        assert!(!question.contains("secret of nothing"));
        assert!(question.contains("(the rest of this area's diff was left out: too long)"));
    }

    #[test]
    fn the_answer_is_read_wherever_its_object_stands() {
        let answer = "Here it is:\n```json\n{\"overall\": \"It adds uploads.\", \
                      \"areas\": {\"API\": \"A route.\"}, \"files\": {\"api/routes.py\": \"the route\"}}\n```";
        let read = parse_summaries("abc", answer);
        assert_eq!(read.overall, "It adds uploads.");
        assert_eq!(read.areas["API"], "A route.");
        assert_eq!(read.files["api/routes.py"], "the route");
        assert_eq!(read.head, "abc");
        // No object: the answer is the summary.
        assert_eq!(
            parse_summaries("abc", "Plain words.").overall,
            "Plain words."
        );
    }

    #[test]
    fn summaries_are_kept_beside_the_copy_per_head() {
        let at = kept_at(Path::new("/r/app-1/pr-7"), "abc");
        assert_eq!(at, Path::new("/r/app-1/pr-7-abc.summaries.json"));
    }
}
