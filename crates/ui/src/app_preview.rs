//! The App view's state: the project's front end running in the system's
//! Chrome against a simulated back end, as the preview last told it, and
//! what the person asks of it. The window says where the page goes on the
//! screen; the browser's own window shows it there, and the host carries
//! out the rest.

use std::collections::VecDeque;
use std::path::PathBuf;

use universe_preview::{Answer, Call, Config, Event, Operation, Place, State, Status};

/// How many calls are kept, the latest.
const KEPT_CALLS: usize = 400;

/// How many of the page's errors are kept, the latest.
const KEPT_ERRORS: usize = 100;

/// How many of the front end's lines are kept, the latest.
const KEPT_LINES: usize = 200;

/// How many lines the log keeps, the latest.
const KEPT_ACTIVITY: usize = 300;

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
    /// Show the browser's window with its page there, or hide it.
    Place(Option<Place>),
    /// Show a workflow: its steps' pages in turn.
    Play(Vec<String>),
    /// Change the state the back end answers in.
    Change(Change),
    /// Keep what operation answered last as its fixture.
    Keep(usize),
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
    /// rather than the browser's window shown.
    page_shown: bool,
    /// Where the browser's window was last asked to be.
    place: Option<Place>,
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

    /// Whether the browser's window shows the page: running, its first
    /// page loaded.
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
            ..Self::default()
        };
        AppOrder::Start(Box::new(config))
    }

    pub(crate) fn stop(&mut self) -> AppOrder {
        self.status = Some(Status::Stopped);
        self.playing = None;
        self.place = None;
        AppOrder::Stop
    }

    /// The order to show the browser's window at `place`, or to hide it,
    /// when that changed: shown only while the page is.
    pub(crate) fn place(&mut self, place: Option<Place>) -> Option<AppOrder> {
        let place = place.filter(|_| self.page_shown());
        if self.place == place || !self.is_running() {
            return None;
        }
        self.place = place;
        Some(AppOrder::Place(place))
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
                if !matches!(status, Status::Running { .. }) {
                    self.place = None;
                }
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
                self.place = None;
            }
        }
        None
    }
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
    fn the_browser_s_window_shows_once_the_first_page_has_loaded() {
        let place = Place {
            left: 10,
            top: 20,
            width: 800,
            height: 600,
        };
        let mut preview = AppPreview::default();
        preview.told(Event::Status(Status::Running {
            address: "http://127.0.0.1:3000".to_owned(),
        }));
        // Built still: the wait shows, not the window.
        assert!(!preview.page_shown());
        assert_eq!(preview.place(Some(place)), None);
        preview.told(address("http://127.0.0.1:3000/products", false));
        preview.told(Event::Quiet);
        assert!(preview.page_shown());
        assert_eq!(
            preview.place(Some(place)),
            Some(AppOrder::Place(Some(place)))
        );
        // Asked again where it is: nothing to send.
        assert_eq!(preview.place(Some(place)), None);
        assert_eq!(preview.place(None), Some(AppOrder::Place(None)));
        // Closed: a stop that says so, and nothing more to place.
        preview.told(Event::Gone);
        assert!(!preview.is_on());
        assert_eq!(preview.place(Some(place)), None);
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
}
