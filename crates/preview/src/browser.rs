//! The system's Chrome, Chromium or Edge, in a window of its own that
//! universe places over the App view's page and drives over the DevTools
//! protocol. The person uses the page in it as in any browser, at the
//! browser's own speed; what it does (its address, its loads, its
//! console's errors) is told, and its pages are kept from leaving this
//! machine.

use std::collections::{HashMap, VecDeque};
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

/// How often a window not yet found is looked for, once it is to show.
const LOOKED_FOR: Duration = Duration::from_millis(250);

/// How the name of a browser's profile begins, in the system's folder of
/// temporary files.
pub(crate) const PROFILE_PREFIX: &str = "universe-browser-";

/// Where the page's window goes on the screen, in the screen's
/// device-independent pixels: where the page itself shows, the window's
/// own frame around it left to the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    /// From the screen's left.
    pub left: i32,
    /// From the screen's top.
    pub top: i32,
    /// Its width.
    pub width: u32,
    /// Its height.
    pub height: u32,
}

/// What the browser is asked.
pub(crate) enum Request {
    /// Show this address.
    Go(String),
    /// Load the page again.
    Reload,
    /// Back in the page's history, or forward.
    History(bool),
    /// Show the window there, or hide it.
    Place(Option<Place>),
    /// Show these addresses in turn, each once it has loaded.
    Play(Vec<String>),
}

/// A browser running, stopped when dropped.
pub(crate) struct Browser {
    child: Child,
    program: PathBuf,
    profile: PathBuf,
    requests: Sender<Request>,
    thread: Option<JoinHandle<()>>,
}

impl Browser {
    /// Starts a browser at `address`, `cookies` set for it first. `shown`,
    /// its window is hidden until it is placed, and held over the window
    /// it names where the system lets it be; else it runs headless.
    ///
    /// # Errors
    ///
    /// [`PreviewError::NoBrowser`] when none is found, others when it does
    /// not start or its DevTools cannot be reached.
    pub(crate) fn start(
        address: &str,
        cookies: &[(String, String)],
        shown: Option<Option<u64>>,
        sink: Sink,
    ) -> Result<Self, PreviewError> {
        let program = find().ok_or(PreviewError::NoBrowser)?;
        static PROFILES: AtomicU64 = AtomicU64::new(0);
        let profile = std::env::temp_dir().join(format!(
            "{PROFILE_PREFIX}{}-{}",
            std::process::id(),
            PROFILES.fetch_add(1, Ordering::Relaxed)
        ));
        prepare(&profile);
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
                "--class=universe-page",
                "--window-size=800,600",
            ])
            // Kept from the desktop's session: nothing of the person's
            // keyring or settings is its to reach.
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        match shown {
            // A window of its own, without tabs or an address bar.
            Some(_) => {
                // An address the app window takes, which `about:blank` is
                // not: an empty page the colour of the wait.
                command.arg("--app=data:text/html,<body style='background:%2308090c'>");
                if cfg!(target_os = "linux") {
                    // Placed by universe, which only X11 lets a program do.
                    command
                        .arg("--ozone-platform=x11")
                        .env_remove("WAYLAND_DISPLAY");
                }
            }
            // Headless, it needs no screen at all, and is kept from it.
            None => {
                command
                    .args(["--headless=new", "about:blank"])
                    .env_remove("DISPLAY")
                    .env_remove("WAYLAND_DISPLAY");
            }
        }
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
        let url = wait_for_devtools(&listening, &mut child)?;
        let socket = connect(&url)?;
        let (requests, asked) = mpsc::channel();
        let mut driver = Driver {
            socket,
            session: String::new(),
            target: String::new(),
            window: None,
            next: 0,
            pending: HashMap::new(),
            main_frame: None,
            sink,
            playing: VecDeque::new(),
            step: None,
            frame: (0, 0),
            placed: None,
            held: match shown {
                Some(parent) => crate::window::Held::new(child.id(), parent),
                None => crate::window::Held::none(),
            },
            holding: false,
            looked: Instant::now(),
        };
        driver.open(address, cookies)?;
        let thread = std::thread::spawn(move || driver.drive(&asked));
        Ok(Self {
            child,
            program,
            profile,
            requests,
            thread: Some(thread),
        })
    }

    /// Its process's id.
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    /// What it was started as.
    pub(crate) fn program(&self) -> String {
        self.program.display().to_string()
    }

    /// The folder of its profile.
    pub(crate) fn profile(&self) -> &Path {
        &self.profile
    }

    /// Asks the browser something; nothing when it is gone.
    pub(crate) fn ask(&self, request: Request) {
        let _ = self.requests.send(request);
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // Its helpers may write there a moment longer than it lives.
        for _ in 0..5 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// A profile whose app windows take the system's frame, which universe
/// then takes away: the browser draws no title bar of its own over the
/// page.
fn prepare(profile: &Path) {
    let preferences = json!({
        "browser": {"custom_chrome_frame": false, "has_seen_welcome_page": true},
        "profile": {"exit_type": "Normal", "exited_cleanly": true},
    });
    let folder = profile.join("Default");
    let _ = std::fs::create_dir_all(&folder)
        .and_then(|()| std::fs::write(folder.join("Preferences"), preferences.to_string()));
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

/// What a command's answer is for, once it comes.
enum Pending {
    /// How much wider and taller the window is than its page: its frame.
    Frame,
}

/// The socket's side: what is asked, sent; what the browser says, told.
struct Driver {
    socket: WebSocket<TcpStream>,
    session: String,
    /// The page's target, and its window.
    target: String,
    window: Option<i64>,
    next: u64,
    pending: HashMap<u64, Pending>,
    main_frame: Option<String>,
    sink: Sink,
    /// The steps of a workflow yet to show, and since when the one
    /// showing was asked for.
    playing: VecDeque<String>,
    step: Option<Instant>,
    /// How much wider and taller the window is than the page it shows.
    frame: (i32, i32),
    /// Where the page was last placed, while it shows.
    placed: Option<Place>,
    /// The window held over universe's, where the system lets it be,
    /// whether it is, and when it was last looked for.
    held: crate::window::Held,
    holding: bool,
    looked: Instant,
}

impl Driver {
    /// Sends a command, on the page's session unless `browser`.
    fn send(&mut self, method: &str, params: Value, browser: bool) -> Result<u64, PreviewError> {
        self.next += 1;
        let mut message = json!({"id": self.next, "method": method, "params": params});
        if !browser {
            message["sessionId"] = Value::String(self.session.clone());
        }
        self.socket
            .send(Message::text(message.to_string()))
            .map_err(PreviewError::Socket)?;
        Ok(self.next)
    }

    /// Sends a command and waits for its answer, what the browser says
    /// meanwhile left unsaid.
    fn call(&mut self, method: &str, params: Value, browser: bool) -> Result<Value, PreviewError> {
        let id = self.send(method, params, browser)?;
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

    /// Takes the page of the app window it opened with, hides the window
    /// until it is placed, sets the cookies, and goes to `address`.
    fn open(&mut self, address: &str, cookies: &[(String, String)]) -> Result<(), PreviewError> {
        let began = Instant::now();
        let target = loop {
            let targets = self.call("Target.getTargets", json!({}), true)?;
            let page = targets["targetInfos"].as_array().and_then(|all| {
                all.iter()
                    .find(|t| t["type"] == "page")
                    .and_then(|t| t["targetId"].as_str())
                    .map(str::to_owned)
            });
            match page {
                Some(page) => break page,
                None if began.elapsed() < STARTS_WITHIN => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                None => {
                    return Err(PreviewError::Refused {
                        method: "Target.getTargets".to_owned(),
                        message: "no window opened".to_owned(),
                    });
                }
            }
        };
        let attached = self.call(
            "Target.attachToTarget",
            json!({"targetId": target, "flatten": true}),
            true,
        )?;
        self.session = attached["sessionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let window = self.call(
            "Browser.getWindowForTarget",
            json!({"targetId": target}),
            true,
        )?;
        self.window = window["windowId"].as_i64();
        self.target = target;
        self.hide()?;
        for domain in [
            "Page.enable",
            "Runtime.enable",
            "Log.enable",
            "Network.enable",
        ] {
            self.call(domain, json!({}), false)?;
        }
        self.call(
            "Page.setLifecycleEventsEnabled",
            json!({"enabled": true}),
            false,
        )?;
        // Pages are held before they load, to keep them on this machine.
        self.call(
            "Fetch.enable",
            json!({"patterns": [{"resourceType": "Document", "requestStage": "Request"}]}),
            false,
        )?;
        let cookies: Vec<Value> = cookies
            .iter()
            .map(|(name, value)| json!({"name": name, "value": value, "url": address}))
            .collect();
        if !cookies.is_empty() {
            self.call("Network.setCookies", json!({"cookies": cookies}), false)?;
        }
        self.send("Page.navigate", json!({"url": address}), false)?;
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
            // A window shown is held once the system has it: placed again,
            // its frame gone.
            if let Some(place) = self.placed
                && !self.holding
                && self.held.can_hold()
                && self.looked.elapsed() > LOOKED_FOR
            {
                self.looked = Instant::now();
                self.holding = self.held.hold();
                if self.holding && self.bounds(place).is_err() {
                    return;
                }
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
                self.send("Page.navigate", json!({"url": address}), false)?;
            }
            Request::Reload => {
                self.send("Page.reload", json!({}), false)?;
            }
            Request::History(back) => {
                let expression = if back {
                    "history.back()"
                } else {
                    "history.forward()"
                };
                self.send("Runtime.evaluate", json!({"expression": expression}), false)?;
            }
            Request::Place(Some(place)) => self.place(place)?,
            Request::Place(None) => {
                if self.placed.take().is_some() {
                    self.hide()?;
                }
            }
            Request::Play(steps) => {
                self.playing = steps.into();
                self.next_step()?;
            }
        }
        Ok(())
    }

    /// Shows the window with its page at `place`, held over universe's.
    fn place(&mut self, place: Place) -> Result<(), PreviewError> {
        let Some(window) = self.window else {
            return Ok(());
        };
        if self.placed.is_none() {
            self.send(
                "Browser.setWindowBounds",
                json!({"windowId": window, "bounds": {"windowState": "normal"}}),
                true,
            )?;
            // How much of the window is its frame, where it keeps one.
            if !self.held.can_hold() {
                let id = self.send(
                    "Runtime.evaluate",
                    json!({"expression": "[outerWidth - innerWidth, outerHeight - innerHeight]", "returnByValue": true}),
                    false,
                )?;
                self.pending.insert(id, Pending::Frame);
            }
        }
        if self.holding {
            self.holding = self.held.hold();
        }
        self.placed = Some(place);
        self.bounds(place)
    }

    /// Sets the window's bounds for its page to be at `place`, its frame
    /// above and around it.
    fn bounds(&mut self, place: Place) -> Result<(), PreviewError> {
        let Some(window) = self.window else {
            return Ok(());
        };
        let (wider, taller) = self.frame;
        let side = wider / 2;
        self.send(
            "Browser.setWindowBounds",
            json!({"windowId": window, "bounds": {
                "left": place.left - side,
                "top": place.top - (taller - side).max(0),
                "width": i64::from(place.width) + i64::from(wider),
                "height": i64::from(place.height) + i64::from(taller),
            }}),
            true,
        )?;
        Ok(())
    }

    fn hide(&mut self) -> Result<(), PreviewError> {
        if let Some(window) = self.window {
            self.send(
                "Browser.setWindowBounds",
                json!({"windowId": window, "bounds": {"windowState": "minimized"}}),
                true,
            )?;
        }
        Ok(())
    }

    /// Shows the next step of the workflow, if one is left.
    fn next_step(&mut self) -> Result<(), PreviewError> {
        self.step = None;
        if let Some(address) = self.playing.pop_front() {
            self.send("Page.navigate", json!({"url": address}), false)?;
            self.step = Some(Instant::now());
        }
        Ok(())
    }

    fn told(&mut self, message: &Value) -> Result<(), PreviewError> {
        if let Some(id) = message["id"].as_u64() {
            if let Some(Pending::Frame) = self.pending.remove(&id) {
                let value = &message["result"]["result"]["value"];
                let thickness = |i: usize| {
                    value[i]
                        .as_i64()
                        .and_then(|n| i32::try_from(n).ok())
                        .unwrap_or(0)
                        .max(0)
                };
                self.frame = (thickness(0), thickness(1));
                if self.frame != (0, 0)
                    && let Some(place) = self.placed
                {
                    self.bounds(place)?;
                }
            }
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
                    self.send("Fetch.continueRequest", json!({"requestId": id}), false)?;
                } else {
                    self.send(
                        "Fetch.failRequest",
                        json!({"requestId": id, "errorReason": "BlockedByClient"}),
                        false,
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
            // Its window closed: the browser is gone with it.
            "Target.detachedFromTarget" if params["sessionId"].as_str() == Some(&self.session) => {
                (self.sink)(Event::Gone);
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
