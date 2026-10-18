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

/// How long a step of a workflow may load before it is photographed as it
/// is.
const STEP_WITHIN: Duration = Duration::from_secs(15);

/// How long after a step is asked for its page may be said quiet: what the
/// page before said is not its.
const STEP_SETTLES: Duration = Duration::from_millis(400);

/// A button of the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Each of these addresses in turn, each photographed under its label
    /// once quiet.
    Play(Vec<(String, String)>),
}

/// A browser, running, with a page open.
pub struct Browser {
    child: Child,
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
            "universe-browser-{}-{}",
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
            size: (width, height, scale),
        };
        driver.open(address, cookies)?;
        let thread = std::thread::spawn(move || driver.drive(&asked));
        Ok(Self {
            child,
            profile,
            requests,
            thread: Some(thread),
        })
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
    /// A photograph, under its label.
    Shot(String),
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
    playing: VecDeque<(String, String)>,
    /// The step showing, and since when.
    step: Option<(String, Instant)>,
    size: (u32, u32, f32),
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
            if let Some((label, since)) = &self.step
                && since.elapsed() > STEP_WITHIN
            {
                let label = label.clone();
                if self.photograph(label).is_err() {
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
            Request::Shot(label) => {
                let id = self.send("Page.captureScreenshot", json!({"format": "png"}), false)?;
                self.pending.insert(id, Pending::Shot(label));
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
        if let Some((label, address)) = self.playing.pop_front() {
            self.send("Page.navigate", json!({"url": address}), false)?;
            self.step = Some((label, Instant::now()));
        }
        Ok(())
    }

    /// Photographs the step showing.
    fn photograph(&mut self, label: String) -> Result<(), PreviewError> {
        self.step = None;
        let id = self.send("Page.captureScreenshot", json!({"format": "png"}), false)?;
        self.pending.insert(id, Pending::Shot(label));
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
            if let Some(Pending::Shot(label)) = self.pending.remove(&id) {
                let png = message["result"]["data"]
                    .as_str()
                    .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
                    .unwrap_or_default();
                (self.sink)(Event::Shot { label, png });
                if !self.playing.is_empty() {
                    self.next_step()?;
                }
            }
            return Ok(());
        }
        let params = &message["params"];
        match message["method"].as_str().unwrap_or_default() {
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
                        (self.sink)(Event::Address(url.to_owned()));
                    }
                }
            }
            "Page.navigatedWithinDocument" => {
                if params["frameId"].as_str() == self.main_frame.as_deref()
                    && let Some(url) = params["url"].as_str()
                {
                    (self.sink)(Event::Address(url.to_owned()));
                }
            }
            "Page.lifecycleEvent" => {
                let main = params["frameId"].as_str() == self.main_frame.as_deref();
                if main && params["name"] == "networkIdle" {
                    (self.sink)(Event::Quiet);
                    if let Some((label, since)) = &self.step
                        && since.elapsed() > STEP_SETTLES
                    {
                        let label = label.clone();
                        self.photograph(label)?;
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
                (self.sink)(Event::Console(first_lines(text)));
            }
            _ => {}
        }
        Ok(())
    }
}

/// The first lines of a message: a stack's first frames, not all of it.
fn first_lines(text: &str) -> String {
    text.lines().take(4).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

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
