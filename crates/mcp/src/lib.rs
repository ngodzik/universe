//! A client of the Model Context Protocol: the servers the settings declare
//! (`[mcp.<name>]`, a URL or a command), each started on a task of its own,
//! and their tools offered to models as `<server>__<tool>`.
//!
//! Servers are reached two ways: streamable HTTP, a JSON-RPC message per
//! POST answered as JSON or as server-sent events, with fixed headers or
//! OAuth; and stdio, a JSON-RPC message per line. Nothing waits on a server
//! that does not answer: its tools are offered once it does, and `/mcp`
//! says how each answered.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod oauth;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use universe_core::{ExternalCall, ExternalTools, McpServer, ToolSpec};

/// The protocol's version this client speaks.
const PROTOCOL: &str = "2025-06-18";

/// How long a server may take to answer one message.
const ANSWER_WITHIN: Duration = Duration::from_secs(60);

/// Between a server's name and its tool's.
const SEPARATOR: &str = "__";

/// A tool a server offers.
#[derive(Debug, Clone, PartialEq)]
struct RemoteTool {
    name: String,
    description: String,
    schema: Value,
    read_only: bool,
}

/// Where a server is.
#[derive(Debug, Clone, PartialEq)]
enum State {
    Connecting,
    /// Waiting for the person's consent, on this page.
    Consenting(String),
    Ready(Vec<RemoteTool>),
    Failed(String),
}

/// The way to a server.
enum Link {
    Stdio {
        // Kept so that the program is stopped with the link.
        _child: Box<Child>,
        stdin: ChildStdin,
        lines: Lines<BufReader<ChildStdout>>,
    },
    Http {
        client: reqwest::Client,
        url: String,
        headers: BTreeMap<String, String>,
        /// The OAuth token sent, when the server takes one.
        bearer: Option<String>,
        /// The server's last answer was 401: what was sent is not taken.
        refused: bool,
        session: Option<String>,
        initialized: bool,
    },
}

impl Link {
    /// Starts the program or prepares the HTTP client.
    fn open(config: &McpServer) -> Result<Self, String> {
        if let Some(url) = &config.url {
            return Ok(Self::Http {
                client: reqwest::Client::new(),
                url: url.clone(),
                headers: config.headers.clone(),
                bearer: None,
                refused: false,
                session: None,
                initialized: false,
            });
        }
        let (program, args) = config
            .command
            .split_first()
            .ok_or("neither a url nor a command is given")?;
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .envs(&config.env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("{program} could not be started: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        Ok(Self::Stdio {
            _child: Box::new(child),
            stdin,
            lines: BufReader::new(stdout).lines(),
        })
    }

    /// The OAuth token sent from now on.
    fn set_bearer(&mut self, token: String) {
        if let Self::Http { bearer, .. } = self {
            *bearer = Some(token);
        }
    }

    /// Whether the server refused what was sent, with a 401.
    fn was_refused(&self) -> bool {
        matches!(self, Self::Http { refused: true, .. })
    }

    /// Sends a request and waits for its answer's result.
    async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let answer = tokio::time::timeout(ANSWER_WITHIN, self.exchange(&message, Some(id)))
            .await
            .map_err(|_| format!("no answer to {method} in a minute"))??;
        match answer {
            Some(answer) => result_of(answer),
            None => Err(format!("no answer to {method}")),
        }
    }

    /// Sends a notification, which has no answer.
    async fn notify(&mut self, method: &str) -> Result<(), String> {
        let message = json!({"jsonrpc": "2.0", "method": method});
        tokio::time::timeout(ANSWER_WITHIN, self.exchange(&message, None))
            .await
            .map_err(|_| format!("{method} was not taken in a minute"))?
            .map(|_| ())
    }

    /// Sends `message` and, for a request, reads until the answer of `id`.
    async fn exchange(
        &mut self,
        message: &Value,
        id: Option<u64>,
    ) -> Result<Option<Value>, String> {
        match self {
            Self::Stdio { stdin, lines, .. } => {
                let mut text = message.to_string();
                text.push('\n');
                stdin
                    .write_all(text.as_bytes())
                    .await
                    .map_err(|e| format!("the server stopped: {e}"))?;
                stdin.flush().await.map_err(|e| e.to_string())?;
                let Some(id) = id else {
                    return Ok(None);
                };
                // What else it says meanwhile, logs or its own requests, is
                // passed over.
                while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
                    if let Ok(answer) = serde_json::from_str::<Value>(&line)
                        && answers(&answer, id)
                    {
                        return Ok(Some(answer));
                    }
                }
                Err("the server stopped".to_owned())
            }
            Self::Http {
                client,
                url,
                headers,
                bearer,
                refused,
                session,
                initialized,
            } => {
                let mut post = client
                    .post(url.as_str())
                    .header("Accept", "application/json, text/event-stream")
                    .json(message);
                for (name, value) in headers.iter() {
                    post = post.header(name.as_str(), value.as_str());
                }
                if let Some(token) = bearer.as_deref() {
                    post = post.bearer_auth(token);
                }
                if let Some(session) = session.as_deref() {
                    post = post.header("Mcp-Session-Id", session);
                }
                if *initialized {
                    post = post.header("MCP-Protocol-Version", PROTOCOL);
                }
                let response = post.send().await.map_err(|e| format!("{url}: {e}"))?;
                if let Some(given) = response
                    .headers()
                    .get("Mcp-Session-Id")
                    .and_then(|v| v.to_str().ok())
                {
                    *session = Some(given.to_owned());
                }
                let status = response.status();
                *refused = status == reqwest::StatusCode::UNAUTHORIZED;
                let events = response
                    .headers()
                    .get("Content-Type")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|t| t.starts_with("text/event-stream"));
                let body = response.text().await.map_err(|e| format!("{url}: {e}"))?;
                if !status.is_success() {
                    return Err(format!(
                        "{url} answered HTTP {}: {}",
                        status.as_u16(),
                        first_line(&body)
                    ));
                }
                if message["method"] == "initialize" {
                    *initialized = true;
                }
                let Some(id) = id else {
                    return Ok(None);
                };
                let candidates: Vec<Value> = if events {
                    sse_data(&body)
                        .iter()
                        .filter_map(|data| serde_json::from_str(data).ok())
                        .collect()
                } else {
                    serde_json::from_str(&body).ok().into_iter().collect()
                };
                Ok(candidates.into_iter().find(|a| answers(a, id)))
            }
        }
    }
}

/// Whether `message` answers the request `id`.
fn answers(message: &Value, id: u64) -> bool {
    message["id"].as_u64() == Some(id)
        && (message.get("result").is_some() || message.get("error").is_some())
}

/// An answer's result, or its error said.
fn result_of(answer: Value) -> Result<Value, String> {
    if let Some(error) = answer.get("error") {
        return Err(error["message"]
            .as_str()
            .map_or_else(|| error.to_string(), str::to_owned));
    }
    Ok(answer["result"].clone())
}

/// The data of each server-sent event of `body`, its lines joined.
fn sse_data(body: &str) -> Vec<String> {
    let mut events = Vec::new();
    let mut data: Vec<&str> = Vec::new();
    for line in body.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !data.is_empty() {
                events.push(data.join("\n"));
                data.clear();
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    events
}

fn first_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

/// A tool call's result as text: its text parts, what else it holds named.
fn call_text(result: &Value) -> Result<String, String> {
    let parts: Vec<String> = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|part| match part["type"].as_str() {
            Some("text") => part["text"].as_str().unwrap_or("").to_owned(),
            Some("resource") => part["resource"]["text"]
                .as_str()
                .map_or_else(|| "[a resource]".to_owned(), str::to_owned),
            Some(other) => format!("[{other}]"),
            None => String::new(),
        })
        .collect();
    let mut text = parts.join("\n");
    if text.is_empty()
        && let Some(structured) = result.get("structuredContent")
    {
        text = structured.to_string();
    }
    if result["isError"].as_bool() == Some(true) {
        Err(text)
    } else {
        Ok(text)
    }
}

/// A name as model providers take it: letters, digits, `_` and `-`.
fn plain(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// One server declared in the settings.
struct Server {
    name: String,
    config: McpServer,
    state: Mutex<State>,
    link: tokio::sync::Mutex<Option<Link>>,
    next_id: AtomicU64,
    /// Where its OAuth grant is kept, when it takes OAuth.
    grants: Option<oauth::Grants>,
    /// What opens the consent page: the browser, but in tests.
    open: fn(&str) -> bool,
    /// What the person should hear of, shared by every server.
    notices: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn set(&self, state: State) {
        if let Ok(mut current) = self.state.lock() {
            *current = state;
        }
    }

    fn state(&self) -> State {
        self.state
            .lock()
            .map_or_else(|_| State::Failed("poisoned".into()), |s| s.clone())
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Opens the link, says hello, and lists the tools. A grant the server
    /// no longer takes is forgotten, and the person asked once more.
    async fn connect(&self) {
        let outcome = async {
            let mut link = Link::open(&self.config)?;
            if self.config.oauth {
                link.set_bearer(self.token(false).await?);
            }
            let tools = match self.hello(&mut link).await {
                Err(_) if self.config.oauth && link.was_refused() => {
                    if let Some(grants) = &self.grants {
                        grants.forget(&self.name);
                    }
                    link.set_bearer(self.token(true).await?);
                    self.hello(&mut link).await?
                }
                other => other?,
            };
            Ok::<_, String>((link, tools))
        }
        .await;
        match outcome {
            Ok((link, tools)) => {
                *self.link.lock().await = Some(link);
                self.set(State::Ready(tools));
            }
            Err(why) => self.set(State::Failed(why)),
        }
    }

    /// The OAuth token to send: the grant kept, renewed when it is about
    /// to end, or the person's consent asked for when there is none, it
    /// cannot be renewed, or `again`.
    async fn token(&self, again: bool) -> Result<String, String> {
        let grants = self
            .grants
            .as_ref()
            .ok_or("no folder to keep its grant in")?;
        let url = self.config.url.as_deref().ok_or("OAuth needs a url")?;
        let client = reqwest::Client::new();
        if !again && let Some(grant) = grants.load(&self.name) {
            if !grant.is_ending(oauth::now()) {
                return Ok(grant.access_token);
            }
            if let Ok(renewed) = oauth::renew(&client, &grant).await {
                grants.save(&self.name, &renewed)?;
                return Ok(renewed.access_token);
            }
        }
        let grant = oauth::consent(
            &client,
            url,
            |page| {
                self.set(State::Consenting(page.to_owned()));
                if let Ok(mut notices) = self.notices.lock() {
                    notices.push(format!(
                        "The MCP server {} waits for your consent in the browser; if it did not \
                         open: {page}",
                        self.name
                    ));
                }
            },
            self.open,
        )
        .await?;
        self.set(State::Connecting);
        grants.save(&self.name, &grant)?;
        Ok(grant.access_token)
    }

    /// The token renewed, when the grant kept is about to end.
    async fn renewed(&self) -> Option<String> {
        let grants = self.grants.as_ref()?;
        let grant = grants.load(&self.name)?;
        if !grant.is_ending(oauth::now()) {
            return None;
        }
        let renewed = oauth::renew(&reqwest::Client::new(), &grant).await.ok()?;
        grants.save(&self.name, &renewed).ok()?;
        Some(renewed.access_token)
    }

    /// Says hello on `link` and lists the tools.
    async fn hello(&self, link: &mut Link) -> Result<Vec<RemoteTool>, String> {
        link.request(
            self.id(),
            "initialize",
            json!({
                "protocolVersion": PROTOCOL,
                "capabilities": {},
                "clientInfo": {"name": "universe", "version": env!("CARGO_PKG_VERSION")},
            }),
        )
        .await?;
        link.notify("notifications/initialized").await?;
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor.map_or_else(|| json!({}), |c| json!({"cursor": c}));
            let page = link.request(self.id(), "tools/list", params).await?;
            for tool in page["tools"].as_array().into_iter().flatten() {
                let Some(name) = tool["name"].as_str() else {
                    continue;
                };
                tools.push(RemoteTool {
                    name: name.to_owned(),
                    description: tool["description"].as_str().unwrap_or("").to_owned(),
                    schema: tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"})),
                    read_only: tool["annotations"]["readOnlyHint"].as_bool() == Some(true),
                });
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// Calls one of its tools. A call the server refuses with a 401
    /// reconnects, renewing or asking for the grant, and is tried once
    /// more.
    async fn call(&self, tool: &str, arguments: Value) -> Result<String, String> {
        let first = self.call_once(tool, arguments.clone()).await;
        let refused = self.config.oauth
            && first.is_err()
            && self
                .link
                .lock()
                .await
                .as_ref()
                .is_some_and(Link::was_refused);
        if !refused {
            return first;
        }
        self.connect().await;
        self.call_once(tool, arguments).await
    }

    async fn call_once(&self, tool: &str, arguments: Value) -> Result<String, String> {
        let mut link = self.link.lock().await;
        let Some(open) = link.as_mut() else {
            return Err(format!("{} is not connected", self.name));
        };
        if self.config.oauth
            && let Some(token) = self.renewed().await
        {
            open.set_bearer(token);
        }
        let id = self.id();
        match open
            .request(
                id,
                "tools/call",
                json!({"name": tool, "arguments": arguments}),
            )
            .await
        {
            Ok(result) => call_text(&result),
            Err(why) => Err(why),
        }
    }
}

/// The servers the settings declare, and their tools.
#[derive(Clone)]
pub struct Mcp {
    servers: Vec<Arc<Server>>,
    notices: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for Mcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.servers.iter().map(|s| (&s.name, &s.config)))
            .finish()
    }
}

impl Mcp {
    /// Starts every server of `declared`, each on a task of its own: their
    /// tools are offered as each answers. The OAuth grants of those that
    /// take it are kept in `grants`, one file each, readable by their owner
    /// only. Needs a Tokio runtime.
    #[must_use]
    pub fn start(declared: &BTreeMap<String, McpServer>, grants: Option<PathBuf>) -> Self {
        Self::start_with(declared, grants, oauth::open_browser)
    }

    /// As [`Mcp::start`], the consent page opened by `open`.
    fn start_with(
        declared: &BTreeMap<String, McpServer>,
        grants: Option<PathBuf>,
        open: fn(&str) -> bool,
    ) -> Self {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let servers: Vec<Arc<Server>> = declared
            .iter()
            .map(|(name, config)| {
                Arc::new(Server {
                    name: plain(name),
                    config: config.clone(),
                    state: Mutex::new(State::Connecting),
                    link: tokio::sync::Mutex::new(None),
                    next_id: AtomicU64::new(1),
                    grants: grants.clone().map(oauth::Grants::new),
                    open,
                    notices: Arc::clone(&notices),
                })
            })
            .collect();
        for server in &servers {
            let server = Arc::clone(server);
            tokio::spawn(async move { server.connect().await });
        }
        Self { servers, notices }
    }

    /// Whether the settings declare any server.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Waits until every server answered or failed, `within` at most.
    pub async fn settled(&self, within: Duration) {
        let _ = tokio::time::timeout(within, async {
            while self.servers.iter().any(|s| s.state() == State::Connecting) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
    }

    fn find(&self, name: &str) -> Option<(&Arc<Server>, RemoteTool)> {
        let (server, tool) = name.split_once(SEPARATOR)?;
        let server = self.servers.iter().find(|s| s.name == server)?;
        let State::Ready(tools) = server.state() else {
            return None;
        };
        let tool = tools.into_iter().find(|t| t.name == tool)?;
        Some((server, tool))
    }
}

impl ExternalTools for Mcp {
    fn specs(&self) -> Vec<ToolSpec> {
        self.servers
            .iter()
            .flat_map(|server| match server.state() {
                State::Ready(tools) => tools
                    .into_iter()
                    .map(|tool| ToolSpec {
                        name: format!("{}{SEPARATOR}{}", server.name, tool.name),
                        description: if tool.description.is_empty() {
                            format!("A tool of the MCP server {}.", server.name)
                        } else {
                            tool.description
                        },
                        parameters: tool.schema,
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    fn reads_only(&self, name: &str) -> Option<bool> {
        self.find(name).map(|(_, tool)| tool.read_only)
    }

    fn call<'a>(&'a self, name: &'a str, arguments: &'a str) -> ExternalCall<'a> {
        Box::pin(async move {
            let (server, tool) = self.find(name).ok_or_else(|| format!("no tool {name}"))?;
            let arguments: Value = if arguments.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(arguments)
                    .map_err(|e| format!("the arguments are not valid JSON: {e}"))?
            };
            server.call(&tool.name, arguments).await
        })
    }

    fn status(&self) -> Vec<(String, String)> {
        self.servers
            .iter()
            .map(|server| {
                let how = match server.state() {
                    State::Connecting => "connecting".to_owned(),
                    State::Consenting(page) => format!(
                        "waiting for your consent in the browser; if it did not open: {page}"
                    ),
                    State::Ready(tools) => {
                        let read = tools.iter().filter(|t| t.read_only).count();
                        format!(
                            "{} tool{}, {read} only reading",
                            tools.len(),
                            if tools.len() == 1 { "" } else { "s" }
                        )
                    }
                    State::Failed(why) => format!("failed: {why}"),
                };
                (server.name.clone(), how)
            })
            .collect()
    }

    fn notices(&self) -> Vec<String> {
        self.notices
            .lock()
            .map(|mut n| std::mem::take(&mut *n))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// An MCP server over HTTP, as small as a test needs: it answers
    /// `initialize`, `tools/list` and `tools/call`, the list as server-sent
    /// events, and checks the session it gave is sent back.
    async fn serve() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, body) = loop {
                        let n = socket.read(&mut chunk).await.unwrap();
                        buffer.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buffer).to_string();
                        if let Some((head, body)) = text.split_once("\r\n\r\n") {
                            let length: usize = head
                                .lines()
                                .find_map(|l| {
                                    l.to_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse().unwrap())
                                })
                                .unwrap_or(0);
                            if body.len() >= length {
                                break (head.to_lowercase(), body[..length].to_owned());
                            }
                        }
                    };
                    let message: Value = serde_json::from_str(&body).unwrap();
                    let id = message["id"].clone();
                    let (content_type, reply) = match message["method"].as_str().unwrap() {
                        "initialize" => (
                            "application/json",
                            json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": PROTOCOL}})
                                .to_string(),
                        ),
                        "tools/list" => {
                            assert!(head.contains("mcp-session-id: s-1"), "{head}");
                            assert!(head.contains("authorization: bearer t"), "{head}");
                            let list = json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [
                                {"name": "get_issue", "description": "An issue.",
                                 "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": true}},
                                {"name": "create_issue", "inputSchema": {"type": "object"}},
                            ]}});
                            ("text/event-stream", format!("event: message\ndata: {list}\n\n"))
                        }
                        "tools/call" => (
                            "application/json",
                            json!({"jsonrpc": "2.0", "id": id, "result": {"content": [
                                {"type": "text", "text": format!("issue {}", message["params"]["arguments"]["id"])}
                            ]}})
                            .to_string(),
                        ),
                        _ => ("application/json", String::new()),
                    };
                    let status = if reply.is_empty() {
                        "202 Accepted"
                    } else {
                        "200 OK"
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nMcp-Session-Id: s-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        url
    }

    #[tokio::test]
    async fn a_servers_tools_are_offered_prefixed_and_called() {
        let url = serve().await;
        let declared = BTreeMap::from([(
            "tracker".to_owned(),
            McpServer {
                url: Some(url),
                headers: BTreeMap::from([("Authorization".into(), "Bearer t".into())]),
                ..McpServer::default()
            },
        )]);
        let mcp = Mcp::start(&declared, None);
        mcp.settled(Duration::from_secs(5)).await;
        assert_eq!(
            mcp.status(),
            [("tracker".to_owned(), "2 tools, 1 only reading".to_owned())]
        );
        let names: Vec<String> = mcp.specs().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["tracker__get_issue", "tracker__create_issue"]);
        assert_eq!(mcp.reads_only("tracker__get_issue"), Some(true));
        assert_eq!(mcp.reads_only("tracker__create_issue"), Some(false));
        assert_eq!(mcp.reads_only("other__x"), None);
        assert_eq!(
            mcp.call("tracker__get_issue", r#"{"id": "ENG-1"}"#).await,
            Ok("issue \"ENG-1\"".to_owned())
        );
    }

    /// How many times a test's consent page was opened.
    static CONSENTS: AtomicU64 = AtomicU64::new(0);

    /// The person consenting at once: the browser goes back to the
    /// callback with the PKCE challenge as its code, for the token
    /// endpoint to check against the verifier.
    fn consent_at_once(page: &str) -> bool {
        CONSENTS.fetch_add(1, Ordering::SeqCst);
        let page = reqwest::Url::parse(page).unwrap();
        let param = |name: &str| {
            page.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
                .unwrap()
        };
        let redirect = reqwest::Url::parse(&param("redirect_uri")).unwrap();
        let target = format!(
            "{}?code={}&state={}",
            redirect.path(),
            param("code_challenge"),
            param("state")
        );
        let address = format!("127.0.0.1:{}", redirect.port().unwrap());
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut socket = std::net::TcpStream::connect(address).unwrap();
            write!(socket, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
            let mut answer = String::new();
            let _ = socket.read_to_string(&mut answer);
        });
        true
    }

    /// One request read whole: its first line, headers in lower case, body.
    async fn read_request(socket: &mut tokio::net::TcpStream) -> (String, String, String) {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = socket.read(&mut chunk).await.unwrap();
            buffer.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buffer).to_string();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let length: usize = head
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if body.len() >= length || n == 0 {
                    let first = head.lines().next().unwrap_or("").to_owned();
                    return (
                        first,
                        head.to_lowercase(),
                        body[..length.min(body.len())].to_owned(),
                    );
                }
            }
            if n == 0 {
                return (String::new(), String::new(), String::new());
            }
        }
    }

    /// An MCP server that takes OAuth only, and its authorization server,
    /// at one address: discovery, registration, the token endpoint, and the
    /// MCP endpoint, which takes the tokens in `accepted`.
    async fn serve_oauth(accepted: &'static [&'static str]) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let origin = base.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let base = origin.clone();
                tokio::spawn(async move {
                    let (first, head, body) = read_request(&mut socket).await;
                    let target = first.split(' ').nth(1).unwrap_or("").to_owned();
                    let field = |name: &str| {
                        body.split('&')
                            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
                            .map(|v| v.replace("%3A", ":").replace("%2F", "/"))
                    };
                    let json = |value: Value| (200, "application/json", value.to_string());
                    let (status, content_type, reply) = match target.as_str() {
                        "/.well-known/oauth-protected-resource" => json(json!({
                            "resource": format!("{base}/mcp"),
                            "authorization_servers": [base],
                        })),
                        "/.well-known/oauth-authorization-server" => json(json!({
                            "issuer": base,
                            "authorization_endpoint": format!("{base}/authorize"),
                            "token_endpoint": format!("{base}/token"),
                            "registration_endpoint": format!("{base}/register"),
                            "code_challenge_methods_supported": ["S256"],
                        })),
                        "/register" => {
                            let asked: Value = serde_json::from_str(&body).unwrap();
                            assert_eq!(asked["token_endpoint_auth_method"], "none");
                            json(json!({"client_id": "c-1"}))
                        }
                        "/token" => match field("grant_type").as_deref() {
                            Some("authorization_code") => {
                                let verifier = field("code_verifier").unwrap();
                                assert_eq!(field("code"), Some(oauth::challenge_of(&verifier)));
                                assert_eq!(field("client_id").as_deref(), Some("c-1"));
                                assert_eq!(field("resource"), Some(format!("{base}/mcp")));
                                json(
                                    json!({"access_token": "t-1", "refresh_token": "r-1", "expires_in": 3600}),
                                )
                            }
                            Some("refresh_token") => {
                                assert_eq!(field("refresh_token").as_deref(), Some("r-old"));
                                json(json!({"access_token": "t-2", "expires_in": 3600}))
                            }
                            other => panic!("grant {other:?}"),
                        },
                        "/mcp" => {
                            let taken = accepted
                                .iter()
                                .any(|t| head.contains(&format!("authorization: bearer {t}")));
                            if taken {
                                let message: Value = serde_json::from_str(&body).unwrap();
                                let id = message["id"].clone();
                                match message["method"].as_str().unwrap() {
                                    "initialize" => {
                                        json(json!({"jsonrpc": "2.0", "id": id, "result": {}}))
                                    }
                                    "tools/list" => json(
                                        json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [
                                            {"name": "fetch", "inputSchema": {"type": "object"}}
                                        ]}}),
                                    ),
                                    "tools/call" => {
                                        json(json!({"jsonrpc": "2.0", "id": id, "result": {
                                            "content": [{"type": "text", "text": "a page"}]
                                        }}))
                                    }
                                    _ => (202, "application/json", String::new()),
                                }
                            } else {
                                (401, "application/json", String::new())
                            }
                        }
                        _ => (404, "text/plain", String::new()),
                    };
                    let reason = match status {
                        200 => "OK",
                        202 => "Accepted",
                        401 => "Unauthorized",
                        _ => "Not Found",
                    };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        base
    }

    /// Waits until the server is ready or failed, past the consent that
    /// `settled` does not wait for.
    async fn ready(mcp: &Mcp) -> String {
        for _ in 0..500 {
            let how = mcp.status()[0].1.clone();
            if !how.starts_with("waiting") && how != "connecting" {
                return how;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        mcp.status()[0].1.clone()
    }

    fn oauth_server(url: String) -> BTreeMap<String, McpServer> {
        BTreeMap::from([(
            "docs".to_owned(),
            McpServer {
                url: Some(url),
                oauth: true,
                ..McpServer::default()
            },
        )])
    }

    #[tokio::test]
    async fn a_server_that_takes_oauth_only_gets_a_grant_kept_renewed_and_asked_again() {
        let dir = tempfile::tempdir().unwrap();
        let grants = dir.path().join("mcp-auth");

        // No grant yet: the person consents once, and the grant is kept.
        let base = serve_oauth(&["t-1", "t-2"]).await;
        let before = CONSENTS.load(Ordering::SeqCst);
        let mcp = Mcp::start_with(
            &oauth_server(format!("{base}/mcp")),
            Some(grants.clone()),
            consent_at_once,
        );
        mcp.settled(Duration::from_secs(10)).await;
        assert_eq!(ready(&mcp).await, "1 tool, 0 only reading");
        assert_eq!(CONSENTS.load(Ordering::SeqCst), before + 1);
        // Said once, for the person to open the page if no browser did.
        let notices = mcp.notices();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].starts_with("The MCP server docs waits for your consent"));
        assert!(mcp.notices().is_empty());
        assert_eq!(mcp.call("docs__fetch", "{}").await, Ok("a page".to_owned()));
        let kept = oauth::Grants::new(grants.clone()).load("docs").unwrap();
        assert_eq!(kept.access_token, "t-1");

        // A grant about to end is renewed, without asking.
        let ending = oauth::Grant::for_test(&base, "t-old", "r-old", oauth::now() + 10);
        oauth::Grants::new(grants.clone())
            .save("docs", &ending)
            .unwrap();
        let before = CONSENTS.load(Ordering::SeqCst);
        let mcp = Mcp::start_with(
            &oauth_server(format!("{base}/mcp")),
            Some(grants.clone()),
            consent_at_once,
        );
        assert_eq!(ready(&mcp).await, "1 tool, 0 only reading");
        assert_eq!(CONSENTS.load(Ordering::SeqCst), before);
        let kept = oauth::Grants::new(grants.clone()).load("docs").unwrap();
        assert_eq!(kept.access_token, "t-2");

        // A grant the server no longer takes is forgotten, and the person
        // asked once more.
        let refused = oauth::Grant::for_test(&base, "t-revoked", "r-x", oauth::now() + 3600);
        oauth::Grants::new(grants.clone())
            .save("docs", &refused)
            .unwrap();
        let before = CONSENTS.load(Ordering::SeqCst);
        let mcp = Mcp::start_with(
            &oauth_server(format!("{base}/mcp")),
            Some(grants.clone()),
            consent_at_once,
        );
        assert_eq!(ready(&mcp).await, "1 tool, 0 only reading");
        assert_eq!(CONSENTS.load(Ordering::SeqCst), before + 1);
        let kept = oauth::Grants::new(grants).load("docs").unwrap();
        assert_eq!(kept.access_token, "t-1");
    }

    #[tokio::test]
    async fn a_server_that_cannot_start_says_why() {
        let declared = BTreeMap::from([(
            "broken".to_owned(),
            McpServer {
                command: vec!["/no/such/program".into()],
                ..McpServer::default()
            },
        )]);
        let mcp = Mcp::start(&declared, None);
        mcp.settled(Duration::from_secs(5)).await;
        let status = mcp.status();
        assert!(
            status[0]
                .1
                .starts_with("failed: /no/such/program could not be started")
        );
        assert!(mcp.specs().is_empty());
    }

    #[test]
    fn events_are_read_from_their_data_lines() {
        let body = "event: message\ndata: {\"a\":1}\n\ndata: line one\ndata: line two\n\n";
        assert_eq!(sse_data(body), ["{\"a\":1}", "line one\nline two"]);
        assert_eq!(plain("my.server"), "my_server");
    }
}
