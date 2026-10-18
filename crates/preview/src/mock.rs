//! The simulated back end: a local server that answers each operation of
//! the spec with its fixture, kept in a folder, else a body made from its
//! schema, in the state asked for; and says each call it answers.
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
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::error::PreviewError;
use crate::made::{Tone, body};
use crate::spec::Spec;

/// How long a slow back end takes over each call.
pub const SLOW: Duration = Duration::from_millis(1500);

/// The most a request may hold, its body included.
const LARGEST_REQUEST: usize = 4 * 1024 * 1024;

/// How an operation answers, chosen for it alone.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// What the back end does with each call it answers.
pub type Calls = Arc<dyn Fn(Call) + Send + Sync>;

/// The simulated back end, serving until dropped.
pub struct Mock {
    port: u16,
    spec: Arc<Spec>,
    state: Arc<Mutex<State>>,
    fixtures: Option<PathBuf>,
    /// The last body each operation answered, to keep as its fixture.
    last: Arc<Mutex<HashMap<usize, Value>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Mock {
    /// Starts serving `spec` on a free local port, with the fixtures of
    /// `fixtures` when given, saying each call to `calls`.
    ///
    /// # Errors
    ///
    /// [`PreviewError::Listen`] when no local port can be listened on.
    pub fn start(
        spec: Spec,
        fixtures: Option<PathBuf>,
        calls: Calls,
    ) -> Result<Self, PreviewError> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(PreviewError::Listen)?;
        let port = listener.local_addr().map_err(PreviewError::Listen)?.port();
        let spec = Arc::new(spec);
        let state = Arc::new(Mutex::new(State::default()));
        let last = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let served = Served {
            spec: Arc::clone(&spec),
            state: Arc::clone(&state),
            fixtures: fixtures.clone(),
            last: Arc::clone(&last),
            calls,
            started: Instant::now(),
            seq: Arc::new(AtomicU64::new(0)),
        };
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
            stop,
            thread: Some(thread),
        })
    }

    /// The port it serves on, on `127.0.0.1`.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Changes the state it answers in.
    pub fn change(&self, change: impl FnOnce(&mut State)) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut state);
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
    state: Arc<Mutex<State>>,
    fixtures: Option<PathBuf>,
    last: Arc<Mutex<HashMap<usize, Value>>>,
    calls: Calls,
    started: Instant,
    seq: Arc<AtomicU64>,
}

/// A request, as far as the back end reads it.
struct Request {
    method: String,
    path: String,
    origin: Option<String>,
    asked_headers: Option<String>,
    from_page: bool,
}

impl Served {
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
            Some(index) => self.answer_operation(index, &request, &state),
        };
        let slow = state.slow || found.and_then(|i| state.answers.get(&i)) == Some(&Answer::Slow);
        if slow {
            std::thread::sleep(SLOW);
        }
        let text = value.as_ref().map(Value::to_string);
        let _ = write_response(&mut stream, status, &request, text.as_deref());
        let _ = stream.shutdown(Shutdown::Both);
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        (self.calls)(Call {
            seq,
            at: begun.duration_since(self.started),
            method: request.method,
            path: request.path,
            operation: found,
            status,
            source,
            took: begun.elapsed(),
            from_page: request.from_page,
        });
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
        let status = success(&operation.statuses);
        let code = status.parse().unwrap_or(200);
        let fixture = self
            .fixtures
            .as_deref()
            .map(|f| f.join(fixture_name(&operation.method, &operation.path)))
            .and_then(|path| read_fixture(&path));
        let tone = Tone {
            empty: (state.tone.empty && !kept) || chosen == Some(&Answer::Empty),
            ..state.tone
        };
        let (value, source) = match fixture {
            Some(value) => (Some(value), Source::Fixture),
            None => match self.spec.schema(index, &status) {
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
                None if code == 204 => (None, Source::Schema),
                None => (Some(json!({})), Source::Schema),
            },
        };
        if let Some(value) = &value
            && source != Source::Changed
        {
            self.last
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(index, value.clone());
        }
        (code, value, source)
    }
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

/// Reads a request's line, headers and body, which it does not need but
/// must not leave unread.
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
            _ => {}
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        origin,
        asked_headers,
        from_page,
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
            Arc::new(move |call| kept.lock().unwrap().push(call)),
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

    #[test]
    fn who_is_there_is_told_by_me_and_by_one_name() {
        assert!(tells_who("/v1/users/me"));
        assert!(tells_who("/v1/me/organizations"));
        assert!(tells_who("/v1/organizations/?slug=demo&limit=1"));
        assert!(!tells_who("/v1/organizations/?limit=10"));
        assert!(!tells_who("/v1/memes"));
    }
}
