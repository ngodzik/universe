//! The App view's state: the project's front end running in a browser
//! against a simulated back end, as the preview last told it, and what the
//! person asks of it. Drawn by the window, which gives it the person's
//! pointer and keys, and carried out by the host.

use std::collections::VecDeque;
use std::path::PathBuf;

use universe_preview::{
    Answer, Call, Config, Event, Frame, Key, Operation, Pointer, State, Status,
};

/// How many calls are kept, the latest.
const KEPT_CALLS: usize = 400;

/// How many of the page's errors are kept, the latest.
const KEPT_ERRORS: usize = 100;

/// How many of the front end's lines are kept, the latest.
const KEPT_LINES: usize = 200;

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
    /// The person's pointer on the page.
    Pointer(Pointer),
    /// A key pressed on the page.
    Key(Key),
    /// Text typed on the page.
    Text(String),
    /// The page's size in its pixels, and the scale it is drawn at.
    Size(u32, u32, f32),
    /// Photograph the page under this label.
    Shoot(String),
    /// Show each of these paths in turn, each with its label.
    Play(Vec<(String, String)>),
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
}

/// A photograph of a step of a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    /// The step's label.
    pub label: String,
    /// Its picture, as PNG.
    pub png: Vec<u8>,
}

/// The App view's state.
#[derive(Debug, Default)]
pub struct AppPreview {
    status: Option<Status>,
    frame: Option<Frame>,
    address: String,
    operations: Vec<Operation>,
    calls: VecDeque<Call>,
    /// The first call of the page shown: those before are another's.
    page_from: u64,
    errors: VecDeque<String>,
    said: VecDeque<String>,
    state: State,
    focused: bool,
    hidden_pilot: bool,
    shots: Vec<Shot>,
    playing: Option<Playing>,
    kept: Option<PathBuf>,
    size: Option<(u32, u32, f32)>,
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

    /// Whether its page shows.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self.status, Some(Status::Running { .. }))
    }

    /// The page's last frame.
    #[must_use]
    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
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
        self.errors.iter()
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

    /// Whether the person's keys go to the page.
    #[must_use]
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Whether the pilot shows beside the page.
    #[must_use]
    pub fn pilot_shown(&self) -> bool {
        !self.hidden_pilot
    }

    /// The photographs of the workflow's steps.
    #[must_use]
    pub fn shots(&self) -> &[Shot] {
        &self.shots
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

    pub(crate) fn toggle_pilot(&mut self) {
        self.hidden_pilot = !self.hidden_pilot;
    }

    pub(crate) fn focus(&mut self, on: bool) {
        self.focused = on && self.is_running();
    }

    /// Starts afresh, as `config` says.
    pub(crate) fn start(&mut self, config: Config) -> AppOrder {
        *self = Self {
            hidden_pilot: self.hidden_pilot,
            status: Some(Status::Reading),
            size: Some(config.size),
            ..Self::default()
        };
        AppOrder::Start(Box::new(config))
    }

    pub(crate) fn stop(&mut self) -> AppOrder {
        self.status = Some(Status::Stopped);
        self.focused = false;
        self.playing = None;
        AppOrder::Stop
    }

    /// The order to size the page, if its size changed.
    pub(crate) fn size(&mut self, width: u32, height: u32, scale: f32) -> Option<AppOrder> {
        let size = (width.max(200), height.max(150), scale);
        if self.size == Some(size) {
            return None;
        }
        self.size = Some(size);
        Some(AppOrder::Size(size.0, size.1, size.2))
    }

    pub(crate) fn change(&mut self, change: Change) -> AppOrder {
        change.apply(&mut self.state);
        AppOrder::Change(change)
    }

    pub(crate) fn play(&mut self, playing: Playing, paths: Vec<String>) -> AppOrder {
        self.shots.clear();
        let steps = playing.steps.clone().into_iter().zip(paths).collect();
        self.playing = Some(playing);
        AppOrder::Play(steps)
    }

    /// Applies what the preview told; a fixture kept is the file to open.
    pub(crate) fn told(&mut self, event: Event) -> Option<PathBuf> {
        match event {
            Event::Status(status) => {
                if !matches!(status, Status::Running { .. }) {
                    self.focused = false;
                }
                self.status = Some(status);
            }
            Event::Operations(operations) => self.operations = operations,
            Event::Said(line) => push(&mut self.said, line, KEPT_LINES),
            Event::Call(call) => {
                if let Some(playing) = &mut self.playing
                    && let Some(operation) = call.operation.and_then(|i| self.operations.get(i))
                    && operation.method == playing.method
                    && operation.path == playing.path
                {
                    playing.acted = true;
                }
                push(&mut self.calls, call, KEPT_CALLS);
            }
            Event::Frame(frame) => self.frame = Some(frame),
            // A page that loads: its server's calls come before it says
            // where it is, however long it takes to build.
            Event::Loading => self.page_from = self.calls.back().map_or(0, |c| c.seq + 1),
            Event::Address { url, within } => {
                if within && url != self.address {
                    // An app that routes itself: the new page's calls are
                    // those of the last moment, and from now on.
                    self.page_from = self
                        .calls
                        .back()
                        .map_or(0, |c| c.seq.saturating_sub(recent(&self.calls)));
                }
                self.address = url;
            }
            Event::Quiet => {}
            Event::Console(error) => push(&mut self.errors, error, KEPT_ERRORS),
            Event::Shot { label, png } => self.shots.push(Shot { label, png }),
            Event::Kept(path) => {
                self.kept = Some(path.clone());
                return Some(path);
            }
            Event::Gone => {
                self.status = Some(Status::Failed("The browser went away".to_owned()));
                self.focused = false;
            }
        }
        None
    }
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

    #[test]
    fn a_page_s_calls_are_those_since_it_was_reached() {
        let address = |url: &str, within| Event::Address {
            url: url.to_owned(),
            within,
        };
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
            },
            vec!["/a".to_owned(), "/b".to_owned()],
        );
        assert_eq!(
            order,
            AppOrder::Play(vec![
                ("1".to_owned(), "/a".to_owned()),
                ("2".to_owned(), "/b".to_owned())
            ])
        );
        preview.told(Event::Call(call(1, 0, None)));
        assert!(!preview.playing().unwrap().acted);
        preview.told(Event::Call(call(2, 10, Some(0))));
        assert!(preview.playing().unwrap().acted);
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
        // A size asked again is not sent again.
        assert!(preview.size(800, 600, 1.0).is_some());
        assert!(preview.size(800, 600, 1.0).is_none());
    }
}
