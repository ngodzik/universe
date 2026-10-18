//! A project's front end run as its developers run it, against a back end
//! simulated from the API's spec, in a headless browser driven over the
//! DevTools protocol: what its page shows is sent as frames, what the
//! person does on them is sent back, and every call the page makes is told
//! with the operation it reached and where its answer came from.
//!
//! Nothing here asks a model. The answers are made from the spec's
//! schemas, the same for the same place, bent to a state on demand (lists
//! empty, rights withheld, flags on, slow, failing, a status of the spec's
//! for one operation), or read from a fixture kept to be changed by hand.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod browser;
mod dev;
mod error;
mod kept;
mod leftovers;
mod made;
mod mock;
mod pages;
mod spec;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

pub use browser::{Button, Input, Key, Modifiers, Play, Pointer, Recorded, Step, browser_found};
pub use dev::{Framework, Run, command_line, command_shown, free_port};
pub use error::PreviewError;
pub use kept::{Kept, KeptState, KeptTest, difference, keep_test, kept_tests, png_of};
pub use leftovers::{leftovers_file, stop_leftovers};
pub use made::Tone;
pub use mock::{Answer, Call, SLOW, Source, State, fixture_name};
pub use pages::{keep_page, kept_pages};
pub use spec::{Operation, Spec};

use browser::{Browser, Request};
use dev::DevServer;
use mock::Mock;

/// Where the spec the back end is simulated from comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecSource {
    /// Its text, read already.
    Text(String),
    /// A file.
    File(PathBuf),
    /// A command that prints it, run in the project's root.
    Command(String),
}

/// How a preview runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The project's root, where a command that prints the spec runs.
    pub root: PathBuf,
    /// How the front end runs.
    pub run: Run,
    /// The variable of the environment it reads its API's address from.
    pub api_env: Option<String>,
    /// The spec of its API.
    pub spec: SpecSource,
    /// The folder of the operations' fixtures.
    pub fixtures: Option<PathBuf>,
    /// The cookies set before its first page.
    pub cookies: Vec<(String, String)>,
    /// The path of its first page: `/dashboard/demo`.
    pub start: String,
    /// Its page's size in its pixels, and the scale it is drawn at.
    pub size: (u32, u32, f32),
    /// Where what it starts is written down, to be stopped at the next
    /// start if it is left running.
    pub leftovers: Option<PathBuf>,
    /// The folder its workflows are kept in as tests.
    pub tests: Option<PathBuf>,
    /// The folder the last frame of each page is kept in, between
    /// sessions.
    pub pages: Option<PathBuf>,
}

/// Where a preview is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Its spec is being read.
    Reading,
    /// Its back end is simulated on this port, the front end starting with
    /// this line.
    Starting {
        /// The simulated back end's port.
        api: u16,
        /// What runs the front end.
        line: String,
    },
    /// The front end answers; the browser is opening it.
    Opening,
    /// Its page shows.
    Running {
        /// The front end's address.
        address: String,
    },
    /// It stopped, and why.
    Failed(String),
    /// It was stopped.
    Stopped,
}

/// A frame of the page, as it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Its width, in pixels.
    pub width: u32,
    /// Its height, in pixels.
    pub height: u32,
    /// Its pixels, four bytes each, a row after another.
    pub rgba: Vec<u8>,
    /// Its place among the frames, from 1.
    pub seq: u64,
}

/// Something the preview tells.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Where it is.
    Status(Status),
    /// The operations of the spec, once read.
    Operations(Vec<Operation>),
    /// A line the front end's server printed.
    Said(String),
    /// A call the back end answered.
    Call(Call),
    /// A frame of the page.
    Frame(Frame),
    /// A page started loading: the calls from now on are its.
    Loading,
    /// The page's address changed: a page loaded, or `within` the one
    /// shown, as an app that routes itself does.
    Address {
        /// The address.
        url: String,
        /// Whether the page stayed the same document.
        within: bool,
    },
    /// The page's network went quiet: it is loaded.
    Quiet,
    /// The page has shown the same frame for a moment.
    Still,
    /// An error the page met.
    Console(String),
    /// A photograph compared with the one kept: the share of its pixels
    /// that changed, `None` when either is no picture.
    Compared {
        /// The step it was taken at, or `after`.
        label: String,
        /// The share changed, from 0 to 1.
        changed: Option<f32>,
    },
    /// A photograph of the page, as asked.
    Shot {
        /// What it was asked under.
        label: String,
        /// Its picture, as PNG.
        png: Vec<u8>,
    },
    /// An operation's answer kept as a fixture, in this file.
    Kept(PathBuf),
    /// The browser went away.
    Gone,
    /// The pages kept from before, the latest last: each its key and its
    /// last frame.
    Pages(Vec<(String, Frame)>),
}

/// What a preview tells its events to.
pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// What a running preview is asked.
enum Order {
    Browser(Request),
    Change(Box<dyn FnOnce(&mut State) + Send>),
    Keep(usize),
    KeepPage(String, Box<Frame>),
}

/// A preview, starting or running, stopped when dropped.
pub struct Preview {
    orders: Option<Sender<Order>>,
    thread: Option<JoinHandle<()>>,
}

impl Preview {
    /// Starts a preview as `config` says, telling `sink` all it does. It
    /// starts on a thread of its own: this returns at once.
    #[must_use]
    pub fn start(config: Config, sink: Sink) -> Self {
        let (orders, taken) = mpsc::channel();
        let thread = std::thread::spawn(move || run(config, &sink, &taken));
        Self {
            orders: Some(orders),
            thread: Some(thread),
        }
    }

    fn order(&self, order: Order) {
        if let Some(orders) = &self.orders {
            let _ = orders.send(order);
        }
    }

    /// Shows the page at `address`, a path of the front end's or a whole
    /// address.
    pub fn go(&self, address: &str) {
        self.order(Order::Browser(Request::Go(address.to_owned())));
    }

    /// Loads the page again.
    pub fn reload(&self) {
        self.order(Order::Browser(Request::Reload));
    }

    /// Goes back in the page's history, or forward.
    pub fn history(&self, back: bool) {
        self.order(Order::Browser(Request::History(back)));
    }

    /// What the person's pointer does on the page.
    pub fn pointer(&self, pointer: Pointer) {
        self.order(Order::Browser(Request::Pointer(pointer)));
    }

    /// A key pressed on the page.
    pub fn key(&self, key: Key) {
        self.order(Order::Browser(Request::Key(key)));
    }

    /// Text typed on the page.
    pub fn text(&self, text: &str) {
        self.order(Order::Browser(Request::Text(text.to_owned())));
    }

    /// Sizes the page, in its pixels, drawn at `scale`.
    pub fn size(&self, width: u32, height: u32, scale: f32) {
        self.order(Order::Browser(Request::Size(width, height, scale)));
    }

    /// Photographs the page under `label`.
    pub fn shoot(&self, label: &str) {
        self.order(Order::Browser(Request::Shot(label.to_owned())));
    }

    /// Shows a workflow: its steps in turn, each photographed once loaded,
    /// then what the person did on the last done again.
    pub fn play(&self, play: Play) {
        self.order(Order::Browser(Request::Play(play)));
    }

    /// Changes the state the back end answers in.
    pub fn change(&self, change: impl FnOnce(&mut State) + Send + 'static) {
        self.order(Order::Change(Box::new(change)));
    }

    /// Keeps what operation `index` last answered as its fixture.
    pub fn keep(&self, index: usize) {
        self.order(Order::Keep(index));
    }

    /// Keeps `frame` as the last of page `key`, to show at once as the
    /// page loads in a later session.
    pub fn keep_page(&self, key: &str, frame: Frame) {
        self.order(Order::KeepPage(key.to_owned(), Box::new(frame)));
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        // Its thread stops once it has no one to take orders from.
        self.orders = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Runs a preview until its orders end.
fn run(config: Config, sink: &Sink, taken: &Receiver<Order>) {
    if let Some(file) = &config.leftovers {
        stop_leftovers(file);
    }
    let record = leftovers::Record::new(config.leftovers.clone());
    let pages = config.pages.clone().map(|folder| pages_kept(folder, sink));
    let parts = match start(&config, sink, &record) {
        Ok(parts) => parts,
        Err(error) => {
            record.clear();
            sink(Event::Status(Status::Failed(said(&error))));
            // Orders are taken until the preview is dropped, unanswered.
            while taken.recv().is_ok() {}
            return;
        }
    };
    let (mock, server, browser) = parts;
    while let Ok(order) = taken.recv() {
        match order {
            Order::Browser(request) => browser.ask(match request {
                Request::Go(path) if path.starts_with('/') => {
                    Request::Go(format!("http://127.0.0.1:{}{path}", server.port()))
                }
                Request::Play(mut play) => {
                    for step in &mut play.steps {
                        step.path = format!("http://127.0.0.1:{}{}", server.port(), step.path);
                    }
                    Request::Play(play)
                }
                other => other,
            }),
            Order::Change(change) => mock.change(change),
            Order::Keep(index) => match mock.keep(index) {
                Ok(path) => sink(Event::Kept(path)),
                Err(error) => sink(Event::Console(said(&error))),
            },
            Order::KeepPage(key, frame) => {
                if let Some(pages) = &pages {
                    let _ = pages.send((key, frame));
                }
            }
        }
    }
    drop(browser);
    drop(server);
    drop(mock);
    record.clear();
    sink(Event::Status(Status::Stopped));
}

/// The pages kept in `folder`, told at once, then kept again as asked, on a
/// thread of their own: a picture takes a moment to read and to write.
fn pages_kept(folder: PathBuf, sink: &Sink) -> Sender<(String, Box<Frame>)> {
    let (keep, asked) = mpsc::channel::<(String, Box<Frame>)>();
    let sink = Arc::clone(sink);
    std::thread::spawn(move || {
        sink(Event::Pages(kept_pages(&folder, SHOWN_PAGES)));
        while let Ok(first) = asked.recv() {
            // Only the latest frame of each page asked meanwhile.
            let mut latest = vec![first];
            latest.extend(asked.try_iter());
            for (i, (key, frame)) in latest.iter().enumerate() {
                if !latest[i + 1..].iter().any(|(k, _)| k == key) {
                    keep_page(&folder, key, frame);
                }
            }
        }
    });
    keep
}

/// How many of the pages kept are shown again: the latest.
const SHOWN_PAGES: usize = 12;

/// Starts the back end, the front end and the browser, in that order.
fn start(
    config: &Config,
    sink: &Sink,
    record: &leftovers::Record,
) -> Result<(Mock, DevServer, Browser), PreviewError> {
    sink(Event::Status(Status::Reading));
    let text = match &config.spec {
        SpecSource::Text(text) => text.clone(),
        SpecSource::File(path) => {
            std::fs::read_to_string(path).map_err(|source| PreviewError::Read {
                path: path.clone(),
                source,
            })?
        }
        SpecSource::Command(command) => printed_spec(&config.root, command)?,
    };
    let spec = Spec::read(&text)?;
    sink(Event::Operations(spec.operations.clone()));
    let calls = Arc::clone(sink);
    let mock = Mock::start(
        spec,
        config.fixtures.clone(),
        Arc::new(move |call| calls(Event::Call(call))),
    )?;
    let port = free_port()?;
    let api = format!("http://127.0.0.1:{}", mock.port());
    let lines = Arc::clone(sink);
    let mut server = DevServer::start(
        &config.run,
        port,
        config.api_env.as_deref().map(|name| (name, api.as_str())),
        Arc::new(move |line| lines(Event::Said(line))),
    )?;
    record.add(true, server.pid(), server.line());
    sink(Event::Status(Status::Starting {
        api: mock.port(),
        line: server.line().to_owned(),
    }));
    server.ready()?;
    sink(Event::Status(Status::Opening));
    let address = format!("http://127.0.0.1:{port}");
    let browser = Browser::start(
        &format!("{address}{}", config.start),
        &config.cookies,
        config.size,
        Arc::clone(sink),
    )?;
    record.add(false, browser.pid(), &browser.program());
    record.add_profile(browser.profile());
    sink(Event::Status(Status::Running { address }));
    Ok((mock, server, browser))
}

/// The spec a command prints, run in `root`.
fn printed_spec(root: &std::path::Path, command: &str) -> Result<String, PreviewError> {
    let mut shell = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    let output = shell
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|source| PreviewError::Start {
            command: command.to_owned(),
            source,
        })?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() || text.trim().is_empty() {
        let err = String::from_utf8_lossy(&output.stderr);
        let said: Vec<&str> = err.lines().rev().take(6).collect();
        return Err(PreviewError::NoSpec {
            command: command.to_owned(),
            said: said.into_iter().rev().collect::<Vec<_>>().join("\n"),
        });
    }
    Ok(text)
}

/// An error and its causes, as the person reads them.
fn said(error: &PreviewError) -> String {
    let mut text = error.to_string();
    let mut cause = std::error::Error::source(error);
    while let Some(next) = cause {
        text.push_str(": ");
        text.push_str(&next.to_string());
        cause = next.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Waits for an event `wanted` picks out, for a while at most.
    fn wait_for(events: &Arc<Mutex<Vec<Event>>>, wanted: impl Fn(&Event) -> bool) -> Option<Event> {
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(60) {
            if let Some(event) = events.lock().unwrap().iter().find(|e| wanted(e)) {
                return Some(event.clone());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn a_spec_that_cannot_be_read_says_why() {
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let kept = Arc::clone(&events);
        let config = Config {
            root: std::env::temp_dir(),
            run: Run::default(),
            api_env: None,
            spec: SpecSource::Command("echo nothing here >&2; exit 1".to_owned()),
            fixtures: None,
            cookies: Vec::new(),
            start: "/".to_owned(),
            size: (800, 600, 1.0),
            leftovers: None,
            tests: None,
            pages: None,
        };
        let preview = Preview::start(config, Arc::new(move |e| kept.lock().unwrap().push(e)));
        let failed = wait_for(&events, |e| matches!(e, Event::Status(Status::Failed(_))));
        let Some(Event::Status(Status::Failed(why))) = failed else {
            panic!("no failure said");
        };
        assert!(why.contains("printed no spec"), "{why}");
        assert!(why.contains("nothing here"), "{why}");
        drop(preview);
    }

    /// A front end of one page, served by Python's server, its page calling
    /// the simulated back end, seen through a real browser when one is
    /// found here.
    #[cfg(unix)]
    #[test]
    fn a_page_is_seen_and_calls_the_back_end() {
        if !browser_found()
            || std::process::Command::new("python3")
                .arg("--version")
                .output()
                .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!doctype html><body style='background:#0a0'><h1 id=t>wait</h1><script>\nconst api = location.search.slice(1);\nfetch(api + '/v1/me', {credentials: 'include'}).then(r => r.json()).then(j => document.getElementById('t').textContent = j.email);\n</script></body>",
        )
        .unwrap();
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let kept = Arc::clone(&events);
        let config = Config {
            root: dir.path().to_owned(),
            run: Run {
                folder: dir.path().to_owned(),
                command: Some("python3 -m http.server {port} --bind 127.0.0.1".to_owned()),
                ..Run::default()
            },
            api_env: None,
            spec: SpecSource::Text(
                r#"{"paths": {"/v1/me": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"email": {"type": "string"}}}}}}}}}}}"#.to_owned(),
            ),
            fixtures: None,
            cookies: vec![("session".to_owned(), "simulated".to_owned())],
            start: "/index.html".to_owned(),
            size: (640, 400, 1.0),
            leftovers: None,
            tests: None,
            pages: None,
        };
        let preview = Preview::start(config, Arc::new(move |e| kept.lock().unwrap().push(e)));
        let started = wait_for(&events, |e| {
            matches!(
                e,
                Event::Status(Status::Starting { .. } | Status::Failed(_))
            )
        });
        let Some(Event::Status(Status::Starting { api, .. })) = started else {
            panic!("did not start: {started:?}");
        };
        // The page is told where the back end is by its address.
        assert!(
            wait_for(&events, |e| matches!(
                e,
                Event::Status(Status::Running { .. }) | Event::Status(Status::Failed(_))
            ))
            .is_some()
        );
        preview.go(&format!("/index.html?http://127.0.0.1:{api}"));
        let call = wait_for(
            &events,
            |e| matches!(e, Event::Call(c) if c.path == "/v1/me"),
        );
        let Some(Event::Call(call)) = call else {
            panic!("the page never called the back end");
        };
        assert_eq!(
            (call.status, call.source, call.from_page),
            (200, Source::Schema, true)
        );
        // Green, as the page is, once it is drawn.
        let green = |f: &Frame| {
            let middle = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
            f.rgba.len() > middle + 2 && f.rgba[middle + 1] > 100 && f.rgba[middle] < 60
        };
        let frame = wait_for(&events, |e| matches!(e, Event::Frame(f) if green(f)));
        assert!(frame.is_some(), "no frame of the page");
        preview.shoot("page");
        let shot = wait_for(&events, |e| matches!(e, Event::Shot { .. }));
        let Some(Event::Shot { label, png }) = shot else {
            panic!("no photograph");
        };
        assert_eq!(label, "page");
        assert!(png.starts_with(b"\x89PNG"));

        // Shown again as a kept test: its screen compared with the one
        // kept, what the person did done again, its end photographed.
        let kept = dir.path().join("kept.png");
        std::fs::write(&kept, &png).unwrap();
        preview.play(Play {
            steps: vec![Step {
                label: "page".to_owned(),
                path: format!("/index.html?http://127.0.0.1:{api}"),
                kept: Some(kept.clone()),
            }],
            inputs: vec![Recorded {
                after_ms: 100,
                input: Input::Pointer(Pointer::Press {
                    x: 5.0,
                    y: 5.0,
                    button: Button::Left,
                    clicks: 1,
                }),
            }],
            after: Some(kept),
            size: None,
        });
        let compared = wait_for(
            &events,
            |e| matches!(e, Event::Compared { label, .. } if label == "page"),
        );
        let Some(Event::Compared { changed, .. }) = compared else {
            panic!("the step was not compared");
        };
        assert!(changed.is_some_and(|c| c < 0.02), "{changed:?}");
        let after = wait_for(
            &events,
            |e| matches!(e, Event::Compared { label, .. } if label == "after"),
        );
        assert!(after.is_some(), "the end was not photographed");
        drop(preview);
        assert!(
            events
                .lock()
                .unwrap()
                .contains(&Event::Status(Status::Stopped))
        );
    }
}
