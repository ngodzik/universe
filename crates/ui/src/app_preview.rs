//! The App view's state: the project's front end running in the Chrome
//! universe holds in its window, against a simulated back end, as the
//! preview last told it, and what the person asks of it. The window shows
//! the page; the host carries out the rest.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use universe_preview::{
    Answer, Call, Config, Event, Held, Operation, Spec, State, Status, Written,
};

/// How many calls are kept, the latest.
const KEPT_CALLS: usize = 400;

/// How many of the page's errors are kept, the latest.
const KEPT_ERRORS: usize = 100;

/// How many of the front end's lines are kept, the latest.
const KEPT_LINES: usize = 200;

/// How many lines the log keeps, the latest.
const KEPT_ACTIVITY: usize = 300;

/// How many of what the written behaviour refused are kept, the latest.
const KEPT_REFUSED: usize = 20;

/// What the model that writes a simulated back end's behaviour is told it
/// does, the program's API after it.
pub const BACK_END_PROMPT: &str = "You write the behaviour of a back end simulated from an OpenAPI spec, for a person who tests a front end against it in universe. Without your program, the simulated back end already answers every operation with a body made from its schema, and keeps what the page does: POST /items/ adds to the list, PATCH and PUT change an item, DELETE removes it, an action answered with the item changes it, ids given become the objects they name, and each item reads the same wherever it shows. You write only what these defaults cannot know: what the person asks for, what a real server would also do (a checkout confirmed makes an order), answers bent to a case. Write the whole program, those of its handlers the request does not change kept as they are, in one ```js block. It is tried before it is kept: every handler asked once, its answers checked against the spec's schemas.";

/// The behaviour a model wrote for the back end, as the preview last told
/// it, and what is asked of a model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WrittenBehaviour {
    /// The operations it handles, as it writes them.
    pub handled: Vec<String>,
    /// Those it names that the spec has not.
    pub unknown: Vec<String>,
    /// Why it does not run, when it does not.
    pub broken: Option<String>,
    /// What a model is asked to write, while it writes it.
    pub asked: Option<String>,
    /// What its handlers refused lately, the operation and why, the latest
    /// last.
    pub refused: VecDeque<(String, String)>,
}

/// What the App view asks the preview.
#[derive(Debug, Clone, PartialEq)]
pub enum AppOrder {
    /// Start it, as this says.
    Start(Box<Config>),
    /// Stop it, and all it started.
    Stop,
    /// Show this page: a path of the front end's.
    Go(String),
    /// Load the page again.
    Reload,
    /// Back in the page's history, or forward.
    History(bool),
    /// Show a workflow: its steps' pages in turn.
    Play(Vec<String>),
    /// Change the state the back end answers in.
    Change(Change),
    /// Keep what operation answered last as its fixture.
    Keep(usize),
    /// Forget what the page created, changed and deleted.
    Forget,
    /// Remove the behaviour written for the back end: its file.
    Unwrite(PathBuf),
}

/// A change to the state the simulated back end answers in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Every list empty, or not.
    Empty(bool),
    /// Every call slow, or not.
    Slow(bool),
    /// Every call failing, but those that tell who is there, or not.
    Failing(bool),
    /// No rights granted, or every one.
    NoRights(bool),
    /// Every flag of a feature on, or as the schema makes it.
    FlagsOn(bool),
    /// An operation answering so, or as the state says.
    Answer(usize, Option<Answer>),
    /// Back to answering as the spec says.
    Reset,
}

impl Change {
    /// Applies the change to a state.
    pub fn apply(&self, state: &mut State) {
        match self {
            Self::Empty(on) => state.tone.empty = *on,
            Self::Slow(on) => state.slow = *on,
            Self::Failing(on) => state.failing = *on,
            Self::NoRights(on) => state.tone.no_rights = *on,
            Self::FlagsOn(on) => state.tone.flags_on = *on,
            Self::Answer(index, Some(answer)) => {
                state.answers.insert(*index, answer.clone());
            }
            Self::Answer(index, None) => {
                state.answers.remove(index);
            }
            Self::Reset => *state = State::default(),
        }
    }
}

/// A workflow being shown, step by step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playing {
    /// Its name: `Create Product`.
    pub name: String,
    /// The labels of its steps, in order.
    pub steps: Vec<String>,
    /// The action it ends with: its method.
    pub method: String,
    /// And its path in the spec.
    pub path: String,
    /// Whether the page has called the action since it started.
    pub acted: bool,
    /// How many of its steps have loaded.
    pub shown: usize,
}

/// What kind of thing a line of the pilot's log tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    /// Something under way: a page loading, the front end starting.
    Step,
    /// A call the back end answered.
    Call,
    /// Something done: a page loaded.
    Done,
    /// The front end's server at work: building a page.
    Built,
    /// Something that went wrong.
    Failed,
}

/// A line of the pilot's log: what happened, said in a few words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    /// When, from when the preview started.
    pub at: std::time::Duration,
    /// What.
    pub text: String,
    /// Of what kind.
    pub kind: ActivityKind,
}

/// The App view's state.
#[derive(Debug, Default)]
pub struct AppPreview {
    status: Option<Status>,
    /// Whether its first page has loaded: until then, the wait is drawn
    /// rather than the page.
    page_shown: bool,
    /// Since when a page loads, and which, as far as known.
    loading: Option<std::time::Instant>,
    target: Option<String>,
    /// What happened, for the log, and since when.
    activity: VecDeque<Activity>,
    started: Option<std::time::Instant>,
    /// The page's errors are those from this count of errors on.
    errors_seen: u64,
    page_errors_from: u64,
    address: String,
    operations: Vec<Operation>,
    calls: VecDeque<Call>,
    /// The first call of the page shown: those before are another's.
    page_from: u64,
    errors: VecDeque<(u64, String)>,
    said: VecDeque<String>,
    state: State,
    /// What the back end holds of what the page did, by collection.
    held: Vec<Held>,
    /// The spec the back end is simulated from, once read.
    spec: Option<Arc<Spec>>,
    /// The file of the behaviour written for the back end, and the folder
    /// of its fixtures.
    program: Option<PathBuf>,
    fixtures: Option<PathBuf>,
    written: WrittenBehaviour,
    hidden_pilot: bool,
    playing: Option<Playing>,
    kept: Option<PathBuf>,
}

impl AppPreview {
    /// Where the preview is, once started.
    #[must_use]
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    /// Whether it runs or is starting: neither stopped nor failed.
    #[must_use]
    pub fn is_on(&self) -> bool {
        matches!(
            self.status,
            Some(
                Status::Reading
                    | Status::Starting { .. }
                    | Status::Opening
                    | Status::Running { .. }
            )
        )
    }

    /// Whether its page is open in the browser.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self.status, Some(Status::Running { .. }))
    }

    /// Whether the page shows: running, its first page loaded.
    #[must_use]
    pub fn page_shown(&self) -> bool {
        self.is_running() && self.page_shown
    }

    /// The page's address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The operations of the spec the back end is simulated from.
    #[must_use]
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }

    /// The calls of the page shown, the latest last.
    pub fn page_calls(&self) -> impl Iterator<Item = &Call> {
        self.calls.iter().filter(|c| c.seq >= self.page_from)
    }

    /// Every call kept, the latest last.
    pub fn calls(&self) -> impl Iterator<Item = &Call> {
        self.calls.iter()
    }

    /// The errors the page met, the latest last.
    pub fn errors(&self) -> impl Iterator<Item = &String> {
        self.errors.iter().map(|(_, e)| e)
    }

    /// The errors of the page shown, since it started loading.
    pub fn page_errors(&self) -> impl Iterator<Item = &String> {
        self.errors
            .iter()
            .filter(|(n, _)| *n >= self.page_errors_from)
            .map(|(_, e)| e)
    }

    /// The pilot's log, the latest last.
    pub fn activity(&self) -> impl Iterator<Item = &Activity> {
        self.activity.iter()
    }

    /// How long the page has been loading, while it does.
    #[must_use]
    pub fn loading_for(&self) -> Option<std::time::Duration> {
        self.loading.map(|since| since.elapsed())
    }

    /// The page being loaded, as far as known.
    #[must_use]
    pub fn loading_page(&self) -> &str {
        page_of(self.target.as_deref().unwrap_or(&self.address))
    }

    /// The lines the front end's server printed, the latest last.
    pub fn said(&self) -> impl Iterator<Item = &String> {
        self.said.iter()
    }

    /// The state the back end answers in.
    #[must_use]
    pub fn state(&self) -> &State {
        &self.state
    }

    /// What the back end holds of what the page created, changed and
    /// deleted, by collection.
    #[must_use]
    pub fn held(&self) -> &[Held] {
        &self.held
    }

    /// The behaviour written for the back end, as last told.
    #[must_use]
    pub fn written(&self) -> &WrittenBehaviour {
        &self.written
    }

    /// The file of the behaviour written for the back end.
    #[must_use]
    pub fn program(&self) -> Option<&Path> {
        self.program.as_deref()
    }

    /// The spec the back end is simulated from, once read.
    #[must_use]
    pub fn spec(&self) -> Option<&Arc<Spec>> {
        self.spec.as_ref()
    }

    /// The folder of the operations' fixtures.
    #[must_use]
    pub fn fixtures(&self) -> Option<&Path> {
        self.fixtures.as_deref()
    }

    /// Notes that a model is asked to write `asked`.
    pub(crate) fn asking(&mut self, asked: &str) {
        self.written.asked = Some(asked.to_owned());
        self.log(
            ActivityKind::Step,
            format!("A model writes the back end's behaviour: {asked}"),
        );
    }

    /// Notes what came of it: the operations the program kept handles, or
    /// why it was not kept.
    pub(crate) fn wrote(&mut self, result: &Result<Vec<String>, String>) {
        self.written.asked = None;
        let (kind, said) = match result {
            Ok(handled) => (
                ActivityKind::Done,
                format!("The back end's behaviour is kept: {}", handled.join(", ")),
            ),
            Err(why) => (
                ActivityKind::Failed,
                format!("The back end's behaviour was not kept: {why}"),
            ),
        };
        self.log(kind, said);
    }

    /// Whether the pilot shows beside the page.
    #[must_use]
    pub fn pilot_shown(&self) -> bool {
        !self.hidden_pilot
    }

    /// The workflow being shown.
    #[must_use]
    pub fn playing(&self) -> Option<&Playing> {
        self.playing.as_ref()
    }

    /// The last fixture kept.
    #[must_use]
    pub fn kept(&self) -> Option<&std::path::Path> {
        self.kept.as_deref()
    }

    fn log(&mut self, kind: ActivityKind, text: String) {
        let started = *self.started.get_or_insert_with(std::time::Instant::now);
        push(
            &mut self.activity,
            Activity {
                at: started.elapsed(),
                text,
                kind,
            },
            KEPT_ACTIVITY,
        );
    }

    /// Notes that `path` is asked for.
    pub(crate) fn going(&mut self, path: &str) {
        self.target = Some(path.to_owned());
        self.loading = Some(std::time::Instant::now());
        self.log(ActivityKind::Step, format!("Asked for {}", page_of(path)));
    }

    pub(crate) fn toggle_pilot(&mut self) {
        self.hidden_pilot = !self.hidden_pilot;
    }

    /// Starts afresh, as `config` says.
    pub(crate) fn start(&mut self, config: Config) -> AppOrder {
        *self = Self {
            hidden_pilot: self.hidden_pilot,
            status: Some(Status::Reading),
            target: Some(config.start.clone()),
            loading: Some(std::time::Instant::now()),
            program: config.program.clone(),
            fixtures: config.fixtures.clone(),
            ..Self::default()
        };
        AppOrder::Start(Box::new(config))
    }

    pub(crate) fn stop(&mut self) -> AppOrder {
        self.status = Some(Status::Stopped);
        self.playing = None;
        AppOrder::Stop
    }

    pub(crate) fn change(&mut self, change: Change) -> AppOrder {
        change.apply(&mut self.state);
        let said = match &change {
            Change::Empty(on) => format!("Lists empty: {}", on_off(*on)),
            Change::Slow(on) => format!("Every call slow: {}", on_off(*on)),
            Change::Failing(on) => format!("Every call failing: {}", on_off(*on)),
            Change::NoRights(on) => format!("No rights: {}", on_off(*on)),
            Change::FlagsOn(on) => format!("Every flag on: {}", on_off(*on)),
            Change::Answer(index, answer) => {
                let operation = self
                    .operations
                    .get(*index)
                    .map_or_else(String::new, |o| format!("{} {}", o.method, o.path));
                match answer {
                    Some(Answer::Status(status)) => format!("{operation} answers {status}"),
                    Some(Answer::Empty) => format!("{operation} answers empty"),
                    Some(Answer::Slow) => format!("{operation} answers slowly"),
                    None => format!("{operation} answers as the state says"),
                }
            }
            Change::Reset => "The back end answers as the spec says".to_owned(),
        };
        self.log(ActivityKind::Step, said);
        AppOrder::Change(change)
    }

    pub(crate) fn forget(&mut self) -> AppOrder {
        self.log(
            ActivityKind::Step,
            "Forgot what the page created, changed and deleted".to_owned(),
        );
        AppOrder::Forget
    }

    pub(crate) fn play(&mut self, playing: Playing, paths: Vec<String>) -> AppOrder {
        self.log(
            ActivityKind::Step,
            format!("Showing {}: {} screens", playing.name, paths.len()),
        );
        if let Some(first) = paths.first() {
            self.target = Some(first.clone());
            self.loading = Some(std::time::Instant::now());
        }
        self.playing = Some(playing);
        AppOrder::Play(paths)
    }

    /// Applies what the preview told; a fixture kept is the file to open.
    pub(crate) fn told(&mut self, event: Event) -> Option<PathBuf> {
        match event {
            Event::Status(status) => {
                let (kind, said) = match &status {
                    Status::Reading => (ActivityKind::Step, "Reading the spec".to_owned()),
                    Status::Starting { api, .. } => (
                        ActivityKind::Step,
                        format!("API simulated on port {api}; starting the front end"),
                    ),
                    Status::Opening => (
                        ActivityKind::Step,
                        "The front end answers: opening it in Chrome".to_owned(),
                    ),
                    Status::Running { .. } => (ActivityKind::Done, "Running".to_owned()),
                    Status::Failed(why) => (
                        ActivityKind::Failed,
                        format!("Stopped: {}", why.lines().next().unwrap_or_default()),
                    ),
                    Status::Stopped => (ActivityKind::Step, "Stopped".to_owned()),
                };
                self.log(kind, said);
                self.status = Some(status);
            }
            Event::Operations(operations) => {
                self.log(
                    ActivityKind::Step,
                    format!("The spec holds {} operations", operations.len()),
                );
                self.operations = operations;
            }
            Event::Said(line) => {
                if let Some((kind, said)) = server_said(&line) {
                    self.log(kind, said);
                }
                push(&mut self.said, line, KEPT_LINES);
            }
            Event::Call(call) => {
                if let Some(playing) = &mut self.playing
                    && let Some(operation) = call.operation.and_then(|i| self.operations.get(i))
                    && operation.method == playing.method
                    && operation.path == playing.path
                {
                    playing.acted = true;
                }
                let path = call.path.split('?').next().unwrap_or(&call.path);
                let source = match call.source {
                    universe_preview::Source::Schema => "made",
                    universe_preview::Source::Fixture => "fixture",
                    universe_preview::Source::Changed => "changed",
                    universe_preview::Source::Held => "held",
                    universe_preview::Source::Written => "written",
                    universe_preview::Source::Missing => "not in the spec",
                };
                let kind = if call.status >= 400 {
                    ActivityKind::Failed
                } else {
                    ActivityKind::Call
                };
                self.log(
                    kind,
                    format!(
                        "{} {path} → {} · {source} · {} ms",
                        call.method,
                        call.status,
                        call.took.as_millis()
                    ),
                );
                push(&mut self.calls, call, KEPT_CALLS);
            }
            // A page that loads: its server's calls come before it says
            // where it is, however long it takes to build.
            Event::Loading => {
                self.page_from = self.calls.back().map_or(0, |c| c.seq + 1);
                self.page_errors_from = self.errors_seen + 1;
                if self.loading.is_none() {
                    self.loading = Some(std::time::Instant::now());
                }
                self.log(ActivityKind::Step, "A page loads".to_owned());
            }
            Event::Address { url, within } => {
                if within && url != self.address {
                    // An app that routes itself: the new page's calls are
                    // those of the last moment, and from now on.
                    self.page_from = self
                        .calls
                        .back()
                        .map_or(0, |c| c.seq.saturating_sub(recent(&self.calls)));
                }
                if page_of(&url) != page_of(&self.address) {
                    self.log(ActivityKind::Step, format!("Showing {}", page_of(&url)));
                }
                // Routed within, there is nothing more to load: the browser
                // said it started, never that it ended.
                if within {
                    self.loading = None;
                    self.target = None;
                }
                self.address = url;
            }
            Event::Quiet => {
                if let Some(since) = self.loading.take() {
                    let page = page_of(&self.address).to_owned();
                    self.log(
                        ActivityKind::Done,
                        format!("{page} loaded in {:.1} s", since.elapsed().as_secs_f32()),
                    );
                }
                self.target = None;
                self.page_shown = true;
                if let Some(playing) = &mut self.playing
                    && playing.shown < playing.steps.len()
                {
                    playing.shown += 1;
                }
            }
            Event::Console(error) => {
                let first = error.lines().next().unwrap_or_default().to_owned();
                self.log(ActivityKind::Failed, first);
                self.errors_seen += 1;
                push(&mut self.errors, (self.errors_seen, error), KEPT_ERRORS);
            }
            Event::Held(held) => self.held = held,
            Event::Spec(spec) => self.spec = Some(spec),
            Event::Written(written) => self.told_written(written),
            Event::Kept(path) => {
                self.log(
                    ActivityKind::Done,
                    "Its answer kept as a fixture".to_owned(),
                );
                self.kept = Some(path.clone());
                return Some(path);
            }
            Event::Gone => {
                self.log(ActivityKind::Failed, "Chrome was closed".to_owned());
                self.status = Some(Status::Failed(
                    "Chrome was closed: start it again to see the page".to_owned(),
                ));
            }
        }
        None
    }
}

impl AppPreview {
    fn told_written(&mut self, written: Written) {
        match written {
            Written::Loaded { handled, unknown } => {
                let mut said = if handled.is_empty() {
                    "The back end answers without a written behaviour".to_owned()
                } else {
                    format!("The written behaviour handles {}", handled.join(", "))
                };
                if !unknown.is_empty() {
                    said.push_str(&format!("; the spec has no {}", unknown.join(", ")));
                }
                self.log(ActivityKind::Step, said);
                self.written.handled = handled;
                self.written.unknown = unknown;
                self.written.broken = None;
            }
            Written::Broken(why) => {
                self.log(
                    ActivityKind::Failed,
                    format!(
                        "The written behaviour does not run: {}",
                        why.lines().next().unwrap_or_default()
                    ),
                );
                self.written.broken = Some(why);
            }
            Written::Refused { operation, why } => {
                self.log(ActivityKind::Failed, format!("{operation}: {why}"));
                push(&mut self.written.refused, (operation, why), KEPT_REFUSED);
            }
            Written::Said(line) => self.log(ActivityKind::Step, line),
        }
    }
}

/// The program in what a model answered: its last block of JavaScript,
/// else all of it.
#[must_use]
pub fn program_in(answer: &str) -> String {
    let mut found = None;
    let mut rest = answer;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        let Some(line_end) = after.find('\n') else {
            break;
        };
        let language = after[..line_end].trim();
        let body = &after[line_end + 1..];
        let Some(close) = body.find("```") else {
            break;
        };
        if matches!(language, "js" | "javascript" | "") {
            found = Some(body[..close].trim().to_owned());
        }
        rest = &body[close + 3..];
    }
    found.unwrap_or_else(|| answer.trim().to_owned())
}

/// The path of an address, its query aside: `/dashboard/demo`.
fn page_of(address: &str) -> &str {
    let path = address.split_once("://").map_or(address, |(_, rest)| {
        rest.find('/').map_or("/", |i| &rest[i..])
    });
    path.split(['?', '#']).next().unwrap_or(path)
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// What a line of the front end's server says of its work, when it says
/// something worth a line of the log: a page built, a page served, an
/// error.
fn server_said(line: &str) -> Option<(ActivityKind, String)> {
    let line = line.trim();
    let short = |text: &str| text.chars().take(120).collect::<String>();
    if line.contains("Compiling") {
        let what = line
            .split("Compiling")
            .nth(1)
            .unwrap_or_default()
            .trim_matches(|c: char| c == '.' || c.is_whitespace());
        return Some((
            ActivityKind::Built,
            short(&format!("The server builds {what}")),
        ));
    }
    if line.starts_with('⨯') || line.contains("Error:") || line.starts_with("error") {
        return Some((
            ActivityKind::Failed,
            short(line.trim_start_matches('⨯').trim()),
        ));
    }
    let served = ["GET /", "POST /", "HEAD /"]
        .iter()
        .any(|m| line.starts_with(m))
        && line.contains(" in ");
    if served {
        return Some((
            ActivityKind::Built,
            short(&format!("The server answered {line}")),
        ));
    }
    None
}

/// How many of the latest calls came in the last moment: those a page
/// made as it loaded, before it said where it is.
fn recent(calls: &VecDeque<Call>) -> u64 {
    let Some(last) = calls.back() else {
        return 0;
    };
    let window = std::time::Duration::from_millis(1500);
    let count = calls
        .iter()
        .rev()
        .take_while(|c| last.at.saturating_sub(c.at) < window)
        .count();
    (count as u64).saturating_sub(1)
}

fn push<T>(list: &mut VecDeque<T>, item: T, most: usize) {
    list.push_back(item);
    while list.len() > most {
        list.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use universe_preview::Source;

    fn call(seq: u64, at_ms: u64, operation: Option<usize>) -> Call {
        Call {
            seq,
            at: Duration::from_millis(at_ms),
            method: "GET".to_owned(),
            path: "/v1/x".to_owned(),
            operation,
            status: 200,
            source: Source::Schema,
            took: Duration::from_millis(3),
            from_page: true,
        }
    }

    fn address(url: &str, within: bool) -> Event {
        Event::Address {
            url: url.to_owned(),
            within,
        }
    }

    #[test]
    fn a_page_s_calls_are_those_since_it_was_reached() {
        let mut preview = AppPreview::default();
        preview.told(Event::Call(call(1, 0, None)));
        preview.told(Event::Loading);
        // Its server's call, long before it says where it is.
        preview.told(Event::Call(call(2, 1000, None)));
        preview.told(address("http://127.0.0.1:3000/", false));
        preview.told(Event::Call(call(3, 9000, None)));
        let seqs: Vec<u64> = preview.page_calls().map(|c| c.seq).collect();
        assert_eq!(seqs, [2, 3]);
        // Routed within: its first call came just before it said so.
        preview.told(Event::Call(call(4, 20000, None)));
        preview.told(address("http://127.0.0.1:3000/products", true));
        preview.told(Event::Call(call(5, 20100, None)));
        let seqs: Vec<u64> = preview.page_calls().map(|c| c.seq).collect();
        assert_eq!(seqs, [4, 5]);
    }

    #[test]
    fn a_workflow_is_acted_once_its_action_is_called() {
        let mut preview = AppPreview::default();
        preview.told(Event::Operations(
            universe_preview::Spec::read(r#"{"paths": {"/v1/products/": {"post": {}}}}"#)
                .unwrap()
                .operations,
        ));
        let order = preview.play(
            Playing {
                name: "Create Product".to_owned(),
                steps: vec!["1".to_owned(), "2".to_owned()],
                method: "POST".to_owned(),
                path: "/v1/products/".to_owned(),
                acted: false,
                shown: 0,
            },
            vec!["/a".to_owned(), "/b".to_owned()],
        );
        assert_eq!(
            order,
            AppOrder::Play(vec!["/a".to_owned(), "/b".to_owned()])
        );
        preview.told(Event::Call(call(1, 0, None)));
        assert!(!preview.playing().unwrap().acted);
        preview.told(Event::Call(call(2, 10, Some(0))));
        assert!(preview.playing().unwrap().acted);
    }

    #[test]
    fn the_page_shows_once_its_first_page_has_loaded() {
        let mut preview = AppPreview::default();
        preview.told(Event::Status(Status::Running {
            address: "http://127.0.0.1:3000".to_owned(),
        }));
        // Built still: the wait shows, not the page.
        assert!(!preview.page_shown());
        preview.told(address("http://127.0.0.1:3000/products", false));
        preview.told(Event::Quiet);
        assert!(preview.page_shown());
        // Closed: a stop that says so, and no page.
        preview.told(Event::Gone);
        assert!(!preview.is_on());
        assert!(!preview.page_shown());
    }

    #[test]
    fn a_load_routed_within_the_page_ends() {
        let mut preview = AppPreview::default();
        preview.told(address("http://127.0.0.1:3000/products", false));
        preview.told(Event::Quiet);
        preview.told(Event::Loading);
        assert!(preview.loading_for().is_some());
        preview.told(address("http://127.0.0.1:3000/orders", true));
        assert!(preview.loading_for().is_none());
        assert!(preview.activity().any(|a| a.text == "Showing /orders"));
    }

    #[test]
    fn changes_keep_the_state_the_back_end_answers_in() {
        let mut preview = AppPreview::default();
        preview.change(Change::Empty(true));
        preview.change(Change::Answer(3, Some(Answer::Status(403))));
        assert!(preview.state().tone.empty);
        assert_eq!(preview.state().answers[&3], Answer::Status(403));
        preview.change(Change::Reset);
        assert_eq!(preview.state(), &State::default());
    }

    #[test]
    fn what_the_written_behaviour_does_is_told() {
        let mut preview = AppPreview::default();
        preview.asking("make an order");
        assert_eq!(preview.written().asked.as_deref(), Some("make an order"));
        preview.told(Event::Written(Written::Loaded {
            handled: vec!["POST /v1/checkouts/{id}/confirm".to_owned()],
            unknown: vec!["GET /v1/nowhere".to_owned()],
        }));
        preview.told(Event::Written(Written::Refused {
            operation: "POST /v1/checkouts/{id}/confirm".to_owned(),
            why: "it threw".to_owned(),
        }));
        preview.wrote(&Ok(vec!["POST /v1/checkouts/{id}/confirm".to_owned()]));
        let written = preview.written();
        assert_eq!(written.asked, None);
        assert_eq!(written.handled.len(), 1);
        assert_eq!(written.unknown, ["GET /v1/nowhere"]);
        assert_eq!(written.refused.len(), 1);
        assert!(
            preview
                .activity()
                .any(|a| a.text.starts_with("The back end's behaviour is kept"))
        );
        preview.told(Event::Written(Written::Broken(
            "SyntaxError at back-end.js:3".to_owned(),
        )));
        assert!(preview.written().broken.is_some());
    }

    #[test]
    fn a_program_is_the_last_block_of_javascript_answered() {
        assert_eq!(
            program_in(
                "Here:\n```js\non('GET /a', () => 1);\n```\nand\n```javascript\non('GET /b', () => 2);\n```\n"
            ),
            "on('GET /b', () => 2);"
        );
        assert_eq!(
            program_in("```json\n{}\n```\n```\non('GET /c', () => 3);\n```"),
            "on('GET /c', () => 3);"
        );
        assert_eq!(
            program_in("  on('GET /d', () => 4);  "),
            "on('GET /d', () => 4);"
        );
    }

    #[test]
    fn what_the_back_end_holds_is_told_and_forgotten() {
        let mut preview = AppPreview::default();
        let held = Held {
            path: "/v1/products".to_owned(),
            created: 1,
            changed: 0,
            deleted: 0,
        };
        preview.told(Event::Held(vec![held.clone()]));
        assert_eq!(preview.held(), [held]);
        assert_eq!(preview.forget(), AppOrder::Forget);
        assert!(preview.activity().any(|a| a.text.starts_with("Forgot")));
        preview.told(Event::Held(Vec::new()));
        assert!(preview.held().is_empty());
    }
}
