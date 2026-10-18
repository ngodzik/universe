//! A browser driven over the DevTools protocol: Chrome, Chromium or Edge,
//! headless, in a profile of its own. Its page is seen as the frames it
//! sends, each sent on only once the one before is taken, so that a slow
//! reader slows it rather than piles frames up; it is given the person's
//! pointer and keys, sent where it is asked, photographed.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

use crate::error::PreviewError;
use crate::{Event, Frame, Sink};

/// How long the browser may take to say where its DevTools listen.
const STARTS_WITHIN: Duration = Duration::from_secs(30);

/// How the name of a browser's profile begins, in the system's folder of
/// temporary files.
pub(crate) const PROFILE_PREFIX: &str = "universe-browser-";

/// How long a step of a workflow may load before it is photographed as it
/// is.
const STEP_WITHIN: Duration = Duration::from_secs(15);

/// How long after a step is asked for its page may be said quiet: what the
/// page before said is not its.
const STEP_SETTLES: Duration = Duration::from_millis(400);

/// How long the page must show the same frame to be said still.
const STILL_AFTER: Duration = Duration::from_millis(700);

/// A button of the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Button {
    /// None held.
    None,
    /// The main one.
    Left,
    /// The wheel's.
    Middle,
    /// The one for a menu.
    Right,
}

/// What the person's pointer does on the page, in its pixels from its top
/// left.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Pointer {
    /// Moves there.
    Move {
        /// Across.
        x: f32,
        /// Down.
        y: f32,
    },
    /// Presses a button there.
    Press {
        /// Across.
        x: f32,
        /// Down.
        y: f32,
        /// Which.
        button: Button,
        /// How many times in a row: 2 for a double click.
        clicks: u32,
    },
    /// Lets a button go there.
    Release {
        /// Across.
        x: f32,
        /// Down.
        y: f32,
        /// Which.
        button: Button,
        /// How many times in a row.
        clicks: u32,
    },
    /// Turns the wheel there, down and right when positive.
    Wheel {
        /// Across.
        x: f32,
        /// Down.
        y: f32,
        /// Across, in pixels.
        dx: f32,
        /// Down, in pixels.
        dy: f32,
    },
}

/// The keys held with another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Modifiers {
    /// Alt, Option.
    pub alt: bool,
    /// Control.
    pub ctrl: bool,
    /// Command, Windows.
    pub meta: bool,
    /// Shift.
    pub shift: bool,
}

impl Modifiers {
    /// As the protocol counts them.
    fn bits(self) -> u8 {
        u8::from(self.alt)
            | u8::from(self.ctrl) << 1
            | u8::from(self.meta) << 2
            | u8::from(self.shift) << 3
    }
}

/// A key pressed on the page.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Key {
    /// What it means: `Enter`, `a`.
    pub key: String,
    /// Where it is: `Enter`, `KeyA`.
    pub code: String,
    /// What it types, if anything.
    pub text: Option<String>,
    /// Windows' number for it, which pages still read.
    pub number: u32,
    /// What is held with it.
    pub modifiers: Modifiers,
}

impl Key {
    /// A key that types nothing, or a line's end, by its name: `Enter`,
    /// `Backspace`, `Tab`, `Escape`, `ArrowLeft`, `ArrowUp`, `ArrowRight`,
    /// `ArrowDown`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`.
    #[must_use]
    pub fn named(name: &str, modifiers: Modifiers) -> Option<Self> {
        let (number, text) = match name {
            "Enter" => (13, Some("\r")),
            "Backspace" => (8, None),
            "Tab" => (9, None),
            "Escape" => (27, None),
            "ArrowLeft" => (37, None),
            "ArrowUp" => (38, None),
            "ArrowRight" => (39, None),
            "ArrowDown" => (40, None),
            "Delete" => (46, None),
            "Home" => (36, None),
            "End" => (35, None),
            "PageUp" => (33, None),
            "PageDown" => (34, None),
            _ => return None,
        };
        Some(Self {
            key: name.to_owned(),
            code: name.to_owned(),
            text: text.map(str::to_owned),
            number,
            modifiers,
        })
    }

    /// A letter or a digit pressed with what is held, as a shortcut is:
    /// `Ctrl-A`.
    #[must_use]
    pub fn shortcut(c: char, modifiers: Modifiers) -> Self {
        let upper = c.to_ascii_uppercase();
        let code = if upper.is_ascii_digit() {
            format!("Digit{upper}")
        } else {
            format!("Key{upper}")
        };
        Self {
            key: c.to_string(),
            code,
            text: None,
            number: u32::from(upper),
            modifiers,
        }
    }
}

/// Something the person did on the page.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Input {
    /// With the pointer.
    Pointer(Pointer),
    /// A key.
    Key(Key),
    /// Text typed.
    Text(String),
}

/// What the person did, and how long after what they did before.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Recorded {
    /// How long after the input before, in milliseconds; the first's, after
    /// the last step showed.
    pub after_ms: u64,
    /// What they did.
    pub input: Input,
}

/// A step of a workflow shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Its label.
    pub label: String,
    /// Its address, a path of the front end's.
    pub path: String,
    /// A photograph of it to compare it with, as PNG.
    pub kept: Option<PathBuf>,
}

/// A workflow to show: its steps in turn, each photographed once quiet,
/// then what the person did on the last, done again as they did it, and
/// the page photographed once it is done.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Play {
    /// The steps.
    pub steps: Vec<Step>,
    /// What the person did on the last.
    pub inputs: Vec<Recorded>,
    /// A photograph of the page once they did it, to compare with.
    pub after: Option<PathBuf>,
    /// The page's size to show it at, its own when it was kept.
    pub size: Option<(u32, u32, f32)>,
}

/// How long the page is given to settle once what the person did is done
/// again, before it is photographed.
const ACTED_SETTLES: Duration = Duration::from_millis(2500);

/// What the browser is asked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Request {
    /// Go to this address.
    Go(String),
    /// Load the page again.
    Reload,
    /// Back in its history, or forward.
    History(bool),
    /// The person's pointer.
    Pointer(Pointer),
    /// A key.
    Key(Key),
    /// Text typed.
    Text(String),
    /// A page of this size, in its pixels, drawn at this scale.
    Size(u32, u32, f32),
    /// A photograph, under this label.
    Shot(String),
    /// A workflow, its steps' paths whole addresses.
    Play(Play),
}

/// A browser, running, with a page open.
pub struct Browser {
    child: Child,
    program: PathBuf,
    profile: PathBuf,
    requests: Sender<Request>,
    thread: Option<JoinHandle<()>>,
}

impl Browser {
    /// Starts a browser at `address`, `cookies` set for it first, its page
    /// `width` by `height` at `scale`.
    ///
    /// # Errors
    ///
    /// [`PreviewError::NoBrowser`] when none is found, others when it does
    /// not start or its DevTools cannot be reached.
    pub fn start(
        address: &str,
        cookies: &[(String, String)],
        (width, height, scale): (u32, u32, f32),
        sink: Sink,
    ) -> Result<Self, PreviewError> {
        let program = find().ok_or(PreviewError::NoBrowser)?;
        static PROFILES: AtomicU64 = AtomicU64::new(0);
        let profile = std::env::temp_dir().join(format!(
            "{PROFILE_PREFIX}{}-{}",
            std::process::id(),
            PROFILES.fetch_add(1, Ordering::Relaxed)
        ));
        let mut child = Command::new(&program)
            .args([
                "--headless=new",
                "--remote-debugging-port=0",
                &format!("--user-data-dir={}", profile.display()),
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-extensions",
                "--disable-sync",
                "--metrics-recording-only",
                "--mute-audio",
                "--hide-scrollbars",
                "--password-store=basic",
                "--use-mock-keychain",
                &format!("--window-size={width},{height}"),
                "about:blank",
            ])
            // Headless, it needs neither the screen nor the desktop's
            // session, and is kept from both: nothing of the person's
            // desktop is its to reach.
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| PreviewError::Start {
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
            next: 0,
            pending: HashMap::new(),
            main_frame: None,
            sink,
            frames: 0,
            playing: VecDeque::new(),
            step: None,
            inputs: VecDeque::new(),
            next_input: None,
            after: None,
            after_kept: None,
            size: (width, height, scale),
            last_frame: None,
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
    pub(crate) fn profile(&self) -> &std::path::Path {
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
        let _ = std::fs::remove_dir_all(&self.profile);
    }
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
    /// A photograph, under its label, to compare with one kept; of a
    /// step of a workflow, which goes on once it is taken.
    Shot {
        label: String,
        kept: Option<PathBuf>,
        step: bool,
    },
}

/// The socket's side: what is asked, sent; what the browser says, told.
struct Driver {
    socket: WebSocket<TcpStream>,
    session: String,
    next: u64,
    pending: HashMap<u64, Pending>,
    main_frame: Option<String>,
    sink: Sink,
    frames: u64,
    /// The steps of a workflow yet to show.
    playing: VecDeque<Step>,
    /// The step showing, and since when.
    step: Option<(Step, Instant)>,
    /// What the person did, to do again once the steps are shown, and
    /// when the next is due.
    inputs: VecDeque<Recorded>,
    next_input: Option<Instant>,
    /// Once done, when the page is photographed, and what to compare it
    /// with.
    after: Option<(Instant, Option<PathBuf>)>,
    /// The photograph of the end to compare with, until the steps are
    /// shown.
    after_kept: Option<PathBuf>,
    size: (u32, u32, f32),
    /// When the last frame came, until the page is said still.
    last_frame: Option<Instant>,
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

    /// The next message, `None` when none came in a moment.
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

    /// Opens a page of its own, the cookies set, at `address`.
    fn open(&mut self, address: &str, cookies: &[(String, String)]) -> Result<(), PreviewError> {
        let target = self.call("Target.createTarget", json!({"url": "about:blank"}), true)?;
        let id = target["targetId"].as_str().unwrap_or_default().to_owned();
        let attached = self.call(
            "Target.attachToTarget",
            json!({"targetId": id, "flatten": true}),
            true,
        )?;
        self.session = attached["sessionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
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
        self.resize()?;
        self.send("Page.navigate", json!({"url": address}), false)?;
        Ok(())
    }

    /// Sizes the page, and has its frames sent at that size.
    fn resize(&mut self) -> Result<(), PreviewError> {
        let (width, height, scale) = self.size;
        self.send(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": width, "height": height, "deviceScaleFactor": scale, "mobile": false}),
            false,
        )?;
        self.send("Page.stopScreencast", json!({}), false)?;
        let pixels = |points: u32| (f64::from(points) * f64::from(scale)).round();
        self.send(
            "Page.startScreencast",
            json!({"format": "jpeg", "quality": 82, "maxWidth": pixels(width), "maxHeight": pixels(height), "everyNthFrame": 1}),
            false,
        )?;
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
            if self
                .step
                .as_ref()
                .is_some_and(|(_, since)| since.elapsed() > STEP_WITHIN)
                && self.photograph_step().is_err()
            {
                return;
            }
            if self.next_input.is_some_and(|due| Instant::now() >= due)
                && self.input_again().is_err()
            {
                return;
            }
            if self.last_frame.is_some_and(|at| at.elapsed() > STILL_AFTER) {
                self.last_frame = None;
                (self.sink)(Event::Still);
            }
            if let Some((due, _)) = &self.after
                && Instant::now() >= *due
                && let Some((_, kept)) = self.after.take()
                && self.photograph("after".to_owned(), kept).is_err()
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
            Request::Pointer(pointer) => self.pointer(pointer)?,
            Request::Key(key) => self.key(&key)?,
            Request::Text(text) => {
                self.send("Input.insertText", json!({"text": text}), false)?;
            }
            Request::Size(width, height, scale) => {
                if self.size != (width, height, scale) {
                    self.size = (width, height, scale);
                    self.resize()?;
                }
            }
            Request::Shot(label) => self.photograph(label, None)?,
            Request::Play(play) => {
                if let Some(size) = play.size
                    && size != self.size
                {
                    self.size = size;
                    self.resize()?;
                }
                self.playing = play.steps.into();
                self.inputs = play.inputs.into();
                self.after_kept = play.after;
                self.next_input = None;
                self.after = None;
                self.next_step()?;
            }
        }
        Ok(())
    }

    /// Shows the next step of the workflow; once none is left, does again
    /// what the person did.
    fn next_step(&mut self) -> Result<(), PreviewError> {
        self.step = None;
        match self.playing.pop_front() {
            Some(step) => {
                self.send("Page.navigate", json!({"url": step.path}), false)?;
                self.step = Some((step, Instant::now()));
            }
            None => match self.inputs.front() {
                Some(first) => {
                    self.next_input = Some(Instant::now() + Duration::from_millis(first.after_ms));
                }
                None => {
                    if let Some(kept) = self.after_kept.take() {
                        self.after = Some((Instant::now(), Some(kept)));
                    }
                }
            },
        }
        Ok(())
    }

    /// Does again the next thing the person did, and schedules the one
    /// after; the last done, the page is photographed once settled.
    fn input_again(&mut self) -> Result<(), PreviewError> {
        let Some(recorded) = self.inputs.pop_front() else {
            self.next_input = None;
            return Ok(());
        };
        match recorded.input {
            Input::Pointer(pointer) => self.pointer(pointer)?,
            Input::Key(key) => self.key(&key)?,
            Input::Text(text) => {
                self.send("Input.insertText", json!({"text": text}), false)?;
            }
        }
        self.next_input = self
            .inputs
            .front()
            .map(|next| Instant::now() + Duration::from_millis(next.after_ms));
        if self.next_input.is_none() {
            self.after = Some((Instant::now() + ACTED_SETTLES, self.after_kept.take()));
        }
        Ok(())
    }

    /// Photographs the step showing.
    fn photograph_step(&mut self) -> Result<(), PreviewError> {
        if let Some((step, _)) = self.step.take() {
            let id = self.send("Page.captureScreenshot", json!({"format": "png"}), false)?;
            self.pending.insert(
                id,
                Pending::Shot {
                    label: step.label,
                    kept: step.kept,
                    step: true,
                },
            );
        }
        Ok(())
    }

    /// Photographs the page under `label`, to be compared with `kept`.
    fn photograph(&mut self, label: String, kept: Option<PathBuf>) -> Result<(), PreviewError> {
        let id = self.send("Page.captureScreenshot", json!({"format": "png"}), false)?;
        self.pending.insert(
            id,
            Pending::Shot {
                label,
                kept,
                step: false,
            },
        );
        Ok(())
    }

    fn pointer(&mut self, pointer: Pointer) -> Result<(), PreviewError> {
        let name = |button: Button| match button {
            Button::None => "none",
            Button::Left => "left",
            Button::Middle => "middle",
            Button::Right => "right",
        };
        let params = match pointer {
            Pointer::Move { x, y } => json!({"type": "mouseMoved", "x": x, "y": y}),
            Pointer::Press {
                x,
                y,
                button,
                clicks,
            } => {
                json!({"type": "mousePressed", "x": x, "y": y, "button": name(button), "buttons": 1, "clickCount": clicks})
            }
            Pointer::Release {
                x,
                y,
                button,
                clicks,
            } => {
                json!({"type": "mouseReleased", "x": x, "y": y, "button": name(button), "clickCount": clicks})
            }
            Pointer::Wheel { x, y, dx, dy } => {
                json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy})
            }
        };
        self.send("Input.dispatchMouseEvent", params, false)?;
        Ok(())
    }

    fn key(&mut self, key: &Key) -> Result<(), PreviewError> {
        let modifiers = key.modifiers.bits();
        let mut down = json!({
            "type": if key.text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": key.key, "code": key.code,
            "windowsVirtualKeyCode": key.number, "nativeVirtualKeyCode": key.number,
            "modifiers": modifiers,
        });
        if let Some(text) = &key.text {
            down["text"] = Value::String(text.clone());
            down["unmodifiedText"] = Value::String(text.clone());
        }
        self.send("Input.dispatchKeyEvent", down, false)?;
        self.send(
            "Input.dispatchKeyEvent",
            json!({"type": "keyUp", "key": key.key, "code": key.code, "windowsVirtualKeyCode": key.number, "nativeVirtualKeyCode": key.number, "modifiers": modifiers}),
            false,
        )?;
        Ok(())
    }

    /// Tells what a message of the browser says.
    fn told(&mut self, message: &Value) -> Result<(), PreviewError> {
        if let Some(id) = message["id"].as_u64() {
            if let Some(Pending::Shot { label, kept, step }) = self.pending.remove(&id) {
                let png = message["result"]["data"]
                    .as_str()
                    .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
                    .unwrap_or_default();
                if let Some(kept) = kept {
                    let changed = std::fs::read(&kept)
                        .ok()
                        .and_then(|was| crate::kept::difference(&was, &png));
                    (self.sink)(Event::Compared {
                        label: label.clone(),
                        changed,
                    });
                }
                (self.sink)(Event::Shot { label, png });
                if step {
                    self.next_step()?;
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
            "Page.screencastFrame" => {
                let decoded = params["data"]
                    .as_str()
                    .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
                    .and_then(|jpeg| {
                        image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).ok()
                    });
                // Taken: the next may come.
                self.send(
                    "Page.screencastFrameAck",
                    json!({"sessionId": params["sessionId"]}),
                    false,
                )?;
                if let Some(picture) = decoded {
                    let rgba = picture.to_rgba8();
                    self.frames += 1;
                    self.last_frame = Some(Instant::now());
                    (self.sink)(Event::Frame(Frame {
                        width: rgba.width(),
                        height: rgba.height(),
                        rgba: rgba.into_raw(),
                        seq: self.frames,
                    }));
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
                        .as_ref()
                        .is_some_and(|(_, since)| since.elapsed() > STEP_SETTLES)
                    {
                        self.photograph_step()?;
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
                let text: Vec<String> = params["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| {
                        a["value"]
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| a["description"].as_str().map(str::to_owned))
                    })
                    .collect();
                (self.sink)(Event::Console(first_lines(&text.join(" "))));
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
    fn a_page_stays_on_this_machine() {
        assert!(stays_here("http://127.0.0.1:3000/dashboard"));
        assert!(stays_here("http://localhost:5173/"));
        assert!(stays_here("http://app.localhost/"));
        assert!(stays_here("http://[::1]:3000/"));
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
    fn keys_as_the_protocol_names_them() {
        let enter = Key::named("Enter", Modifiers::default()).unwrap();
        assert_eq!((enter.number, enter.text.as_deref()), (13, Some("\r")));
        assert!(Key::named("F13", Modifiers::default()).is_none());
        let select = Key::shortcut(
            'a',
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(
            (select.code.as_str(), select.number, select.modifiers.bits()),
            ("KeyA", 65, 2)
        );
        assert_eq!(Key::shortcut('1', Modifiers::default()).code, "Digit1");
    }
}
