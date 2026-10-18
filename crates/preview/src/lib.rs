//! A project's front end run as its developers run it, against a back end
//! simulated from the API's spec, in the system's Chrome: a window of its
//! own that universe places over the App view, driven over the DevTools
//! protocol. The person uses the page there as in any browser; its
//! address, its loads and its errors are told, and every call it makes
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
mod leftovers;
mod made;
mod mock;
mod spec;
mod window;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

pub use browser::{Place, browser_found};
pub use dev::{Framework, Run, command_line, command_shown, free_port};
pub use error::PreviewError;
pub use leftovers::{leftovers_file, stop_leftovers};
pub use made::Tone;
pub use mock::{Answer, Call, SLOW, Source, State, fixture_name};
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
    /// Where what it starts is written down, to be stopped at the next
    /// start if it is left running.
    pub leftovers: Option<PathBuf>,
    /// Whether the browser shows a window, to be placed over universe's;
    /// else it runs headless, as a test's does.
    pub shown: bool,
    /// Universe's own window, on X11, which the browser's is held over.
    pub parent: Option<u64>,
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
    /// Its page is open in the browser.
    Running {
        /// The front end's address.
        address: String,
    },
    /// It stopped, and why.
    Failed(String),
    /// It was stopped.
    Stopped,
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
    /// An error the page met.
    Console(String),
    /// An operation's answer kept as a fixture, in this file.
    Kept(PathBuf),
    /// The browser went away.
    Gone,
}

/// What a preview tells its events to.
pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// What a running preview is asked.
enum Order {
    Browser(Request),
    Change(Box<dyn FnOnce(&mut State) + Send>),
    Keep(usize),
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

    /// Shows the browser's window with its page at `place`, or hides it.
    pub fn place(&self, place: Option<Place>) {
        self.order(Order::Browser(Request::Place(place)));
    }

    /// Shows a workflow: its steps' pages in turn, each once it has
    /// loaded, ending on the last for the person to act on.
    pub fn play(&self, steps: Vec<String>) {
        self.order(Order::Browser(Request::Play(steps)));
    }

    /// Changes the state the back end answers in.
    pub fn change(&self, change: impl FnOnce(&mut State) + Send + 'static) {
        self.order(Order::Change(Box::new(change)));
    }

    /// Keeps what operation `index` last answered as its fixture.
    pub fn keep(&self, index: usize) {
        self.order(Order::Keep(index));
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
                Request::Play(steps) => Request::Play(
                    steps
                        .into_iter()
                        .map(|path| format!("http://127.0.0.1:{}{path}", server.port()))
                        .collect(),
                ),
                other => other,
            }),
            Order::Change(change) => mock.change(change),
            Order::Keep(index) => match mock.keep(index) {
                Ok(path) => sink(Event::Kept(path)),
                Err(error) => sink(Event::Console(said(&error))),
            },
        }
    }
    drop(browser);
    drop(server);
    drop(mock);
    record.clear();
    sink(Event::Status(Status::Stopped));
}

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
        config.shown.then_some(config.parent),
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
            leftovers: None,
            shown: false,
            parent: None,
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
    /// the simulated back end, opened in a real browser, headless, when one
    /// is found here.
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
            leftovers: None,
            shown: false,
            parent: None,
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
        // Shown again as a workflow's step: it loads, and calls again.
        preview.play(vec![format!("/index.html?http://127.0.0.1:{api}")]);
        let again = wait_for(
            &events,
            |e| matches!(e, Event::Call(c) if c.path == "/v1/me" && c.seq > call.seq),
        );
        assert!(again.is_some(), "the step was not shown");
        assert!(
            wait_for(
                &events,
                |e| matches!(e, Event::Address { url, .. } if url.contains("index.html"))
            )
            .is_some()
        );
        drop(preview);
        assert!(
            events
                .lock()
                .unwrap()
                .contains(&Event::Status(Status::Stopped))
        );
    }
}
