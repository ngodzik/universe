//! The App view's state: the project's front end running in a browser
//! against a simulated back end, as the preview last told it, and what the
//! person asks of it. Drawn by the window, which gives it the person's
//! pointer and keys, and carried out by the host.

use std::collections::VecDeque;
use std::path::PathBuf;

use universe_preview::{
    Answer, Call, Config, Event, Frame, Input, Kept, KeptState, KeptTest, Key, Operation, Play,
    Pointer, Recorded, State, Status, Step,
};

/// The most inputs a test keeps of what the person did.
const KEPT_INPUTS: usize = 2000;

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
    /// Show a workflow: its steps in turn, then what the person did again.
    Play(Play),
    /// Keep the workflow shown as a test, with the photographs of its steps
    /// and the page as it is now.
    KeepTest {
        /// The test.
        test: Box<KeptTest>,
        /// The photographs of its steps, as PNG.
        shots: Vec<Vec<u8>>,
        /// The page now, its end.
        after: Option<Box<Frame>>,
    },
    /// Show a kept test again, the back end in its state.
    Replay {
        /// The state it was kept in.
        state: State,
        /// It, its photographs to compare with.
        play: Play,
    },
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
    /// The kept test shown again, by its place among them.
    pub replay: Option<usize>,
}

/// A photograph of a step of a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    /// The step's label.
    pub label: String,
    /// Its picture, as PNG.
    pub png: Vec<u8>,
}

/// What kind of thing a line of the pilot's log tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    /// Something under way: a page loading, the front end starting.
    Step,
    /// A call the back end answered.
    Call,
    /// Something done: a page loaded, a screen the same as kept.
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

/// The frame the page is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown<'a> {
    /// What it shows now.
    Live(&'a Frame),
    /// What it showed last, there or before, while it loads and shows
    /// nothing yet: no blank frame flashes between two pages.
    Stale(&'a Frame),
    /// Nothing yet: the first page is being built.
    Waiting,
}

/// How many lines the log keeps, the latest.
const KEPT_ACTIVITY: usize = 300;

/// How many pages' last frames are held, to show again as they load.
const KEPT_PAGES: usize = 12;

/// The App view's state.
#[derive(Debug, Default)]
pub struct AppPreview {
    status: Option<Status>,
    frame: Option<Frame>,
    /// The last frame that showed something, and the last of each page.
    good: Option<Frame>,
    by_page: VecDeque<(String, Frame)>,
    /// The pages' frames to keep between sessions, not yet given away.
    to_keep: Vec<(String, Frame)>,
    /// Whether the page shown may have changed since it was last kept,
    /// without loading again: an app that routes itself, a server that
    /// built it again.
    unsettled: bool,
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
    focused: bool,
    hidden_pilot: bool,
    shots: Vec<Shot>,
    playing: Option<Playing>,
    kept: Option<PathBuf>,
    size: Option<(u32, u32, f32)>,
    /// The paths of the workflow's steps.
    paths: Vec<String>,
    /// What the person did on the last step of the workflow shown.
    recording: Vec<Recorded>,
    last_input: Option<std::time::Instant>,
    /// The tests kept, and how each step of the one shown again compared.
    tests: Vec<Kept>,
    compared: Vec<(String, Option<f32>)>,
    /// Whether a kept test is being shown again, and whether it ends with
    /// a photograph of its end: the page keeps its size meanwhile.
    replaying: bool,
    replay_after: bool,
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

    /// The frame to draw the page with: what it shows, else, while it
    /// loads, what it last showed there or before, else nothing yet.
    #[must_use]
    pub fn shown(&self) -> Shown<'_> {
        match &self.frame {
            Some(frame) if !blank(frame) => Shown::Live(frame),
            _ => {
                // The page in this state, else in another, else the last.
                let page = page_of(self.target.as_deref().unwrap_or(&self.address));
                let key = self.page_key(page);
                let same_page = |k: &str| k.split_once('|').is_some_and(|(p, _)| p == page);
                let kept = self
                    .by_page
                    .iter()
                    .rev()
                    .find(|(k, _)| *k == key)
                    .or_else(|| self.by_page.iter().rev().find(|(k, _)| same_page(k)))
                    .map(|(_, f)| f)
                    .or(self.good.as_ref());
                kept.map_or(Shown::Waiting, Shown::Stale)
            }
        }
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

    /// A page's key among those kept: its path, and the state the back end
    /// answers in, for a page empty to differ from the same page full.
    fn page_key(&self, page: &str) -> String {
        let state = &self.state;
        let mut answers: Vec<String> = state
            .answers
            .iter()
            .map(|(i, a)| format!("{i}:{a:?}"))
            .collect();
        answers.sort();
        let on = |on: bool| if on { '1' } else { '0' };
        format!(
            "{page}|{}{}{}{}{}|{}",
            on(state.tone.empty),
            on(state.slow),
            on(state.failing),
            on(state.tone.no_rights),
            on(state.tone.flags_on),
            answers.join(",")
        )
    }

    /// Keeps the frame shown as the last of the page at `address`, here
    /// and between sessions, when it shows that page: not blank, and not
    /// what another showed before it.
    fn remember(&mut self) {
        let Some(frame) = self
            .good
            .as_ref()
            .filter(|g| self.frame.as_ref().is_some_and(|f| f.seq == g.seq))
        else {
            return;
        };
        if self.address.is_empty() {
            return;
        }
        let key = self.page_key(page_of(&self.address));
        if self
            .by_page
            .iter()
            .any(|(k, f)| *k == key && f.seq == frame.seq)
        {
            return;
        }
        let frame = frame.clone();
        self.by_page.retain(|(k, _)| *k != key);
        push(&mut self.by_page, (key.clone(), frame.clone()), KEPT_PAGES);
        self.to_keep.retain(|(k, _)| *k != key);
        self.to_keep.push((key, frame));
    }

    /// The pages' frames to keep between sessions, given away once.
    pub(crate) fn take_pages_to_keep(&mut self) -> Vec<(String, Frame)> {
        std::mem::take(&mut self.to_keep)
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

    /// Notes that `path` is asked for: its last frame shows as it loads.
    pub(crate) fn going(&mut self, path: &str) {
        if self.loading.is_none() {
            self.remember();
        }
        self.target = Some(path.to_owned());
        self.loading = Some(std::time::Instant::now());
        self.log(ActivityKind::Step, format!("Asked for {}", page_of(path)));
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

    /// The tests kept.
    #[must_use]
    pub fn tests(&self) -> &[Kept] {
        &self.tests
    }

    /// How each photograph of the test shown again compared with the one
    /// kept: its label, and the share of it that changed.
    #[must_use]
    pub fn compared(&self) -> &[(String, Option<f32>)] {
        &self.compared
    }

    /// Whether the workflow shown has shown all its screens: what the
    /// person does from then on is kept with it.
    #[must_use]
    pub fn shown_whole(&self) -> bool {
        self.playing
            .as_ref()
            .is_some_and(|p| p.replay.is_none() && self.shots.len() >= p.steps.len())
    }

    pub(crate) fn tests_kept(&mut self, tests: Vec<Kept>) {
        self.tests = tests;
    }

    /// Keeps what the person does on the last screen of a workflow.
    pub(crate) fn record(&mut self, order: &AppOrder) {
        if !self.shown_whole() || self.recording.len() >= KEPT_INPUTS {
            return;
        }
        let input = match order {
            AppOrder::Pointer(pointer) => Input::Pointer(*pointer),
            AppOrder::Key(key) => Input::Key(key.clone()),
            AppOrder::Text(text) => Input::Text(text.clone()),
            _ => return,
        };
        let now = std::time::Instant::now();
        let after_ms = self.last_input.map_or(400, |last| {
            u64::try_from(now.duration_since(last).as_millis()).unwrap_or(u64::MAX)
        });
        self.last_input = Some(now);
        self.recording.push(Recorded { after_ms, input });
    }

    /// The order to keep the workflow shown as a test.
    pub(crate) fn keep_test(&mut self) -> Option<AppOrder> {
        let playing = self.playing.as_ref().filter(|p| p.replay.is_none())?;
        if self.shots.len() < playing.steps.len() {
            return None;
        }
        let answers = self
            .state
            .answers
            .iter()
            .filter_map(|(index, answer)| {
                let operation = self.operations.get(*index)?;
                Some((
                    operation.method.clone(),
                    operation.path.clone(),
                    answer.clone(),
                ))
            })
            .collect();
        let state = KeptState {
            empty: self.state.tone.empty,
            slow: self.state.slow,
            failing: self.state.failing,
            no_rights: self.state.tone.no_rights,
            flags_on: self.state.tone.flags_on,
            answers,
        };
        let steps = playing
            .steps
            .iter()
            .cloned()
            .zip(self.paths.iter().cloned())
            .collect();
        let test = KeptTest {
            name: playing.name.clone(),
            method: playing.method.clone(),
            path: playing.path.clone(),
            steps,
            state,
            inputs: self.recording.clone(),
            acted: playing.acted,
            size: self.size.unwrap_or((1280, 800, 1.0)),
        };
        let shots = self
            .shots
            .iter()
            .take(playing.steps.len())
            .map(|s| s.png.clone())
            .collect();
        Some(AppOrder::KeepTest {
            test: Box::new(test),
            shots,
            after: self.frame.clone().map(Box::new),
        })
    }

    /// The order to show kept test `index` again, the back end in its
    /// state, its operations found again in the spec by method and path.
    pub(crate) fn replay(&mut self, index: usize) -> Option<AppOrder> {
        let kept = self.tests.get(index)?.clone();
        let test = &kept.test;
        let mut state = State::default();
        state.tone.empty = test.state.empty;
        state.slow = test.state.slow;
        state.failing = test.state.failing;
        state.tone.no_rights = test.state.no_rights;
        state.tone.flags_on = test.state.flags_on;
        for (method, path, answer) in &test.state.answers {
            if let Some(i) = self
                .operations
                .iter()
                .position(|o| &o.method == method && &o.path == path)
            {
                state.answers.insert(i, answer.clone());
            }
        }
        self.state = state.clone();
        self.shots.clear();
        self.compared.clear();
        self.recording.clear();
        self.paths = test.steps.iter().map(|(_, p)| p.clone()).collect();
        self.playing = Some(Playing {
            name: test.name.clone(),
            steps: test.steps.iter().map(|(l, _)| l.clone()).collect(),
            method: test.method.clone(),
            path: test.path.clone(),
            acted: false,
            replay: Some(index),
        });
        // Shown at the size it was kept at; the window's own is asked for
        // again afterwards.
        self.size = None;
        let steps = test
            .steps
            .iter()
            .enumerate()
            .map(|(i, (label, path))| Step {
                label: label.clone(),
                path: path.clone(),
                kept: Some(kept.step_picture(i)),
            })
            .collect();
        let after = kept.after_picture();
        self.replaying = true;
        self.replay_after = after.exists() || !test.inputs.is_empty();
        Some(AppOrder::Replay {
            state,
            play: Play {
                steps,
                inputs: test.inputs.clone(),
                after: after.exists().then_some(after),
                size: Some(test.size),
            },
        })
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
            tests: std::mem::take(&mut self.tests),
            by_page: std::mem::take(&mut self.by_page),
            // Its first page, as it last showed, while it is built.
            target: Some(config.start.clone()),
            loading: Some(std::time::Instant::now()),
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
        if self.replaying {
            return None;
        }
        let size = (width.max(200), height.max(150), scale);
        if self.size == Some(size) {
            return None;
        }
        self.size = Some(size);
        Some(AppOrder::Size(size.0, size.1, size.2))
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
        self.shots.clear();
        self.compared.clear();
        self.recording.clear();
        self.last_input = None;
        let steps = playing
            .steps
            .iter()
            .zip(&paths)
            .map(|(label, path)| Step {
                label: label.clone(),
                path: path.clone(),
                kept: None,
            })
            .collect();
        self.paths = paths;
        self.playing = Some(playing);
        AppOrder::Play(Play {
            steps,
            ..Play::default()
        })
    }

    /// Applies what the preview told; a fixture kept is the file to open.
    pub(crate) fn told(&mut self, event: Event) -> Option<PathBuf> {
        match event {
            Event::Status(status) => {
                if !matches!(status, Status::Running { .. }) {
                    self.focused = false;
                }
                let (kind, said) = match &status {
                    Status::Reading => (ActivityKind::Step, "Reading the spec".to_owned()),
                    Status::Starting { api, .. } => (
                        ActivityKind::Step,
                        format!("API simulated on port {api}; starting the front end"),
                    ),
                    Status::Opening => (
                        ActivityKind::Step,
                        "The front end answers: opening it in the browser".to_owned(),
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
                self.unsettled = true;
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
            Event::Frame(frame) => {
                if !blank(&frame) {
                    // It shows the page: what it loaded is drawn.
                    if self.target.is_some() && self.loading.is_none() {
                        self.target = None;
                    }
                    self.good = Some(frame.clone());
                }
                self.frame = Some(frame);
            }
            // A page that loads: its server's calls come before it says
            // where it is, however long it takes to build.
            Event::Loading => {
                // What the page before last showed, kept as it goes.
                if self.loading.is_none() {
                    self.remember();
                }
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
                    self.unsettled |= within;
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
                // Its frame, to show again while it loads next time, in
                // this session and the next.
                self.remember();
                self.unsettled = false;
            }
            // Shown a moment unchanged since it changed without loading.
            Event::Still => {
                if self.unsettled && self.loading.is_none() {
                    self.remember();
                    self.unsettled = false;
                }
            }
            Event::Console(error) => {
                let first = error.lines().next().unwrap_or_default().to_owned();
                self.log(ActivityKind::Failed, first);
                self.errors_seen += 1;
                push(&mut self.errors, (self.errors_seen, error), KEPT_ERRORS);
            }
            Event::Shot { label, png } => {
                let stepped = self
                    .playing
                    .as_ref()
                    .is_some_and(|p| self.shots.len() < p.steps.len());
                let after = label == "after";
                if stepped || after {
                    self.shots.push(Shot { label, png });
                }
                // Shown again whole: the page may take the window's size.
                let steps = self.playing.as_ref().map_or(0, |p| p.steps.len());
                if after || (!self.replay_after && self.shots.len() >= steps) {
                    self.replaying = false;
                }
            }
            Event::Compared { label, changed } => {
                let (kind, said) = match changed {
                    Some(c) if c < 0.01 => {
                        (ActivityKind::Done, format!("{label}: the same as kept"))
                    }
                    Some(c) => (
                        ActivityKind::Failed,
                        format!("{label}: {:.0}% changed from what was kept", c * 100.0),
                    ),
                    None => (
                        ActivityKind::Step,
                        format!("{label}: no picture to compare"),
                    ),
                };
                self.log(kind, said);
                self.compared.push((label, changed));
            }
            Event::Kept(path) => {
                self.log(
                    ActivityKind::Done,
                    "Its answer kept as a fixture".to_owned(),
                );
                self.kept = Some(path.clone());
                return Some(path);
            }
            Event::Pages(pages) => {
                let said = match pages.len() {
                    0 => None,
                    1 => Some("A page kept from before, shown as it loads".to_owned()),
                    n => Some(format!("{n} pages kept from before, shown as they load")),
                };
                if let Some(said) = said {
                    self.log(ActivityKind::Step, said);
                }
                // Those seen meanwhile are newer.
                for (key, frame) in pages.into_iter().rev() {
                    if self.by_page.len() < KEPT_PAGES
                        && !self.by_page.iter().any(|(k, _)| *k == key)
                    {
                        self.by_page.push_front((key, frame));
                    }
                }
            }
            Event::Gone => {
                self.status = Some(Status::Failed("The browser went away".to_owned()));
                self.focused = false;
            }
        }
        None
    }
}

/// Whether a frame shows nothing: every point of a grid over it the same
/// colour, as the browser sends between two pages and while one is
/// built.
fn blank(frame: &Frame) -> bool {
    let (width, height) = (frame.width as usize, frame.height as usize);
    if width < 16 || height < 16 {
        return true;
    }
    let at = |x: usize, y: usize| {
        let i = (y * width + x) * 4;
        frame.rgba.get(i..i + 3)
    };
    let Some(first) = at(width / 2, height / 2) else {
        return true;
    };
    (0..10).all(|row| {
        (0..10).all(|column| {
            at(column * (width - 1) / 9, row * (height - 1) / 9)
                .is_some_and(|p| p.iter().zip(first).all(|(a, b)| a.abs_diff(*b) < 8))
        })
    })
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
                replay: None,
            },
            vec!["/a".to_owned(), "/b".to_owned()],
        );
        assert_eq!(
            order,
            AppOrder::Play(Play {
                steps: vec![
                    Step {
                        label: "1".to_owned(),
                        path: "/a".to_owned(),
                        kept: None
                    },
                    Step {
                        label: "2".to_owned(),
                        path: "/b".to_owned(),
                        kept: None
                    },
                ],
                ..Play::default()
            })
        );
        preview.told(Event::Call(call(1, 0, None)));
        assert!(!preview.playing().unwrap().acted);
        preview.told(Event::Call(call(2, 10, Some(0))));
        assert!(preview.playing().unwrap().acted);
    }

    #[test]
    fn a_workflow_is_kept_as_a_test_and_shown_again() {
        let mut preview = AppPreview::default();
        preview.told(Event::Operations(
            universe_preview::Spec::read(
                r#"{"paths": {"/v1/products/": {"post": {}}, "/v1/me": {"get": {}}}}"#,
            )
            .unwrap()
            .operations,
        ));
        preview.told(Event::Status(Status::Running {
            address: "http://127.0.0.1:3000".to_owned(),
        }));
        let _ = preview.size(800, 600, 1.0);
        preview.change(Change::Empty(true));
        // Sorted by path: `GET /v1/me` first.
        preview.change(Change::Answer(0, Some(Answer::Status(403))));
        let playing = Playing {
            name: "Create Product".to_owned(),
            steps: vec!["/products".to_owned()],
            method: "POST".to_owned(),
            path: "/v1/products/".to_owned(),
            acted: false,
            replay: None,
        };
        let _ = preview.play(playing, vec!["/dashboard/demo/products".to_owned()]);
        let press = Pointer::Press {
            x: 1.0,
            y: 2.0,
            button: universe_preview::Button::Left,
            clicks: 1,
        };
        // What is done before the screens are shown is not kept.
        preview.record(&AppOrder::Pointer(press));
        assert!(preview.keep_test().is_none());
        preview.told(Event::Shot {
            label: "/products".to_owned(),
            png: b"png".to_vec(),
        });
        assert!(preview.shown_whole());
        preview.record(&AppOrder::Pointer(press));
        preview.record(&AppOrder::Text("Pro".to_owned()));
        preview.record(&AppOrder::Reload);
        preview.told(Event::Call(call(1, 0, Some(1))));
        let Some(AppOrder::KeepTest { test, shots, .. }) = preview.keep_test() else {
            panic!("not kept");
        };
        assert_eq!(
            test.steps,
            [(
                "/products".to_owned(),
                "/dashboard/demo/products".to_owned()
            )]
        );
        assert_eq!(test.inputs.len(), 2);
        assert!(test.acted);
        assert!(test.state.empty);
        assert_eq!(
            test.state.answers,
            [("GET".to_owned(), "/v1/me".to_owned(), Answer::Status(403))]
        );
        assert_eq!(test.size, (800, 600, 1.0));
        assert_eq!(shots, [b"png".to_vec()]);

        // Shown again: its state found again by method and path, its
        // pictures to compare with, the page's size kept meanwhile.
        preview.change(Change::Reset);
        preview.tests_kept(vec![Kept {
            file: PathBuf::from("/kept/create-product.json"),
            test: *test,
        }]);
        let Some(AppOrder::Replay { state, play }) = preview.replay(0) else {
            panic!("not shown again");
        };
        assert!(state.tone.empty);
        assert_eq!(state.answers[&0], Answer::Status(403));
        assert_eq!(
            play.steps[0].kept.as_deref(),
            Some(std::path::Path::new("/kept/create-product/1.png"))
        );
        assert_eq!(play.inputs.len(), 2);
        assert_eq!(play.size, Some((800, 600, 1.0)));
        assert!(preview.size(900, 700, 1.0).is_none());
        preview.told(Event::Compared {
            label: "/products".to_owned(),
            changed: Some(0.0),
        });
        preview.told(Event::Shot {
            label: "after".to_owned(),
            png: Vec::new(),
        });
        assert_eq!(preview.compared().len(), 1);
        assert!(preview.size(900, 700, 1.0).is_some());
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

    /// A frame of 32 by 32 pixels: one colour, or a page that shows
    /// something.
    fn frame(seq: u64, plain: bool) -> Frame {
        let rgba = (0..32 * 32)
            .flat_map(|i: u32| {
                let shade = if plain { 0 } else { (i * 7 % 251) as u8 };
                [shade, shade, shade, 255]
            })
            .collect();
        Frame {
            width: 32,
            height: 32,
            rgba,
            seq,
        }
    }

    #[test]
    fn a_page_shows_as_it_last_showed_while_it_loads_again() {
        let address = |path: &str, within| Event::Address {
            url: format!("http://127.0.0.1:3000{path}"),
            within,
        };
        let shown = |preview: &AppPreview| match preview.shown() {
            Shown::Live(f) => format!("live {}", f.seq),
            Shown::Stale(f) => format!("stale {}", f.seq),
            Shown::Waiting => "waiting".to_owned(),
        };
        let mut preview = AppPreview::default();
        preview.told(Event::Loading);
        preview.told(Event::Frame(frame(1, true)));
        assert_eq!(shown(&preview), "waiting");
        preview.told(address("/products", false));
        preview.told(Event::Frame(frame(2, false)));
        assert_eq!(shown(&preview), "live 2");
        preview.told(Event::Quiet);
        let kept = preview.take_pages_to_keep();
        let keys: Vec<&str> = kept.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["/products|00000|"]);
        assert!(preview.take_pages_to_keep().is_empty());
        assert!(
            preview
                .activity()
                .any(|a| a.text.starts_with("/products loaded in"))
        );

        // Another page, never shown: the last frame, between two pages.
        preview.going("/orders");
        preview.told(Event::Loading);
        preview.told(Event::Frame(frame(3, true)));
        assert_eq!(shown(&preview), "stale 2");
        preview.told(address("/orders", false));
        preview.told(Event::Frame(frame(4, false)));
        preview.told(Event::Quiet);
        // Back to the first: its own frame, not the last.
        preview.going("/products?tab=all");
        preview.told(Event::Loading);
        preview.told(Event::Frame(frame(5, true)));
        assert_eq!(shown(&preview), "stale 2");
        preview.told(address("/products", false));
        preview.told(Event::Frame(frame(6, false)));
        preview.told(Event::Quiet);

        // Routed within, it is kept once it shows still, and loads no more
        // although the browser said it started.
        preview.told(Event::Loading);
        preview.told(address("/customers", true));
        assert!(preview.loading_for().is_none());
        preview.told(Event::Frame(frame(7, false)));
        preview.told(Event::Still);
        let kept = preview.take_pages_to_keep();
        let keys: Vec<&str> = kept.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            ["/orders|00000|", "/products|00000|", "/customers|00000|"]
        );
        assert_eq!(kept[2].1.seq, 7);
        // Still again, unchanged: nothing more to keep.
        preview.told(Event::Still);
        assert!(preview.take_pages_to_keep().is_empty());

        // In another state, its own key; the page in the first meanwhile.
        preview.change(Change::Empty(true));
        preview.going("/orders");
        preview.told(Event::Loading);
        preview.told(Event::Frame(frame(9, true)));
        assert_eq!(shown(&preview), "stale 4");
        preview.told(address("/orders", false));
        preview.told(Event::Frame(frame(8, false)));
        preview.told(Event::Quiet);
        let kept = preview.take_pages_to_keep();
        assert_eq!(kept.last().map(|(k, _)| k.as_str()), Some("/orders|10000|"));
    }

    #[test]
    fn pages_kept_before_show_while_they_load() {
        let mut preview = AppPreview::default();
        let mut before = frame(u64::MAX, false);
        before.rgba[0] = 1;
        preview.told(Event::Pages(vec![("/products|00000|".to_owned(), before)]));
        preview.going("/products");
        assert!(matches!(preview.shown(), Shown::Stale(f) if f.seq == u64::MAX));
        // Kept already: not given to keep again.
        assert!(preview.take_pages_to_keep().is_empty());
    }
}
