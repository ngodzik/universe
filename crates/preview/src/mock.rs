//! The simulated back end: a local server that answers each operation of
//! the spec with its fixture, kept in a folder, else a body made from its
//! schema, in the state asked for, with what the page created, changed
//! and deleted laid over it, then as the behaviour written for it says,
//! when it holds to the schema; and says each call it answers.
//!
//! The calls that tell who is there (a path that ends in `/me`, a list
//! looked through by one name, `?slug=demo`) answer as they would under
//! an empty or failing state too: without them a page shows nothing of
//! what it does when its own data is empty.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::error::PreviewError;
use crate::held::{Gone, Records, Role, done, links, list_in, roles};
use crate::made::{Tone, body, body_as};
use crate::spec::{Spec, decoded};
use crate::written::{Engine, Outcome, Says, Store};
use crate::{Event, Written};

/// How long a slow back end takes over each call.
pub const SLOW: Duration = Duration::from_millis(1500);

/// The most a request may hold, its body included.
const LARGEST_REQUEST: usize = 4 * 1024 * 1024;

/// How an operation answers, chosen for it alone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Answer {
    /// With this status, one the spec declares or a server's failure, and
    /// its body.
    Status(u16),
    /// Its lists empty.
    Empty,
    /// Slowly.
    Slow,
}

/// The state the back end answers in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// How every body is bent.
    pub tone: Tone,
    /// Whether every call is slow.
    pub slow: bool,
    /// Whether every call fails, but those that tell who is there.
    pub failing: bool,
    /// How some operations answer, by their place in the spec's.
    pub answers: HashMap<usize, Answer>,
}

/// Where an answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Made from the operation's schema.
    Schema,
    /// The operation's fixture.
    Fixture,
    /// The state or a choice for the operation.
    Changed,
    /// What the page created, changed or deleted.
    Held,
    /// The behaviour written for the back end.
    Written,
    /// No operation of the spec has that path.
    Missing,
}

/// A call the back end answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// Its place among the calls, from 1.
    pub seq: u64,
    /// When it came, from when the back end started.
    pub at: Duration,
    /// `GET`, `POST`…
    pub method: String,
    /// The path asked for, its query with it.
    pub path: String,
    /// The operation it reached, by its place in the spec's.
    pub operation: Option<usize>,
    /// The status answered.
    pub status: u16,
    /// Where the answer came from.
    pub source: Source,
    /// How long it took.
    pub took: Duration,
    /// Whether a page asked, rather than the front end's server.
    pub from_page: bool,
}

/// What the back end tells: each call it answers, and what it holds once
/// a call changed it.
pub(crate) type Told = Arc<dyn Fn(Event) + Send + Sync>;

/// The simulated back end, serving until dropped.
pub struct Mock {
    port: u16,
    spec: Arc<Spec>,
    state: Arc<Mutex<State>>,
    fixtures: Option<PathBuf>,
    /// The last body each operation answered, to keep as its fixture.
    last: Arc<Mutex<HashMap<usize, Value>>>,
    records: Arc<Mutex<Records>>,
    told: Told,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Mock {
    /// Starts serving `spec` on a free local port, with the fixtures of
    /// `fixtures` and the behaviour written in `program` when given,
    /// telling `told` each call and what it holds.
    ///
    /// # Errors
    ///
    /// [`PreviewError::Listen`] when no local port can be listened on.
    pub fn start(
        spec: Spec,
        fixtures: Option<PathBuf>,
        program: Option<PathBuf>,
        told: Told,
    ) -> Result<Self, PreviewError> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(PreviewError::Listen)?;
        let port = listener.local_addr().map_err(PreviewError::Listen)?.port();
        let served = Served::new(Arc::new(spec), fixtures.clone(), told.clone()).written(program);
        let (spec, state, last, records) = (
            Arc::clone(&served.spec),
            Arc::clone(&served.state),
            Arc::clone(&served.last),
            Arc::clone(&served.records),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopping.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else {
                    continue;
                };
                let served = served.clone();
                std::thread::spawn(move || served.answer(stream));
            }
        });
        Ok(Self {
            port,
            spec,
            state,
            fixtures,
            last,
            records,
            told,
            stop,
            thread: Some(thread),
        })
    }

    /// The port it serves on, on `127.0.0.1`.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Changes the state it answers in: the lists made otherwise from
    /// now on, their items read so everywhere.
    pub fn change(&self, change: impl FnOnce(&mut State)) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut state);
        self.records
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unsee();
    }

    /// Forgets what the page created, changed and deleted: every answer as
    /// the spec makes it again.
    pub fn forget(&self) {
        let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        records.forget();
        (self.told)(Event::Held(records.held()));
    }

    /// Keeps the body operation `index` last answered as its fixture, to
    /// be changed by hand and answered from then on; or, before it ever
    /// answered, one made from its schema.
    ///
    /// # Errors
    ///
    /// [`PreviewError::Write`] when the fixture cannot be written, and when
    /// there is no folder of fixtures.
    pub fn keep(&self, index: usize) -> Result<PathBuf, PreviewError> {
        let Some(operation) = self.spec.operations.get(index) else {
            return Err(PreviewError::NoPaths);
        };
        let Some(folder) = &self.fixtures else {
            return Err(PreviewError::Write {
                path: PathBuf::from("fixtures"),
                source: std::io::Error::other("no folder of fixtures"),
            });
        };
        let kept = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&index)
            .cloned();
        let made = kept.unwrap_or_else(|| {
            let status = success(&operation.statuses);
            self.spec
                .schema(index, &status)
                .map(|schema| {
                    body(
                        &self.spec,
                        schema,
                        &key(operation.method.as_str(), &operation.path, &status),
                        Tone::default(),
                    )
                })
                .unwrap_or(Value::Null)
        });
        let path = folder.join(fixture_name(&operation.method, &operation.path));
        let text = serde_json::to_string_pretty(&made).unwrap_or_default();
        std::fs::create_dir_all(folder)
            .and_then(|()| std::fs::write(&path, text + "\n"))
            .map_err(|source| PreviewError::Write {
                path: path.clone(),
                source,
            })?;
        Ok(path)
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Its accepting waits: a call wakes it to see it is to stop.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Tries `program` as the behaviour of a back end simulated from `spec`,
/// fresh, before it is kept: run, each operation it handles asked once,
/// its parameters those of items listed, its body made from its schema.
/// The operations it handles, as it writes them.
///
/// # Errors
///
/// What is wrong with it: why it does not run, the operations it names
/// that the spec has not, what a handler threw or answered that its
/// schema refuses.
pub(crate) fn tried(
    spec: &Spec,
    fixtures: Option<PathBuf>,
    program: &str,
) -> Result<Vec<String>, Vec<String>> {
    let refused: Arc<Mutex<Vec<String>>> = Arc::default();
    let kept = Arc::clone(&refused);
    let told: Told = Arc::new(move |event| {
        if let Event::Written(Written::Refused { operation, why }) = event {
            kept.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{operation}: {why}"));
        }
    });
    let served = Served::new(Arc::new(spec.clone()), fixtures, told).written(None);
    let (handled, unknown) = served.load(program).map_err(|why| vec![why])?;
    let mut errors: Vec<String> = unknown
        .iter()
        .map(|u| format!("on(\"{u}\"): no operation of the spec has this method and path"))
        .collect();
    let mut asked: Vec<usize> = served
        .program
        .as_ref()
        .map(|p| {
            p.loaded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .handled
                .keys()
                .copied()
                .collect()
        })
        .unwrap_or_default();
    asked.sort_unstable();
    let state = State::default();
    for index in asked {
        let operation = &spec.operations[index];
        let request = Request {
            method: operation.method.clone(),
            path: served.example_path(index),
            origin: None,
            asked_headers: None,
            from_page: false,
            body: spec.request_schema(index).map(|schema| {
                body(
                    spec,
                    schema,
                    &format!("request {} {}", operation.method, operation.path),
                    Tone::default(),
                )
            }),
        };
        let (code, value, source) = served.answer_operation(index, &request, &state);
        served.bent(index, &request, code, value, source);
    }
    errors.extend(
        refused
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..),
    );
    if errors.is_empty() {
        Ok(handled)
    } else {
        Err(errors)
    }
}

/// The name of an operation's fixture in its folder:
/// `get_v1_products_{id}.json`.
#[must_use]
pub fn fixture_name(method: &str, path: &str) -> String {
    let path: String = path
        .split('/')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    format!("{}_{path}.json", method.to_lowercase())
}

/// The first success an operation declares: `200` when it says none.
fn success(statuses: &[String]) -> String {
    statuses
        .iter()
        .find(|s| s.starts_with('2'))
        .cloned()
        .unwrap_or_else(|| "200".to_owned())
}

/// The place a body is made for, for it to be the same each time.
fn key(method: &str, path: &str, status: &str) -> String {
    format!("{method} {path} {status}")
}

/// What each connection's thread needs.
#[derive(Clone)]
struct Served {
    spec: Arc<Spec>,
    /// What each operation does to what the back end holds, and the
    /// operation that lists its collection.
    roles: Arc<Vec<(Role, Option<usize>)>>,
    state: Arc<Mutex<State>>,
    fixtures: Option<PathBuf>,
    last: Arc<Mutex<HashMap<usize, Value>>>,
    records: Arc<Mutex<Records>>,
    told: Told,
    started: Instant,
    seq: Arc<AtomicU64>,
    /// The behaviour written for the back end; none for the store it is
    /// given, which answers as the spec and the page made it.
    program: Option<Arc<Program>>,
    /// What checks an operation's answers, by its place and their status,
    /// once made.
    checks: Arc<Mutex<Checks>>,
}

/// What checks the answers of an operation, by its place and their
/// status: none where there is no schema to hold to.
type Checks = HashMap<(usize, u16), Option<Arc<jsonschema::Validator>>>;

/// The behaviour written for the back end: where it is read from, the
/// engine it runs in, and what it handles.
struct Program {
    /// Its file; none for one given as text, to be tried.
    file: Option<PathBuf>,
    engine: Engine,
    loaded: Mutex<Loaded>,
}

#[derive(Default)]
struct Loaded {
    /// The file's size and time of change when it was last read.
    version: Option<(u64, Option<SystemTime>)>,
    /// The operations it handles, by their place in the spec's, as it
    /// writes them.
    handled: HashMap<usize, String>,
}

/// A request, as far as the back end reads it.
struct Request {
    method: String,
    path: String,
    origin: Option<String>,
    asked_headers: Option<String>,
    from_page: bool,
    /// Its body, when it is JSON or a form's fields.
    body: Option<Value>,
}

impl Served {
    fn new(spec: Arc<Spec>, fixtures: Option<PathBuf>, told: Told) -> Self {
        Self {
            roles: Arc::new(roles(&spec)),
            spec,
            state: Arc::default(),
            fixtures,
            last: Arc::default(),
            records: Arc::default(),
            told,
            started: Instant::now(),
            seq: Arc::default(),
            program: None,
            checks: Arc::default(),
        }
    }

    /// It with the behaviour written in `file`, read when it changes; with
    /// an engine and no program when there is none, for one to be tried.
    fn written(self, file: Option<PathBuf>) -> Self {
        // What the program asks of the store is answered as the spec and
        // the page made it: by the back end without its program.
        let asked = self.clone();
        let store: Store =
            Arc::new(move |operation, arguments| asked.stored(operation, &arguments));
        let told = Arc::clone(&self.told);
        let says: Says = Arc::new(move |line| told(Event::Written(Written::Said(line))));
        let program = Program {
            file,
            engine: Engine::start(store, says),
            loaded: Mutex::default(),
        };
        Self {
            program: Some(Arc::new(program)),
            ..self
        }
    }

    /// Reads the program again when its file changed since: removed, the
    /// back end answers as it would without it.
    fn refresh(&self) {
        let Some(program) = &self.program else {
            return;
        };
        let Some(file) = &program.file else {
            return;
        };
        let version = std::fs::metadata(file)
            .ok()
            .map(|m| (m.len(), m.modified().ok()));
        let mut loaded = program
            .loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if loaded.version == version {
            return;
        }
        loaded.version = version;
        drop(loaded);
        let text = version
            .and_then(|_| std::fs::read_to_string(file).ok())
            .unwrap_or_default();
        let _ = self.load(&text);
    }

    /// Runs `text` as the program: the operations it handles, as it writes
    /// them, and those it names that the spec has not.
    fn load(&self, text: &str) -> Result<(Vec<String>, Vec<String>), String> {
        let Some(program) = &self.program else {
            return Err("No engine to run it in".to_owned());
        };
        let named = match program.engine.load(text) {
            Ok(named) => named,
            Err(why) => {
                (self.told)(Event::Written(Written::Broken(why.clone())));
                return Err(why);
            }
        };
        let mut handled = HashMap::new();
        let mut unknown = Vec::new();
        for operation in &named {
            let (method, path) = operation.split_once(' ').unwrap_or(("", operation));
            match self.spec.find(&method.to_uppercase(), path.trim()) {
                Some(index) => {
                    handled.insert(index, operation.clone());
                }
                None => unknown.push(operation.clone()),
            }
        }
        let known: Vec<String> = named.into_iter().filter(|n| !unknown.contains(n)).collect();
        program
            .loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .handled = handled;
        (self.told)(Event::Written(Written::Loaded {
            handled: known.clone(),
            unknown: unknown.clone(),
        }));
        Ok((known, unknown))
    }

    /// The answer as the program bends it, when it handles the operation
    /// and its answer holds to the schema; else the answer as it was, and
    /// why it was not bent told.
    fn bent(
        &self,
        index: usize,
        request: &Request,
        code: u16,
        value: Option<Value>,
        source: Source,
    ) -> (u16, Option<Value>, Source) {
        let Some(program) = &self.program else {
            return (code, value, source);
        };
        let operation = program
            .loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .handled
            .get(&index)
            .cloned();
        let Some(operation) = operation else {
            return (code, value, source);
        };
        let call = json!({
            "method": request.method,
            "path": request.path.split('?').next().unwrap_or(&request.path),
            "params": self.params(index, &request.path),
            "query": query(&request.path)
                .into_iter()
                .map(|(name, value)| (name, Value::String(value)))
                .collect::<serde_json::Map<String, Value>>(),
            "body": request.body,
            "status": code,
            "answer": value,
        });
        let refused = |why: String| {
            (self.told)(Event::Written(Written::Refused {
                operation: operation.clone(),
                why,
            }));
        };
        match program.engine.run(&operation, call) {
            Ok(Outcome::Pass) => (code, value, source),
            Ok(Outcome::Answer { status, body }) => {
                let status = status.unwrap_or(code);
                let made = value.as_ref().filter(|_| status == code);
                match self.refuses(index, status, &body, made) {
                    None => (status, Some(body), Source::Written),
                    Some(why) => {
                        refused(format!(
                            "it answered {status} with a body its schema refuses, so the default was given: {why}"
                        ));
                        (code, value, source)
                    }
                }
            }
            Err(why) => {
                refused(format!("{why}; the default was given"));
                (code, value, source)
            }
        }
    }

    /// The parameters of a path, by the names the operation's path gives
    /// them: `{"id": "42"}` for `/v1/products/42`.
    fn params(&self, index: usize, path: &str) -> serde_json::Map<String, Value> {
        let template = &self.spec.operations[index].path;
        let names = template.split('/').filter(|p| !p.is_empty());
        names
            .zip(self.spec.bound(index, path))
            .filter_map(|(name, value)| {
                let name = name.strip_prefix('{')?.strip_suffix('}')?;
                Some((name.to_owned(), Value::String(value)))
            })
            .collect()
    }

    /// Why the schema of operation `index` for `status` refuses `body`,
    /// when it does; nothing when it holds, or there is no schema to hold
    /// to. What it refuses of the answer made by default too is the spec's
    /// own doing (two kinds of a `oneOf` that both fit), not the program's,
    /// and is let be.
    fn refuses(
        &self,
        index: usize,
        status: u16,
        body: &Value,
        made: Option<&Value>,
    ) -> Option<String> {
        let check = self
            .checks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((index, status))
            .or_insert_with(|| {
                self.spec
                    .validator(index, &status.to_string())
                    .map(Arc::new)
            })
            .clone()?;
        // An error as where it is, its items' places aside, and of what kind.
        let shape = |e: &jsonschema::ValidationError| {
            let at = e.instance_path().to_string();
            let places: Vec<&str> = at
                .split('/')
                .map(|p| if p.parse::<usize>().is_ok() { "#" } else { p })
                .collect();
            let kind = format!("{:?}", e.kind());
            let kind = kind
                .split(['(', ' ', '{'])
                .next()
                .unwrap_or_default()
                .to_owned();
            (places.join("/"), kind)
        };
        let already: std::collections::BTreeSet<(String, String)> = made
            .map(|made| check.iter_errors(made).map(|e| shape(&e)).collect())
            .unwrap_or_default();
        let errors: Vec<String> = check
            .iter_errors(body)
            .filter(|e| !already.contains(&shape(e)))
            .take(3)
            .map(|e| {
                let at = e.instance_path().to_string();
                let said: String = e.to_string().chars().take(200).collect();
                format!("{}: {said}", if at.is_empty() { "/" } else { &at })
            })
            .collect();
        (!errors.is_empty()).then(|| errors.join("; "))
    }

    /// What the program's store does: `get`, `list`, `create`, `change`,
    /// `delete`, as the back end without the program would.
    fn stored(&self, operation: &str, arguments: &[Value]) -> Result<Value, String> {
        let text = |at: usize, what: &str| {
            arguments
                .get(at)
                .and_then(|a| match a {
                    Value::String(text) => Some(text.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .ok_or_else(|| format!("store.{operation} needs {what}"))
        };
        let fields = arguments.get(1).cloned().unwrap_or_else(|| json!({}));
        let changed = |records: &Records| (self.told)(Event::Held(records.held()));
        match operation {
            "get" => {
                let id = text(0, "an id")?;
                let records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
                Ok(records.item(&id).unwrap_or(Value::Null))
            }
            "list" => {
                let path = text(0, "a path such as \"/v1/orders/\"")?;
                let (code, body) = self.respond("GET", &path, None);
                let mut body = body.unwrap_or(Value::Null);
                if !(200..300).contains(&code) {
                    return Err(format!("GET {path} answers {code}"));
                }
                let items = list_in(&mut body).map(std::mem::take);
                Ok(items.map_or(body, Value::Array))
            }
            "create" => {
                let path = text(0, "a path such as \"/v1/orders/\"")?;
                if let Some(index) = self.spec.find("POST", &path)
                    && matches!(self.roles[index].0, Role::Create { .. })
                {
                    let (code, body) = self.respond("POST", &path, Some(fields));
                    return if (200..300).contains(&code) {
                        Ok(body.unwrap_or(Value::Null))
                    } else {
                        Err(format!("POST {path} answers {code}"))
                    };
                }
                let Some(index) = self.spec.find("GET", &path) else {
                    return Err(format!("No operation of the spec lists items at {path}"));
                };
                let (Role::List { param }, list) = &self.roles[index] else {
                    return Err(format!("GET {path} lists no items to create one in"));
                };
                let parts = self.spec.bound(index, &path);
                let key = parts.join("/");
                let kind = list
                    .and_then(|l| self.spec.listed(l))
                    .and_then(Spec::reference)
                    .map_or_else(|| key.clone(), str::to_owned);
                let links = links(self.spec.operations[index].parts(), &parts);
                let template = || self.listed(Some(index), Tone::default()).into_iter().next();
                let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
                let (_, record) =
                    records.create(&key, &kind, param, Some(&fields), None, template, &links);
                changed(&records);
                Ok(record)
            }
            "change" => {
                let id = text(0, "an id")?;
                let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
                records
                    .change_item(&id, &fields)
                    .map_err(|Gone| format!("{id} was deleted"))?;
                changed(&records);
                Ok(records.item(&id).unwrap_or(Value::Null))
            }
            "delete" => {
                let id = text(0, "an id")?;
                let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
                let deleted = records.delete_item(&id).is_ok();
                changed(&records);
                Ok(Value::Bool(deleted))
            }
            other => Err(format!("The store has no {other}")),
        }
    }

    /// What the back end answers a request it makes itself, as it would a
    /// page's, its program aside.
    fn respond(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Option<Value>) {
        let request = Request {
            method: method.to_owned(),
            path: path.to_owned(),
            origin: None,
            asked_headers: None,
            from_page: false,
            body,
        };
        let state = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match self.spec.find(method, path) {
            Some(index) => {
                let (code, value, _) = self.answer_operation(index, &request, &state);
                (code, value)
            }
            None => (404, None),
        }
    }

    /// A path of operation `index` as a page would ask it: each parameter
    /// the id of the first item the list before it shows, else `example`.
    fn example_path(&self, index: usize) -> String {
        let template = self.spec.operations[index].parts();
        let mut asked: Vec<String> = Vec::new();
        for (at, part) in template.iter().enumerate() {
            let value = match part {
                Some(literal) => literal.clone(),
                None => {
                    let listed =
                        self.spec
                            .operations
                            .iter()
                            .position(|o| o.method == "GET" && o.parts() == &template[..at])
                            .and_then(|list| {
                                let path = format!("/{}", asked.join("/"));
                                let (_, body) = self.respond("GET", &path, None);
                                let mut body = body?;
                                let first = list_in(&mut body)?.first()?.clone();
                                let param = match &self.roles[list].0 {
                                    Role::List { param } => param.clone(),
                                    _ => "id".to_owned(),
                                };
                                first.get(&param).or_else(|| first.get("id")).and_then(
                                    |id| match id {
                                        Value::String(id) => Some(id.clone()),
                                        Value::Number(n) => Some(n.to_string()),
                                        _ => None,
                                    },
                                )
                            });
                    listed.unwrap_or_else(|| "example".to_owned())
                }
            };
            asked.push(value);
        }
        format!("/{}", asked.join("/"))
    }

    /// Whether the state forces the answer of operation `index` to
    /// `request`: a status chosen for it, or every call failing.
    fn forced(index: usize, request: &Request, state: &State) -> bool {
        matches!(state.answers.get(&index), Some(Answer::Status(_)))
            || (state.failing && !tells_who(&request.path))
    }

    fn answer(&self, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let begun = Instant::now();
        let Some(request) = read_request(&mut stream) else {
            return;
        };
        if request.method == "OPTIONS" {
            let _ = write_response(&mut stream, 204, &request, None);
            return;
        }
        let state = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let found = self.spec.find(&request.method, &request.path);
        let (status, value, source) = match found {
            None => (
                404,
                Some(json!({"detail": "Not in the spec the back end is simulated from"})),
                Source::Missing,
            ),
            Some(index) => {
                self.refresh();
                let (code, value, source) = self.answer_operation(index, &request, &state);
                if Self::forced(index, &request, &state) {
                    (code, value, source)
                } else {
                    self.bent(index, &request, code, value, source)
                }
            }
        };
        let slow = state.slow || found.and_then(|i| state.answers.get(&i)) == Some(&Answer::Slow);
        if slow {
            std::thread::sleep(SLOW);
        }
        let text = value.as_ref().map(Value::to_string);
        let _ = write_response(&mut stream, status, &request, text.as_deref());
        let _ = stream.shutdown(Shutdown::Both);
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        (self.told)(Event::Call(Call {
            seq,
            at: begun.duration_since(self.started),
            method: request.method,
            path: request.path,
            operation: found,
            status,
            source,
            took: begun.elapsed(),
            from_page: request.from_page,
        }));
    }

    fn answer_operation(
        &self,
        index: usize,
        request: &Request,
        state: &State,
    ) -> (u16, Option<Value>, Source) {
        let operation = &self.spec.operations[index];
        let kept = tells_who(&request.path);
        let chosen = state.answers.get(&index);
        if let Some(Answer::Status(status)) = chosen {
            let schema = self.spec.schema(index, &status.to_string());
            let value = schema.map_or_else(
                || json!({"detail": format!("Simulated {status}")}),
                |schema| {
                    body(
                        &self.spec,
                        schema,
                        &key(&operation.method, &operation.path, &status.to_string()),
                        state.tone,
                    )
                },
            );
            return (*status, Some(value), Source::Changed);
        }
        if state.failing && !kept {
            return (
                500,
                Some(json!({"detail": "Simulated failure"})),
                Source::Changed,
            );
        }
        let code = success(&operation.statuses).parse().unwrap_or(200);
        let tone = Tone {
            empty: (state.tone.empty && !kept) || chosen == Some(&Answer::Empty),
            ..state.tone
        };
        let (value, source) = self.made(index, tone);
        if let Some(value) = &value
            && source != Source::Changed
        {
            self.last
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(index, value.clone());
        }
        self.held(index, request, tone, code, value, source)
    }

    /// What operation `index` answers when it succeeds, bent to `tone`: its
    /// fixture, else a body made from its schema.
    fn made(&self, index: usize, tone: Tone) -> (Option<Value>, Source) {
        let operation = &self.spec.operations[index];
        let status = success(&operation.statuses);
        let fixture = self
            .fixtures
            .as_deref()
            .map(|f| f.join(fixture_name(&operation.method, &operation.path)))
            .and_then(|path| read_fixture(&path));
        if let Some(value) = fixture {
            return (Some(value), Source::Fixture);
        }
        match self.spec.schema(index, &status) {
            Some(schema) => {
                let made = body(
                    &self.spec,
                    schema,
                    &key(&operation.method, &operation.path, &status),
                    tone,
                );
                let changed = tone != Tone::default();
                (
                    Some(made),
                    if changed {
                        Source::Changed
                    } else {
                        Source::Schema
                    },
                )
            }
            None if status == "204" => (None, Source::Schema),
            None => (Some(json!({})), Source::Schema),
        }
    }

    /// The items of the list operation `list` answers, its lists not
    /// emptied: those a page saw before it created any.
    fn listed(&self, list: Option<usize>, tone: Tone) -> Vec<Value> {
        let whole = Tone {
            empty: false,
            ..tone
        };
        list.and_then(|list| self.made(list, whole).0)
            .and_then(|mut body| list_in(&mut body).map(std::mem::take))
            .unwrap_or_default()
    }

    /// The answer with what the back end holds laid over it, and what the
    /// call changes of it: a deleted item answers 404.
    fn held(
        &self,
        index: usize,
        request: &Request,
        tone: Tone,
        code: u16,
        value: Option<Value>,
        source: Source,
    ) -> (u16, Option<Value>, Source) {
        if !(200..300).contains(&code) {
            return (code, value, source);
        }
        let (code, mut value, mut source) = self.done(index, request, tone, code, value, source);
        // Every item an answer holds, at any depth, as it reads now.
        let records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        if (200..300).contains(&code)
            && let Some(value) = &mut value
            && records.lay(value)
        {
            source = Source::Held;
        }
        (code, value, source)
    }

    /// What the call does to what the back end holds, as its operation's
    /// role says, and its answer with it: a deleted item answers 404.
    fn done(
        &self,
        index: usize,
        request: &Request,
        tone: Tone,
        code: u16,
        value: Option<Value>,
        source: Source,
    ) -> (u16, Option<Value>, Source) {
        let (role, list) = &self.roles[index];
        let parts = self.spec.bound(index, &request.path);
        // The collection, and the item's id: the part before the action's
        // name for an action, the last for an item.
        let item = |back: usize| {
            let at = parts.len().saturating_sub(back);
            (
                parts[..at].join("/"),
                parts.get(at).cloned().unwrap_or_default(),
            )
        };
        let (key, id) = match role {
            Role::Read { param: Some(_) }
            | Role::Update { param: Some(_) }
            | Role::Delete { .. } => item(1),
            Role::Act { .. } => item(2),
            Role::Create {
                elsewhere: true, ..
            } => (
                list.map(|l| {
                    self.spec.operations[l]
                        .parts()
                        .iter()
                        .flatten()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .unwrap_or_default(),
                String::new(),
            ),
            _ => (parts.join("/"), String::new()),
        };
        let seeds = || self.listed(*list, tone);
        let template = || self.listed(*list, tone).into_iter().next();
        // The kind of what its list lists, else of what it answers.
        let kind = list
            .and_then(|l| self.spec.listed(l))
            .and_then(Spec::reference)
            .or_else(|| self.spec.success_schema(index).and_then(Spec::reference))
            .map_or_else(|| key.clone(), str::to_owned);
        let links = links(self.spec.operations[index].parts(), &parts);
        let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        let found = match role {
            Role::Other => return (code, value, source),
            Role::List { param } => {
                let Some(mut body) = value else {
                    return (code, None, source);
                };
                let query = query(&request.path);
                let held = records.list(&key, &kind, param, &mut body, template, &query, &links);
                return (code, Some(body), if held { Source::Held } else { source });
            }
            Role::Create { param, .. } => {
                // Of the kind the request names.
                let value = request
                    .body
                    .as_ref()
                    .filter(|_| source != Source::Fixture)
                    .and_then(|given| {
                        let operation = &self.spec.operations[index];
                        let status = success(&operation.statuses);
                        let schema = self.spec.schema(index, &status)?;
                        let place = self::key(&operation.method, &operation.path, &status);
                        Some(body_as(&self.spec, schema, &place, tone, given))
                    })
                    .or(value);
                let (answer, _) = records.create(
                    &key,
                    &kind,
                    param,
                    request.body.as_ref(),
                    value,
                    template,
                    &links,
                );
                (self.told)(Event::Held(records.held()));
                return (code, answer, Source::Held);
            }
            Role::Read { param } => records.read(&key, param.as_deref(), &id, seeds, value),
            Role::Update { param } => {
                let found = records.update(
                    &key,
                    param.as_deref(),
                    &id,
                    request.body.as_ref(),
                    None,
                    seeds,
                    value,
                );
                (self.told)(Event::Held(records.held()));
                found
            }
            Role::Act { param, verb } => {
                let statuses = |field: &str| {
                    self.spec
                        .success_schema(index)
                        .map(|schema| self.spec.enumerated(schema, field))
                        .unwrap_or_default()
                };
                let did = value.as_ref().and_then(|made| done(verb, made, &statuses));
                let found = records.update(
                    &key,
                    Some(param),
                    &id,
                    request.body.as_ref(),
                    did,
                    seeds,
                    value,
                );
                (self.told)(Event::Held(records.held()));
                found
            }
            Role::Delete { .. } => {
                let found = records.delete(&key, &id).map(|()| (value, true));
                (self.told)(Event::Held(records.held()));
                found
            }
        };
        match found {
            Ok((value, true)) => (code, value, Source::Held),
            Ok((value, false)) => (code, value, source),
            Err(Gone) => {
                let operation = &self.spec.operations[index];
                let gone = self.spec.schema(index, "404").map_or_else(
                    || json!({"detail": "Not found: it was deleted"}),
                    |schema| {
                        body(
                            &self.spec,
                            schema,
                            &self::key(&operation.method, &operation.path, "404"),
                            tone,
                        )
                    },
                );
                (404, Some(gone), Source::Held)
            }
        }
    }
}

/// The names and values of a path's query, as they read.
fn query(path: &str) -> Vec<(String, String)> {
    let Some((_, query)) = path.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let read = |text: &str| decoded(&text.replace('+', " "));
            (read(name), read(value))
        })
        .collect()
}

/// Whether a call tells who is there, and so answers as it would under an
/// empty or failing state: a path that ends in `me`, or a list looked
/// through by one name.
fn tells_who(path: &str) -> bool {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let me = path.trim_end_matches('/').ends_with("/me") || path.contains("/me/");
    let by_name = query.split('&').any(|pair| {
        let name = pair.split('=').next().unwrap_or_default();
        ["slug", "id", "email", "handle", "username", "key"].contains(&name)
    });
    me || by_name
}

fn read_fixture(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Reads a request's line, headers and body.
fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut words = line.split_whitespace();
    let method = words.next()?.to_owned();
    let path = words.next()?.to_owned();
    let mut length = 0usize;
    let mut origin = None;
    let mut asked_headers = None;
    let mut from_page = false;
    let mut form = false;
    let mut read = line.len();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            break;
        }
        read += header.len();
        if read > LARGEST_REQUEST {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().unwrap_or(0).min(LARGEST_REQUEST),
            "origin" => origin = Some(value.to_owned()),
            "access-control-request-headers" => asked_headers = Some(value.to_owned()),
            "user-agent" => from_page = value.contains("Mozilla"),
            "content-type" => form = value.contains("x-www-form-urlencoded"),
            _ => {}
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    let body = if form {
        let fields = query(&format!("?{}", String::from_utf8_lossy(&body)));
        Some(Value::Object(
            fields
                .into_iter()
                .map(|(name, value)| (name, Value::String(value)))
                .collect(),
        ))
    } else {
        serde_json::from_slice(&body).ok()
    };
    Some(Request {
        method,
        path,
        origin,
        asked_headers,
        from_page,
        body,
    })
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    request: &Request,
    body: Option<&str>,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
    // Any page of the front end may call, with its cookies.
    if let Some(origin) = &request.origin {
        head.push_str(&format!(
            "Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Allow-Credentials: true\r\nVary: Origin\r\n"
        ));
    }
    head.push_str(&format!(
        "Access-Control-Allow-Methods: GET, POST, PUT, PATCH, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: {}\r\nAccess-Control-Max-Age: 600\r\n",
        request.asked_headers.as_deref().unwrap_or("*")
    ));
    let body = body.unwrap_or_default();
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r##"{"paths": {
        "/v1/me": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"email": {"type": "string"}}}}}}}}},
        "/v1/products/": {
            "get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"items": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}}}}}}}}}},
            "post": {"responses": {"201": {"content": {"application/json": {"schema": {"type": "object", "properties": {"id": {"type": "string"}}}}}}, "422": {"content": {"application/json": {"schema": {"type": "object", "properties": {"detail": {"type": "string"}}}}}}}}}}}"##;

    /// A request to the back end, and its status, headers and body.
    fn ask(port: u16, request: &str) -> (u16, String, Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        let (head, body) = answer.split_once("\r\n\r\n").unwrap();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(body).unwrap()
        };
        (status, head.to_owned(), body)
    }

    fn get(port: u16, path: &str) -> (u16, Value) {
        let (status, _, body) = ask(
            port,
            &format!(
                "GET {path} HTTP/1.1\r\nHost: x\r\nOrigin: http://127.0.0.1:3000\r\nUser-Agent: Mozilla/5.0\r\n\r\n"
            ),
        );
        (status, body)
    }

    #[test]
    fn the_back_end_answers_each_operation_in_the_state_asked() {
        let dir = tempfile::tempdir().unwrap();
        let said: Arc<Mutex<Vec<Call>>> = Arc::default();
        let kept = Arc::clone(&said);
        let mock = Mock::start(
            Spec::read(SPEC).unwrap(),
            Some(dir.path().to_owned()),
            None,
            Arc::new(move |event| {
                if let Event::Call(call) = event {
                    kept.lock().unwrap().push(call);
                }
            }),
        )
        .unwrap();
        let port = mock.port();

        let (status, products) = get(port, "/v1/products/");
        assert_eq!(status, 200);
        assert_eq!(products["items"].as_array().unwrap().len(), 3);
        assert_eq!(get(port, "/v1/products/"), (200, products.clone()));
        assert_eq!(get(port, "/v1/orders/").0, 404);
        // A page's preflight, answered for its origin and its headers.
        let (status, head, _) = ask(
            port,
            "OPTIONS /v1/products/ HTTP/1.1\r\nOrigin: http://127.0.0.1:3000\r\nAccess-Control-Request-Headers: x-client\r\n\r\n",
        );
        assert_eq!(status, 204);
        assert!(head.contains("Access-Control-Allow-Origin: http://127.0.0.1:3000"));
        assert!(head.contains("Access-Control-Allow-Headers: x-client"));
        let (status, _, created) = ask(
            port,
            "POST /v1/products/ HTTP/1.1\r\nContent-Length: 13\r\n\r\n{\"name\":\"x\"}\n",
        );
        assert_eq!(status, 201);
        assert!(created["id"].is_string());
        // What it created, forgotten: the rest as the spec makes it.
        mock.forget();

        // Empty, but who is there still answers.
        mock.change(|s| s.tone.empty = true);
        assert_eq!(get(port, "/v1/products/").1["items"], json!([]));
        assert_eq!(
            get(port, "/v1/products/?slug=demo").1["items"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        mock.change(|s| {
            s.tone.empty = false;
            s.failing = true;
        });
        assert_eq!(get(port, "/v1/products/").0, 500);
        assert_eq!(get(port, "/v1/me").0, 200);
        mock.change(|s| {
            s.failing = false;
            s.answers.insert(2, Answer::Status(422));
        });
        let (status, _, refused) = ask(port, "POST /v1/products/ HTTP/1.1\r\n\r\n");
        assert_eq!(status, 422);
        assert!(refused["detail"].is_string());
        mock.change(|s| s.answers.clear());

        // Kept as a fixture, changed by hand, answered from then on.
        let fixture = mock.keep(1).unwrap();
        assert_eq!(fixture.file_name().unwrap(), "get_v1_products.json");
        std::fs::write(&fixture, r#"{"items": [{"id": "1", "name": "Kept"}]}"#).unwrap();
        assert_eq!(get(port, "/v1/products/").1["items"][0]["name"], "Kept");

        let calls = said.lock().unwrap();
        assert!(calls.len() >= 10);
        assert_eq!(calls[0].seq, 1);
        assert_eq!(calls[0].operation, Some(1));
        assert!(calls[0].from_page);
        assert!(
            calls
                .iter()
                .any(|c| c.source == Source::Missing && c.status == 404)
        );
        assert!(calls.iter().any(|c| c.source == Source::Fixture));
    }

    fn send(port: u16, method: &str, path: &str, body: &str) -> (u16, Value) {
        let (status, _, answer) = ask(
            port,
            &format!(
                "{method} {path} HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        (status, answer)
    }

    #[test]
    fn what_a_page_creates_changes_and_deletes_is_held_until_forgotten() {
        let spec = r##"{"paths": {
            "/v1/products/": {
                "get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {
                    "items": {"type": "array", "items": {"$ref": "#/components/schemas/Product"}},
                    "pagination": {"type": "object", "properties": {"total_count": {"type": "integer"}, "max_page": {"type": "integer"}}}}}}}}}},
                "post": {"responses": {"201": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Product"}}}}}}},
            "/v1/products/{id}": {
                "get": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Product"}}}}}},
                "patch": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Product"}}}}}},
                "delete": {"responses": {"204": {"description": "gone"}}}}},
            "components": {"schemas": {"Product": {"type": "object", "properties": {
                "id": {"type": "string", "format": "uuid"}, "name": {"type": "string"}, "is_archived": {"type": "boolean"}}}}}}"##;
        let held: Arc<Mutex<Vec<Vec<crate::Held>>>> = Arc::default();
        let kept = Arc::clone(&held);
        let mock = Mock::start(
            Spec::read(spec).unwrap(),
            None,
            None,
            Arc::new(move |event| {
                if let Event::Held(now) = event {
                    kept.lock().unwrap().push(now);
                }
            }),
        )
        .unwrap();
        let port = mock.port();
        let (_, before) = get(port, "/v1/products/");
        let seeds = before["items"].as_array().unwrap().clone();
        assert_eq!(seeds.len(), 3);

        let (status, created) = send(port, "POST", "/v1/products/", r#"{"name": "Mine"}"#);
        assert_eq!(status, 201);
        assert_eq!(created["name"], "Mine");
        let id = created["id"].as_str().unwrap().to_owned();
        assert!(seeds.iter().all(|s| s["id"] != id.as_str()));
        let (_, listed) = get(port, "/v1/products/");
        assert_eq!(listed["items"][0], created);
        assert_eq!(listed["pagination"]["total_count"], 4);
        assert_eq!(get(port, &format!("/v1/products/{id}")).1, created);

        // A listed item reads as it was listed, and changes.
        let second = seeds[1]["id"].as_str().unwrap();
        assert_eq!(get(port, &format!("/v1/products/{second}")).1, seeds[1]);
        let (status, archived) = send(
            port,
            "PATCH",
            &format!("/v1/products/{second}"),
            r#"{"is_archived": true}"#,
        );
        assert_eq!(
            (status, archived["is_archived"].clone()),
            (200, json!(true))
        );
        let (_, unarchived) = get(port, "/v1/products/?is_archived=false");
        assert_eq!(unarchived["items"].as_array().unwrap().len(), 3);
        assert_eq!(unarchived["pagination"]["total_count"], 3);

        let (status, _) = send(port, "DELETE", &format!("/v1/products/{id}"), "");
        assert_eq!(status, 204);
        assert_eq!(get(port, &format!("/v1/products/{id}")).0, 404);
        assert_eq!(
            get(port, "/v1/products/").1["items"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            held.lock().unwrap().last().unwrap(),
            &[crate::Held {
                path: "/v1/products".to_owned(),
                created: 0,
                changed: 1,
                deleted: 1,
            }]
        );

        // Under an empty state, what the page created still shows.
        send(port, "POST", "/v1/products/", r#"{"name": "Alone"}"#);
        mock.change(|s| s.tone.empty = true);
        let (_, alone) = get(port, "/v1/products/");
        assert_eq!(alone["items"].as_array().unwrap().len(), 1);
        assert_eq!(alone["items"][0]["name"], "Alone");
        assert_eq!(alone["pagination"]["total_count"], 1);
        mock.change(|s| s.tone.empty = false);

        mock.forget();
        assert_eq!(get(port, "/v1/products/").1, before);
        assert!(held.lock().unwrap().last().unwrap().is_empty());
    }

    #[test]
    fn an_item_reads_the_same_wherever_it_shows() {
        let spec = r##"{"paths": {
            "/v1/products/": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"items": {"type": "array", "items": {"$ref": "#/components/schemas/Product"}}}}}}}}}},
            "/v1/products/{id}": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Product"}}}}}}},
            "/v1/products/{id}/perks": {"post": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Product"}}}}}}},
            "/v1/products/{id}/archive": {"post": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Product"}}}}}}},
            "/v1/perks/": {
                "get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"items": {"type": "array", "items": {"$ref": "#/components/schemas/Perk"}}}}}}}}},
                "post": {"responses": {"201": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Perk"}}}}}}},
            "/v1/perks/{id}": {
                "patch": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Perk"}}}}}},
                "delete": {"responses": {"204": {"description": "gone"}}}},
            "/v1/orders/": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"items": {"type": "array", "items": {"$ref": "#/components/schemas/Order"}}}}}}}}}},
            "/v1/members/": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"items": {"type": "array", "items": {"$ref": "#/components/schemas/Member"}}}}}}}}}},
            "/v1/teams/{id}/members": {"post": {"responses": {"201": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Member"}}}}}}}},
          "components": {"schemas": {
            "Product": {"type": "object", "properties": {"id": {"type": "string", "format": "uuid"}, "name": {"type": "string"}, "is_archived": {"type": "boolean"},
                "perks": {"type": "array", "items": {"$ref": "#/components/schemas/Perk"}}}},
            "Perk": {"type": "object", "properties": {"id": {"type": "string", "format": "uuid"}, "description": {"type": "string"}}},
            "Order": {"type": "object", "properties": {"id": {"type": "string", "format": "uuid"}, "product_id": {"type": "string", "format": "uuid"}, "product": {"$ref": "#/components/schemas/OrderProduct"}}},
            "OrderProduct": {"type": "object", "properties": {"id": {"type": "string", "format": "uuid"}, "name": {"type": "string"}}},
            "Member": {"type": "object", "properties": {"id": {"type": "string", "format": "uuid"}, "team_id": {"type": "string", "format": "uuid"}, "email": {"type": "string"}}}}}}"##;
        let mock = Mock::start(Spec::read(spec).unwrap(), None, None, Arc::new(|_| {})).unwrap();
        let port = mock.port();
        let products = get(port, "/v1/products/").1;
        let product = products["items"][0]["id"].as_str().unwrap().to_owned();
        let seeded = get(port, "/v1/perks/").1["items"][1].clone();

        // A perk made, then given to the product with one listed.
        let (_, made) = send(
            port,
            "POST",
            "/v1/perks/",
            r#"{"description": "Early access"}"#,
        );
        let perk = made["id"].as_str().unwrap().to_owned();
        let given = format!(
            r#"{{"perks": ["{perk}", "{}"]}}"#,
            seeded["id"].as_str().unwrap()
        );
        let (status, attached) = send(
            port,
            "POST",
            &format!("/v1/products/{product}/perks"),
            &given,
        );
        assert_eq!(status, 200);
        assert_eq!(attached["perks"][0]["description"], "Early access");
        assert_eq!(attached["perks"][1], seeded);
        let read = get(port, &format!("/v1/products/{product}")).1;
        assert_eq!(read["perks"], attached["perks"]);
        assert_eq!(
            get(port, "/v1/products/").1["items"][0]["perks"],
            attached["perks"]
        );

        // Changed, it reads changed within the product; deleted, it leaves it.
        send(
            port,
            "PATCH",
            &format!("/v1/perks/{perk}"),
            r#"{"description": "Earlier access"}"#,
        );
        let read = get(port, &format!("/v1/products/{product}")).1;
        assert_eq!(read["perks"][0]["description"], "Earlier access");
        send(port, "DELETE", &format!("/v1/perks/{perk}"), "");
        let read = get(port, &format!("/v1/products/{product}")).1;
        assert_eq!(read["perks"], json!([seeded]));

        // An order's product is the product, as it is now.
        let (_, archived) = send(port, "POST", &format!("/v1/products/{product}/archive"), "");
        assert_eq!(archived["is_archived"], true);
        assert_eq!(
            get(port, "/v1/products/").1["items"][0]["is_archived"],
            true
        );
        let order = get(port, "/v1/orders/").1["items"][0].clone();
        assert_eq!(order["product_id"], product.as_str());
        assert_eq!(order["product"]["id"], product.as_str());
        assert_eq!(order["product"]["name"], products["items"][0]["name"]);

        // A member made from a team is one of the team's members.
        let team = "c0ffee00-0000-4000-a000-000000000000";
        let (status, member) = send(
            port,
            "POST",
            &format!("/v1/teams/{team}/members"),
            r#"{"email": "new@example.com"}"#,
        );
        assert_eq!((status, member["team_id"].as_str()), (201, Some(team)));
        let theirs = get(port, &format!("/v1/members/?team_id={team}")).1;
        assert_eq!(theirs["items"], json!([member]));
        assert_eq!(
            get(port, "/v1/members/").1["items"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
    }

    const SHOP: &str = r##"{"openapi": "3.1.0", "paths": {
        "/v1/checkouts/": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"items": {"type": "array", "items": {"$ref": "#/components/schemas/Checkout"}}}}}}}}}},
        "/v1/checkouts/{id}": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Checkout"}}}}}}},
        "/v1/checkouts/{id}/confirm": {"post": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Checkout"}}}}}}},
        "/v1/orders/": {
            "get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"items": {"type": "array", "items": {"$ref": "#/components/schemas/Order"}}}}}}}}},
            "post": {"requestBody": {"content": {"application/json": {"schema": {"type": "object", "properties": {"product_id": {"type": "string"}}}}}},
                "responses": {"201": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Order"}}}}}}},
        "/v1/orders/{id}": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Order"}}}}}}}},
      "components": {"schemas": {
        "Checkout": {"type": "object", "required": ["id", "status"], "properties": {"id": {"type": "string"}, "status": {"type": "string", "enum": ["open", "confirmed"]}, "product_id": {"type": "string"}}},
        "Order": {"type": "object", "required": ["id", "status"], "properties": {"id": {"type": "string"}, "status": {"type": "string", "enum": ["pending", "paid"]}, "checkout_id": {"type": ["string", "null"]}, "product_id": {"type": "string"}}}}}}"##;

    const CONFIRMING: &str = r#"
        on("POST /v1/checkouts/{id}/confirm", ({ params, answer }) => {
          const checkout = store.get(params.id);
          if (!checkout) return;
          const order = store.create("/v1/orders/", { checkout_id: params.id, product_id: checkout.product_id, status: "paid" });
          console.log("order", order.id);
          return { ...answer, status: "confirmed" };
        });
        on("GET /v1/orders/{id}", ({ answer }) => ({ ...answer, status: "lost" }));
    "#;

    #[test]
    fn a_written_behaviour_answers_and_changes_what_is_held_while_its_file_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("back-end.js");
        std::fs::write(&file, CONFIRMING).unwrap();
        let told: Arc<Mutex<Vec<Event>>> = Arc::default();
        let kept = Arc::clone(&told);
        let mock = Mock::start(
            Spec::read(SHOP).unwrap(),
            None,
            Some(file.clone()),
            Arc::new(move |event| kept.lock().unwrap().push(event)),
        )
        .unwrap();
        let port = mock.port();
        let checkouts = get(port, "/v1/checkouts/").1;
        let (first, second) = (
            checkouts["items"][0]["id"].as_str().unwrap().to_owned(),
            checkouts["items"][1]["id"].as_str().unwrap().to_owned(),
        );
        let (status, confirmed) = send(port, "POST", &format!("/v1/checkouts/{first}/confirm"), "");
        assert_eq!(
            (status, confirmed["status"].as_str()),
            (200, Some("confirmed"))
        );
        let orders = get(port, "/v1/orders/").1;
        assert_eq!(orders["items"][0]["checkout_id"], first.as_str());
        assert_eq!(orders["items"][0]["status"], "paid");
        let order = orders["items"][0]["id"].as_str().unwrap().to_owned();
        // An answer its schema refuses is not given.
        assert_eq!(
            get(port, &format!("/v1/orders/{order}")).1["status"],
            "paid"
        );
        let events = told.lock().unwrap().clone();
        let written: Vec<&Written> = events
            .iter()
            .filter_map(|e| match e {
                Event::Written(w) => Some(w),
                _ => None,
            })
            .collect();
        assert!(
            matches!(written[0], Written::Loaded { handled, unknown } if handled.len() == 2 && unknown.is_empty())
        );
        assert!(written.contains(&&Written::Said(format!("order {order}"))));
        assert!(
            written
                .iter()
                .any(|w| matches!(w, Written::Refused { operation, why }
            if operation == "GET /v1/orders/{id}" && why.contains("lost")))
        );
        let sources: Vec<Source> = events
            .iter()
            .filter_map(|e| match e {
                Event::Call(call) => Some(call.source),
                _ => None,
            })
            .collect();
        assert!(sources.contains(&Source::Written));

        // Its file gone, the back end answers as it would without it.
        std::fs::remove_file(&file).unwrap();
        send(port, "POST", &format!("/v1/checkouts/{second}/confirm"), "");
        assert_eq!(
            get(port, "/v1/orders/").1["items"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        // Written again, it is read again.
        std::fs::write(
            &file,
            r#"on("GET /v1/orders/", ({ answer }) => ({ items: [] }));"#,
        )
        .unwrap();
        assert_eq!(get(port, "/v1/orders/").1["items"], json!([]));
    }

    #[test]
    fn a_program_is_tried_before_it_is_kept() {
        let spec = Spec::read(SHOP).unwrap();
        assert_eq!(
            tried(
                &spec,
                None,
                CONFIRMING
                    .split("on(\"GET /v1/orders/{id}\"")
                    .next()
                    .unwrap()
            ),
            Ok(vec!["POST /v1/checkouts/{id}/confirm".to_owned()])
        );
        let wrong = tried(&spec, None, CONFIRMING).unwrap_err();
        assert!(
            wrong[0].starts_with("GET /v1/orders/{id}: it answered 200"),
            "{wrong:?}"
        );
        let unknown = tried(&spec, None, r#"on("GET /v1/nowhere", () => 1);"#).unwrap_err();
        assert!(
            unknown[0].contains("no operation of the spec"),
            "{unknown:?}"
        );
        let thrown = tried(
            &spec,
            None,
            r#"on("GET /v1/checkouts/{id}", ({ body }) => body.x.y);"#,
        )
        .unwrap_err();
        assert!(thrown[0].contains("GET /v1/checkouts/{id}: "), "{thrown:?}");
        assert!(tried(&spec, None, "on(").is_err());
    }

    #[test]
    fn who_is_there_is_told_by_me_and_by_one_name() {
        assert!(tells_who("/v1/users/me"));
        assert!(tells_who("/v1/me/organizations"));
        assert!(tells_who("/v1/organizations/?slug=demo&limit=1"));
        assert!(!tells_who("/v1/organizations/?limit=10"));
        assert!(!tells_who("/v1/memes"));
    }
}
