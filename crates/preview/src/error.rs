//! Why the preview could not do what it was asked.

use std::path::PathBuf;

/// Why the preview could not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    /// The spec is not JSON.
    #[error("the spec is not valid JSON")]
    SpecJson(#[source] serde_json::Error),
    /// The spec is not YAML.
    #[error("the spec is not valid YAML")]
    SpecYaml(#[source] yaml_rust2::ScanError),
    /// The spec has nothing to simulate.
    #[error("the spec declares no paths")]
    NoPaths,
    /// A file could not be read.
    #[error("cannot read {path}")]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// A file could not be written.
    #[error("cannot write {path}")]
    Write {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// No local port could be listened on.
    #[error("cannot listen on a local port")]
    Listen(#[source] std::io::Error),
    /// A program could not be started.
    #[error("cannot start `{command}`")]
    Start {
        /// What was run.
        command: String,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// A program stopped before it did what it was run for.
    #[error("`{command}` stopped before it was ready: {said}")]
    Exited {
        /// What was run.
        command: String,
        /// The last lines it printed.
        said: String,
    },
    /// A command that prints the spec printed none.
    #[error("`{command}` printed no spec: {said}")]
    NoSpec {
        /// What was run.
        command: String,
        /// What it said instead.
        said: String,
    },
    /// The front end did not answer in time.
    #[error("nothing answered on port {port} after {seconds} s")]
    Timeout {
        /// The port waited for.
        port: u16,
        /// How long.
        seconds: u64,
    },
    /// No browser to drive.
    #[error("no Chrome, Chromium or Edge found: install one, or name it in UNIVERSE_CHROME")]
    NoBrowser,
    /// The browser started but never said where its DevTools are.
    #[error("the browser never said where its DevTools listen: {said}")]
    NoDevTools {
        /// What it printed.
        said: String,
    },
    /// The browser's DevTools could not be reached.
    #[error("the browser's DevTools cannot be reached")]
    Socket(#[source] tungstenite::Error),
    /// The browser refused a command of the protocol.
    #[error("the browser refused {method}: {message}")]
    Refused {
        /// The command.
        method: String,
        /// What it said.
        message: String,
    },
}
