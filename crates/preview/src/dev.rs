//! The front end's own server, as its developers run it: its `dev` script
//! through its runner, on a free local port, its API's address pointing at
//! the simulated back end; stopped, with all it started, when dropped.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::error::PreviewError;

/// How long the front end may take to answer on its port: a Flutter app
/// is built for the web before it answers, which takes longer.
const READY_WITHIN: Duration = Duration::from_secs(180);
const FLUTTER_READY_WITHIN: Duration = Duration::from_secs(600);

/// How many of its last lines are kept, to say why it stopped.
const KEPT_LINES: usize = 60;

/// What builds and serves the front end, for the flags its port and host
/// are given with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Framework {
    /// `next dev`: `--port`, `--hostname`.
    Next,
    /// `vite`: `--port`, `--host`, `--strictPort`.
    Vite,
    /// `flutter run -d web-server`: `--web-port`, `--web-hostname`, and
    /// what it reads given as `--dart-define`.
    Flutter,
    /// Anything else: told its port in `PORT`.
    #[default]
    Other,
}

/// How the front end is run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Run {
    /// Its folder.
    pub folder: PathBuf,
    /// What runs it there instead of its `dev` script, `{port}` standing
    /// for its port.
    pub command: Option<String>,
    /// The tool that runs its scripts: `pnpm`, `npm`, `yarn`, `bun`.
    pub runner: String,
    /// What builds and serves it.
    pub framework: Framework,
    /// The variables of its environment, `${NAME}` in a value standing for
    /// that variable's value here.
    pub env: Vec<(String, String)>,
    /// The variables it reads its own address from, given the address it
    /// is served at.
    pub own_env: Vec<String>,
}

/// The line that runs the front end on `port`: its own command, else its
/// `dev` script, the flags of its port and host passed through its runner.
///
/// # Examples
///
/// ```
/// use universe_preview::{Framework, Run, command_line};
/// let run = Run { runner: "npm".into(), framework: Framework::Vite, ..Run::default() };
/// assert_eq!(
///     command_line(&run, 5199),
///     "npm run dev -- --port 5199 --host 127.0.0.1 --strictPort"
/// );
/// ```
#[must_use]
pub fn command_line(run: &Run, port: u16) -> String {
    line(run, &port.to_string())
}

/// The line that runs the front end, as shown before it runs: `<port>`
/// for the port it will be given.
#[must_use]
pub fn command_shown(run: &Run) -> String {
    line(run, "<port>")
}

fn line(run: &Run, port: &str) -> String {
    if let Some(command) = &run.command {
        return command.replace("{port}", port);
    }
    if run.framework == Framework::Flutter {
        return format!("flutter run -d web-server --web-port {port} --web-hostname 127.0.0.1");
    }
    let flags = match run.framework {
        Framework::Next => format!("--port {port} --hostname 127.0.0.1"),
        Framework::Vite => format!("--port {port} --host 127.0.0.1 --strictPort"),
        Framework::Other | Framework::Flutter => String::new(),
    };
    let runner = if run.runner.is_empty() {
        "npm"
    } else {
        run.runner.as_str()
    };
    match (runner, flags.is_empty()) {
        (_, true) => format!("{runner} run dev"),
        // npm passes on only what follows `--`.
        ("npm", false) => format!("npm run dev -- {flags}"),
        (_, false) => format!("{runner} run dev {flags}"),
    }
}

/// A port free on `127.0.0.1` now.
///
/// # Errors
///
/// [`PreviewError::Listen`] when none can be had.
pub fn free_port() -> Result<u16, PreviewError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(PreviewError::Listen)?;
    Ok(listener.local_addr().map_err(PreviewError::Listen)?.port())
}

/// `value` with each `${NAME}` it holds put in from the environment.
fn expanded(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                out.push_str(&std::env::var(&after[..end]).unwrap_or_default());
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The front end's server, running.
pub struct DevServer {
    child: Child,
    port: u16,
    within: Duration,
    line: String,
    said: Arc<Mutex<VecDeque<String>>>,
}

impl DevServer {
    /// Starts the front end on `port`, its API's address in `api_env` set
    /// to `api`, each line it prints given to `lines`.
    ///
    /// # Errors
    ///
    /// [`PreviewError::Start`] when it cannot be started.
    pub fn start(
        run: &Run,
        port: u16,
        api: Option<(&str, &str)>,
        lines: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<Self, PreviewError> {
        let mut line = command_line(run, port);
        // A Flutter app built for the web reads no environment: what it
        // reads is given at its build.
        if run.framework == Framework::Flutter {
            let own = format!("http://127.0.0.1:{port}");
            let defines = api
                .map(|(name, address)| (name.to_owned(), address.to_owned()))
                .into_iter()
                .chain(run.own_env.iter().map(|name| (name.clone(), own.clone())));
            for (name, value) in defines {
                line.push_str(&format!(" --dart-define={name}={value}"));
            }
        }
        let mut command = shell(&line);
        command
            .current_dir(&run.folder)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PORT", port.to_string())
            .env("BROWSER", "none")
            .env("NEXT_TELEMETRY_DISABLED", "1")
            .env("FORCE_COLOR", "0")
            .env("NO_COLOR", "1");
        if let Some((name, address)) = api {
            command.env(name, address);
        }
        for name in &run.own_env {
            command.env(name, format!("http://127.0.0.1:{port}"));
        }
        for (name, value) in &run.env {
            command.env(name, expanded(value));
        }
        // Its own group, so that what it starts stops with it.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().map_err(|source| PreviewError::Start {
            command: line.clone(),
            source,
        })?;
        let said: Arc<Mutex<VecDeque<String>>> = Arc::default();
        if let Some(out) = child.stdout.take() {
            follow(out, Arc::clone(&said), Arc::clone(&lines));
        }
        if let Some(err) = child.stderr.take() {
            follow(err, Arc::clone(&said), lines);
        }
        Ok(Self {
            child,
            port,
            within: if run.framework == Framework::Flutter {
                FLUTTER_READY_WITHIN
            } else {
                READY_WITHIN
            },
            line,
            said,
        })
    }

    /// Waits until it answers on its port.
    ///
    /// # Errors
    ///
    /// [`PreviewError::Exited`] when it stops first, [`PreviewError::Timeout`]
    /// when it takes too long.
    pub fn ready(&mut self) -> Result<(), PreviewError> {
        let began = Instant::now();
        loop {
            if TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], self.port)),
                Duration::from_millis(300),
            )
            .is_ok()
            {
                return Ok(());
            }
            if let Ok(Some(_)) = self.child.try_wait() {
                // What it printed last may still be on its way.
                std::thread::sleep(Duration::from_millis(200));
                return Err(PreviewError::Exited {
                    command: self.line.clone(),
                    said: self.said(),
                });
            }
            if began.elapsed() > self.within {
                return Err(PreviewError::Timeout {
                    port: self.port,
                    seconds: self.within.as_secs(),
                });
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// The port it serves on.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The id of its first process, which leads the group of the others.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The line it was started with.
    #[must_use]
    pub fn line(&self) -> &str {
        &self.line
    }

    /// The last lines it printed.
    #[must_use]
    pub fn said(&self) -> String {
        let said = self.said.lock().unwrap_or_else(PoisonError::into_inner);
        let start = said.len().saturating_sub(12);
        said.iter()
            .skip(start)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Drop for DevServer {
    fn drop(&mut self) {
        // The whole group: the runner, and the servers it started, which
        // outlive it otherwise. Asked to stop, then made to.
        #[cfg(unix)]
        {
            let group = self.child.id();
            signal_group(group, "TERM");
            let began = Instant::now();
            while began.elapsed() < Duration::from_secs(3) {
                if let Ok(Some(_)) = self.child.try_wait() {
                    // What it started may take a moment more.
                    std::thread::sleep(Duration::from_millis(300));
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            signal_group(group, "KILL");
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Sends `signal` to the process group `group`, never to `0` (the
/// caller's) nor `1` (every process there is). The negative number goes
/// after `--`: before it, `kill` takes `-1998` for the option `-1`, and
/// signals every process of the person.
#[cfg(unix)]
fn signal_group(group: u32, signal: &str) {
    if group < 2 {
        return;
    }
    let _ = Command::new("kill")
        .args(["-s", signal, "--", &format!("-{group}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// A command that runs `line` in the system's shell.
fn shell(line: &str) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", line]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", line]);
        command
    }
}

/// Reads what a stream prints, line by line, its colours taken out.
fn follow(
    stream: impl Read + Send + 'static,
    said: Arc<Mutex<VecDeque<String>>>,
    lines: Arc<dyn Fn(String) + Send + Sync>,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                break;
            };
            let line = plain(&line);
            if line.trim().is_empty() {
                continue;
            }
            {
                let mut said = said.lock().unwrap_or_else(PoisonError::into_inner);
                said.push_back(line.clone());
                while said.len() > KEPT_LINES {
                    said.pop_front();
                }
            }
            lines(line);
        }
    });
}

/// A line without the escapes that colour it.
fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_that_runs_it_passes_its_port_through_its_runner() {
        let run = |runner: &str, framework| Run {
            runner: runner.to_owned(),
            framework,
            ..Run::default()
        };
        assert_eq!(
            command_line(&run("pnpm", Framework::Next), 3100),
            "pnpm run dev --port 3100 --hostname 127.0.0.1"
        );
        assert_eq!(
            command_line(&run("yarn", Framework::Other), 3100),
            "yarn run dev"
        );
        assert_eq!(
            command_line(&run("flutter", Framework::Flutter), 3100),
            "flutter run -d web-server --web-port 3100 --web-hostname 127.0.0.1"
        );
        let own = Run {
            command: Some("make serve PORT={port}".to_owned()),
            ..Run::default()
        };
        assert_eq!(command_line(&own, 3100), "make serve PORT=3100");
        assert_eq!(command_shown(&own), "make serve PORT=<port>");
    }

    #[test]
    fn a_value_puts_in_the_environment_it_names() {
        let path = std::env::var("PATH").unwrap_or_default();
        assert_eq!(
            expanded("/opt/node/bin:${PATH}"),
            format!("/opt/node/bin:{path}")
        );
        assert_eq!(expanded("${"), "${");
        assert_eq!(plain("\u{1b}[32m✓\u{1b}[39m Ready"), "✓ Ready");
    }

    /// Whether any process of `group` is left.
    #[cfg(unix)]
    fn group_left(group: u32) -> bool {
        Command::new("kill")
            .args(["-s", "0", "--", &format!("-{group}")])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    #[cfg(unix)]
    #[test]
    fn what_it_started_stops_with_it_and_nothing_else() {
        // A runner that starts a server and goes on: the server is what
        // must stop too.
        let run = Run {
            folder: std::env::temp_dir(),
            command: Some("sleep 120 & echo started; wait".to_owned()),
            ..Run::default()
        };
        let lines: Arc<dyn Fn(String) + Send + Sync> = Arc::new(|_| {});
        let server = DevServer::start(&run, free_port().unwrap(), None, lines).unwrap();
        let group = server.child.id();
        std::thread::sleep(Duration::from_millis(300));
        assert!(group_left(group));
        drop(server);
        assert!(!group_left(group));
    }

    #[cfg(unix)]
    #[test]
    fn it_is_ready_once_its_port_answers_and_says_why_it_stopped() {
        let port = free_port().unwrap();
        let said: Arc<Mutex<Vec<String>>> = Arc::default();
        let kept = Arc::clone(&said);
        let lines: Arc<dyn Fn(String) + Send + Sync> =
            Arc::new(move |l| kept.lock().unwrap().push(l));
        // A server of its own stands in for the front end's.
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        let run = Run {
            folder: std::env::temp_dir(),
            command: Some("echo starting on {port}; sleep 30".to_owned()),
            ..Run::default()
        };
        let mut server = DevServer::start(
            &run,
            port,
            Some(("API_URL", "http://127.0.0.1:1")),
            Arc::clone(&lines),
        )
        .unwrap();
        server.ready().unwrap();
        drop(listener);
        drop(server);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            said.lock()
                .unwrap()
                .iter()
                .any(|l| l == &format!("starting on {port}"))
        );

        let run = Run {
            folder: std::env::temp_dir(),
            command: Some("echo no node here >&2; exit 3".to_owned()),
            ..Run::default()
        };
        let mut server = DevServer::start(&run, free_port().unwrap(), None, lines).unwrap();
        let error = server.ready().unwrap_err();
        assert!(matches!(error, PreviewError::Exited { .. }));
        assert!(error.to_string().contains("no node here"));
    }
}
