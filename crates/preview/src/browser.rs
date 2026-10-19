//! A browser driven over the DevTools protocol: the Chrome universe holds
//! in its own window, which the person uses as any browser, reached at
//! the port its DevTools listen on; else the system's Chrome, Chromium or
//! Edge started headless, as tests do. What its page does (its address,
//! its loads, its console's errors) is told, and its pages are kept from
//! leaving this machine.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

use crate::error::PreviewError;
use crate::{Event, Sink};

/// How long the browser may take to say where its DevTools listen, and
/// to open its window.
const STARTS_WITHIN: Duration = Duration::from_secs(30);

/// How long a step of a workflow may load before the next is shown.
const STEP_WITHIN: Duration = Duration::from_secs(15);

/// How long after a step is asked for its page may be said quiet: what the
/// page before said is not its.
const STEP_SETTLES: Duration = Duration::from_millis(400);

/// How the name of a browser's profile begins, in the system's folder of
/// temporary files.
pub(crate) const PROFILE_PREFIX: &str = "universe-browser-";

/// What the browser is asked.
pub(crate) enum Request {
    /// Show this address.
    Go(String),
    /// Load the page again.
    Reload,
    /// Back in the page's history, or forward.
    History(bool),
    /// Show these addresses in turn, each once it has loaded.
    Play(Vec<String>),
}

/// A browser driven, let go when dropped: stopped with it when it was
/// started for the preview.
pub(crate) struct Browser {
    started: Option<Started>,
    requests: Option<Sender<Request>>,
    thread: Option<JoinHandle<()>>,
}

/// A browser started for the preview: its process, what it was started
/// as, and its profile's folder.
struct Started {
    child: Child,
    program: PathBuf,
    profile: PathBuf,
}

impl Browser {
    /// Starts the system's browser, headless, at `address`, `cookies` set
    /// for it first.
    ///
    /// # Errors
    ///
    /// [`PreviewError::NoBrowser`] when none is found, others when it does
    /// not start or its DevTools cannot be reached.
    pub(crate) fn start(
        address: &str,
        cookies: &[(String, String)],
        sink: Sink,
    ) -> Result<Self, PreviewError> {
        let program = find().ok_or(PreviewError::NoBrowser)?;
        static PROFILES: AtomicU64 = AtomicU64::new(0);
        let profile = std::env::temp_dir().join(format!(
            "{PROFILE_PREFIX}{}-{}",
            std::process::id(),
            PROFILES.fetch_add(1, Ordering::Relaxed)
        ));
        let mut command = Command::new(&program);
        command
            .args([
                "--remote-debugging-port=0",
                &format!("--user-data-dir={}", profile.display()),
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-extensions",
                "--disable-sync",
                "--disable-features=Translate",
                "--hide-crash-restore-bubble",
                "--metrics-recording-only",
                "--password-store=basic",
                "--use-mock-keychain",
                "--window-size=1280,800",
                // Headless, it needs no screen at all, and is kept from it.
                "--headless=new",
                "about:blank",
            ])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            // Kept from the desktop's session: nothing of the person's
            // keyring or settings is its to reach.
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|source| PreviewError::Start {
            command: program.display().to_string(),
            source,
        })?;
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            return Err(PreviewError::NoDevTools {
                said: String::new(),
            });
        };
        let (found, listening) = mpsc::channel();
        std::thread::spawn(move || {
            let mut said = Vec::new();
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Some(at) = line.find("ws://") {
                    let _ = found.send(Ok(line[at..].trim().to_owned()));
                } else if said.len() < 20 {
                    said.push(line);
                    let _ = found.send(Err(said.join("\n")));
                }
            }
        });
        let url = match wait_for_devtools(&listening, &mut child) {
            Ok(url) => url,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&profile);
                return Err(error);
            }
        };
        let started = Started {
            child,
            program,
            profile,
        };
        let port = url
            .trim_start_matches("ws://")
            .split('/')
            .next()
            .and_then(|host| host.rsplit_once(':'))
            .and_then(|(_, port)| port.parse().ok());
        let Some(port) = port else {
            return Err(PreviewError::NoDevTools {
                said: format!("its DevTools listen at {url}"),
            });
        };
        let page = wait_for_page(port)?;
        Self::drive_at(&page, address, cookies, sink, Some(started))
    }

    /// Drives the browser whose DevTools listen on `port`, on this machine:
    /// the one universe holds in its window, whose page it waits for, then
    /// sends to `address`, `cookies` set for it first.
    ///
    /// # Errors
    ///
    /// [`PreviewError::NoDevTools`] when nothing answers there in time,
    /// others when its socket cannot be opened.
    pub(crate) fn attach(
        port: u16,
        address: &str,
        cookies: &[(String, String)],
        sink: Sink,
    ) -> Result<Self, PreviewError> {
        let url = wait_for_page(port)?;
        Self::drive_at(&url, address, cookies, sink, None)
    }

    fn drive_at(
        url: &str,
        address: &str,
        cookies: &[(String, String)],
        sink: Sink,
        started: Option<Started>,
    ) -> Result<Self, PreviewError> {
        let socket = connect(url)?;
        let (requests, asked) = mpsc::channel();
        let mut driver = Driver {
            socket,
            next: 0,
            main_frame: None,
            sink,
            playing: VecDeque::new(),
            step: None,
        };
        driver.open(address, cookies)?;
        let thread = std::thread::spawn(move || driver.drive(&asked));
        Ok(Self {
            started,
            requests: Some(requests),
            thread: Some(thread),
        })
    }

    /// The process it was started as, what it was started as, and its
    /// profile: none for the browser universe holds.
    pub(crate) fn started(&self) -> Option<(u32, String, &Path)> {
        self.started.as_ref().map(|s| {
            (
                s.child.id(),
                s.program.display().to_string(),
                s.profile.as_path(),
            )
        })
    }

    /// Asks the browser something; nothing when it is gone.
    pub(crate) fn ask(&self, request: Request) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(request);
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // Its driver stops once no one asks it anything, or once the
        // browser it drives is gone.
        self.requests = None;
        if let Some(started) = &mut self.started {
            let _ = started.child.kill();
            let _ = started.child.wait();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(started) = &self.started {
            // Its helpers may write there a moment longer than it lives.
            for _ in 0..5 {
                if std::fs::remove_dir_all(&started.profile).is_ok() || !started.profile.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// The socket of the page the DevTools on `port` list first, once they
/// list one.
fn wait_for_page(port: u16) -> Result<String, PreviewError> {
    let began = Instant::now();
    loop {
        match page_socket(port) {
            Some(url) => return Ok(url),
            None if began.elapsed() < STARTS_WITHIN => {
                std::thread::sleep(Duration::from_millis(100));
            }
            None => {
                return Err(PreviewError::NoDevTools {
                    said: format!("no page answered on port {port}"),
                });
            }
        }
    }
}

/// The socket of the page the DevTools on `port` list first: `None` while
/// nothing answers there, or no page is listed. A page's own socket, as
/// the Chrome held in universe's window has its page driven: its browser
/// lists no targets of its own.
fn page_socket(port: u16) -> Option<String> {
    use std::io::{Read as _, Write as _};
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .write_all(
            format!(
                "GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .ok()?;
    // Read as long as its length says: the DevTools keep the connection
    // open, however they are asked.
    let mut answer = Vec::new();
    let mut chunk = [0u8; 8192];
    let body = loop {
        let read = stream.read(&mut chunk).ok()?;
        answer.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&answer);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            });
            if read == 0 || length.is_some_and(|n| body.len() >= n) {
                break body.to_owned();
            }
        } else if read == 0 {
            return None;
        }
    };
    let listed: Value = serde_json::from_str(body.trim()).ok()?;
    listed
        .as_array()?
        .iter()
        .find(|target| target["type"] == "page")
        .and_then(|page| page["webSocketDebuggerUrl"].as_str())
        .map(str::to_owned)
}

/// Waits for the line that says where the DevTools listen.
fn wait_for_devtools(
    listening: &Receiver<Result<String, String>>,
    child: &mut Child,
) -> Result<String, PreviewError> {
    let began = Instant::now();
    let mut said = String::new();
    while began.elapsed() < STARTS_WITHIN {
        match listening.recv_timeout(Duration::from_millis(200)) {
            Ok(Ok(url)) => return Ok(url),
            Ok(Err(lines)) => said = lines,
            Err(RecvTimeoutError::Timeout) => {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    Err(PreviewError::NoDevTools { said })
}

/// Opens the DevTools' socket at `url`, waiting a little at most for each
/// message so that what is asked is sent at once.
fn connect(url: &str) -> Result<WebSocket<TcpStream>, PreviewError> {
    let host = url
        .trim_start_matches("ws://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let stream = TcpStream::connect(&host).map_err(|e| PreviewError::Socket(e.into()))?;
    let (socket, _) = tungstenite::client(url, stream).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(e) => PreviewError::Socket(e),
        tungstenite::HandshakeError::Interrupted(_) => {
            PreviewError::Socket(std::io::Error::from(std::io::ErrorKind::WouldBlock).into())
        }
    })?;
    socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(15)))
        .map_err(|e| PreviewError::Socket(e.into()))?;
    Ok(socket)
}

/// The browser's executable: the one `UNIVERSE_CHROME` names, else the
/// first found of Chrome, Chromium and Edge.
fn find() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("UNIVERSE_CHROME") {
        return Some(PathBuf::from(named));
    }
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
        "microsoft-edge",
        "msedge",
    ];
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    ]
    .iter()
    .map(Path::new)
    .find(|p| p.is_file())
    .map(Path::to_path_buf)
}

/// Whether a browser can be driven here.
#[must_use]
pub fn browser_found() -> bool {
    find().is_some()
}

/// The socket's side: what is asked, sent; what the browser says, told.
struct Driver {
    socket: WebSocket<TcpStream>,
    next: u64,
    main_frame: Option<String>,
    sink: Sink,
    /// The steps of a workflow yet to show, and since when the one
    /// showing was asked for.
    playing: VecDeque<String>,
    step: Option<Instant>,
}

impl Driver {
    /// Sends a command to the page.
    fn send(&mut self, method: &str, params: Value) -> Result<u64, PreviewError> {
        self.next += 1;
        let message = json!({"id": self.next, "method": method, "params": params});
        self.socket
            .send(Message::text(message.to_string()))
            .map_err(PreviewError::Socket)?;
        Ok(self.next)
    }

    /// Sends a command and waits for its answer, what the browser says
    /// meanwhile left unsaid.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, PreviewError> {
        let id = self.send(method, params)?;
        let began = Instant::now();
        while began.elapsed() < STARTS_WITHIN {
            let Some(message) = self.receive()? else {
                continue;
            };
            if message["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(PreviewError::Refused {
                    method: method.to_owned(),
                    message: error["message"].as_str().unwrap_or_default().to_owned(),
                });
            }
            return Ok(message["result"].clone());
        }
        Err(PreviewError::Refused {
            method: method.to_owned(),
            message: "no answer".to_owned(),
        })
    }

    fn receive(&mut self) -> Result<Option<Value>, PreviewError> {
        match self.socket.read() {
            Ok(Message::Text(text)) => Ok(serde_json::from_str(text.as_str()).ok()),
            Ok(Message::Close(_)) => {
                Err(PreviewError::Socket(tungstenite::Error::ConnectionClosed))
            }
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(PreviewError::Socket(e)),
        }
    }

    /// Hears what its page says, sets the cookies, and goes to `address`.
    fn open(&mut self, address: &str, cookies: &[(String, String)]) -> Result<(), PreviewError> {
        for domain in [
            "Page.enable",
            "Runtime.enable",
            "Log.enable",
            "Network.enable",
        ] {
            self.call(domain, json!({}))?;
        }
        self.call("Page.setLifecycleEventsEnabled", json!({"enabled": true}))?;
        // Pages are held before they load, to keep them on this machine.
        self.call(
            "Fetch.enable",
            json!({"patterns": [{"resourceType": "Document", "requestStage": "Request"}]}),
        )?;
        let cookies: Vec<Value> = cookies
            .iter()
            .map(|(name, value)| json!({"name": name, "value": value, "url": address}))
            .collect();
        if !cookies.is_empty() {
            self.call("Network.setCookies", json!({"cookies": cookies}))?;
        }
        self.send("Page.navigate", json!({"url": address}))?;
        Ok(())
    }

    /// Sends what is asked and tells what the browser says, until either
    /// side is gone.
    fn drive(mut self, asked: &Receiver<Request>) {
        loop {
            loop {
                match asked.try_recv() {
                    Ok(request) => {
                        if self.ask(request).is_err() {
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            if self.step.is_some_and(|since| since.elapsed() > STEP_WITHIN)
                && self.next_step().is_err()
            {
                return;
            }
            match self.receive() {
                Ok(Some(message)) => {
                    if self.told(&message).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    (self.sink)(Event::Gone);
                    return;
                }
            }
        }
    }

    fn ask(&mut self, request: Request) -> Result<(), PreviewError> {
        match request {
            Request::Go(address) => {
                self.send("Page.navigate", json!({"url": address}))?;
            }
            Request::Reload => {
                self.send("Page.reload", json!({}))?;
            }
            Request::History(back) => {
                let expression = if back {
                    "history.back()"
                } else {
                    "history.forward()"
                };
                self.send("Runtime.evaluate", json!({"expression": expression}))?;
            }
            Request::Play(steps) => {
                self.playing = steps.into();
                self.next_step()?;
            }
        }
        Ok(())
    }

    /// Shows the next step of the workflow, if one is left.
    fn next_step(&mut self) -> Result<(), PreviewError> {
        self.step = None;
        if let Some(address) = self.playing.pop_front() {
            self.send("Page.navigate", json!({"url": address}))?;
            self.step = Some(Instant::now());
        }
        Ok(())
    }

    fn told(&mut self, message: &Value) -> Result<(), PreviewError> {
        if message.get("id").is_some() {
            return Ok(());
        }
        let params = &message["params"];
        match message["method"].as_str().unwrap_or_default() {
            // A page that would leave this machine (the front end sending
            // to where it is deployed, a sign-in elsewhere) is kept from it:
            // what it shows would not be the front end's, nor its calls
            // simulated.
            "Fetch.requestPaused" => {
                let url = params["request"]["url"].as_str().unwrap_or_default();
                let id = params["requestId"].clone();
                if stays_here(url) {
                    self.send("Fetch.continueRequest", json!({"requestId": id}))?;
                } else {
                    self.send(
                        "Fetch.failRequest",
                        json!({"requestId": id, "errorReason": "BlockedByClient"}),
                    )?;
                    (self.sink)(Event::Console(format!(
                        "Kept from leaving this machine: the page went to {url}"
                    )));
                }
            }
            "Page.frameNavigated" => {
                let frame = &params["frame"];
                if frame.get("parentId").is_none() {
                    self.main_frame = frame["id"].as_str().map(str::to_owned);
                    if let Some(url) = frame["url"].as_str() {
                        (self.sink)(Event::Address {
                            url: url.to_owned(),
                            within: false,
                        });
                    }
                }
            }
            "Page.navigatedWithinDocument" => {
                if params["frameId"].as_str() == self.main_frame.as_deref()
                    && let Some(url) = params["url"].as_str()
                {
                    (self.sink)(Event::Address {
                        url: url.to_owned(),
                        within: true,
                    });
                }
            }
            "Page.frameStartedLoading" => {
                let main = self.main_frame.is_none()
                    || params["frameId"].as_str() == self.main_frame.as_deref();
                if main {
                    (self.sink)(Event::Loading);
                }
            }
            "Page.lifecycleEvent" => {
                let main = params["frameId"].as_str() == self.main_frame.as_deref();
                if main && params["name"] == "networkIdle" {
                    (self.sink)(Event::Quiet);
                    if self
                        .step
                        .is_some_and(|since| since.elapsed() > STEP_SETTLES)
                    {
                        self.next_step()?;
                    }
                }
            }
            "Runtime.exceptionThrown" => {
                let details = &params["exceptionDetails"];
                let text = details["exception"]["description"]
                    .as_str()
                    .or_else(|| details["text"].as_str())
                    .unwrap_or("An exception was thrown");
                (self.sink)(Event::Console(first_lines(text)));
            }
            "Runtime.consoleAPICalled" if params["type"] == "error" => {
                let args = params["args"].as_array().map_or(&[][..], Vec::as_slice);
                (self.sink)(Event::Console(first_lines(&console_text(args))));
            }
            "Log.entryAdded" if params["entry"]["level"] == "error" => {
                let entry = &params["entry"];
                let text = entry["text"].as_str().unwrap_or_default();
                let url = entry["url"].as_str().unwrap_or_default();
                // A page with no icon of its own has not failed.
                if url
                    .split(['?', '#'])
                    .next()
                    .unwrap_or(url)
                    .ends_with("/favicon.ico")
                {
                    return Ok(());
                }
                // What failed to load, said with it.
                let said = if url.is_empty() || text.contains(url) {
                    text.to_owned()
                } else {
                    format!("{text}: {}", path_of(url))
                };
                (self.sink)(Event::Console(first_lines(&said)));
            }
            _ => {}
        }
        Ok(())
    }
}

/// What a call to `console.error` printed: its first argument's `%s`,
/// `%d`, `%o` and the like each given the next argument, as the console
/// shows it, then the arguments left.
fn console_text(args: &[Value]) -> String {
    let shown = |arg: &Value| {
        arg["value"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| arg["value"].as_number().map(ToString::to_string))
            .or_else(|| arg["description"].as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let Some((first, rest)) = args.split_first() else {
        return String::new();
    };
    let Some(format) = first["value"].as_str() else {
        return args.iter().map(shown).collect::<Vec<_>>().join(" ");
    };
    let mut rest = rest.iter();
    let mut text = String::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            text.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('s' | 'd' | 'i' | 'f' | 'o' | 'O') => {
                chars.next();
                text.push_str(&rest.next().map(shown).unwrap_or_default());
            }
            // Styles, which a line of text does without.
            Some('c') => {
                chars.next();
                rest.next();
            }
            Some('%') => {
                chars.next();
                text.push('%');
            }
            _ => text.push('%'),
        }
    }
    for arg in rest {
        text.push(' ');
        text.push_str(&shown(arg));
    }
    text
}

/// An address of this machine as its path: `/v1/products/?page=2`.
fn path_of(url: &str) -> &str {
    match url.split_once("://") {
        Some((_, rest)) if stays_here(url) => rest.find('/').map_or("/", |i| &rest[i..]),
        _ => url,
    }
}

/// Whether an address is of this machine, or one of a page's own (`data:`,
/// `about:`, `blob:`).
fn stays_here(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return true;
    };
    if !matches!(scheme, "http" | "https" | "ws" | "wss") {
        return true;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = if host.starts_with('[') {
        host.split(']').next().map_or(host, |h| &h[1..])
    } else {
        host.split(':').next().unwrap_or(host)
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "0.0.0.0") || host.ends_with(".localhost")
}

/// The first lines of a message: a stack's first frames, not all of it.
fn first_lines(text: &str) -> String {
    text.lines().take(4).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_machine_s_pages_load() {
        assert!(stays_here("http://127.0.0.1:3000/dashboard"));
        assert!(stays_here("http://localhost:5173/"));
        assert!(stays_here("http://[::1]:8000/"));
        assert!(stays_here("http://app.localhost/"));
        assert!(stays_here("about:blank"));
        assert!(stays_here("data:text/html,hi"));
        assert!(!stays_here("https://shop.example.com/dashboard"));
        assert!(!stays_here("https://127.0.0.1.example.com/"));
        assert!(!stays_here("http://user@example.com/"));
        // Said by its path, when of this machine.
        assert_eq!(
            path_of("http://127.0.0.1:4100/v1/items?page=2"),
            "/v1/items?page=2"
        );
        assert_eq!(path_of("http://localhost:3000"), "/");
        assert_eq!(
            path_of("https://cdn.example.com/a.js"),
            "https://cdn.example.com/a.js"
        );
    }

    #[test]
    fn a_console_line_reads_as_the_console_shows_it() {
        let args = [
            json!({"type": "string", "value": "Mounting a new %s while <%s> is: %o%c."}),
            json!({"type": "string", "value": "html"}),
            json!({"type": "string", "value": "html"}),
            json!({"type": "object", "description": "TypeError: x is null"}),
            json!({"type": "string", "value": "color: red"}),
            json!({"type": "number", "value": 3}),
        ];
        assert_eq!(
            console_text(&args),
            "Mounting a new html while <html> is: TypeError: x is null. 3"
        );
        assert_eq!(console_text(&[json!({"type": "number", "value": 7})]), "7");
        assert_eq!(console_text(&[]), "");
    }
}
