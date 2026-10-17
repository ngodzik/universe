//! OAuth for servers reached over HTTP that take nothing else, as the
//! protocol's authorization spec describes: the authorization server found
//! from the resource's metadata, a client registered for universe, the
//! person's consent in the browser with PKCE, and the grant kept on disk
//! and renewed before it expires.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_lc_rs::rand::SecureRandom;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long the person has to consent in the browser.
const CONSENT_WITHIN: Duration = Duration::from_secs(300);

/// How long before it expires a grant is renewed.
const RENEW_BEFORE: u64 = 60;

/// Where the authorization server takes the person and the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoints {
    authorize: String,
    token: String,
    register: String,
    /// The protected resource, as the token is asked for it (RFC 8707).
    resource: String,
}

/// What a request carries to the server, and what renews it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Grant {
    client_id: String,
    token_endpoint: String,
    resource: String,
    pub(crate) access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    /// Seconds since 1970; `None` when the server did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
}

/// Written by hand: the tokens never show.
impl fmt::Debug for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Grant")
            .field("client_id", &self.client_id)
            .field("token_endpoint", &self.token_endpoint)
            .field("resource", &self.resource)
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl Grant {
    /// A grant of the tests' server at `base`, ending at `expires_at`.
    #[cfg(test)]
    pub(crate) fn for_test(base: &str, access: &str, refresh: &str, expires_at: u64) -> Self {
        Self {
            client_id: "c-1".into(),
            token_endpoint: format!("{base}/token"),
            resource: format!("{base}/mcp"),
            access_token: access.into(),
            refresh_token: Some(refresh.into()),
            expires_at: Some(expires_at),
        }
    }

    /// Whether it should be renewed before it is used, at `now`.
    pub(crate) fn is_ending(&self, now: u64) -> bool {
        self.expires_at
            .is_some_and(|at| at <= now.saturating_add(RENEW_BEFORE))
    }
}

/// Seconds since 1970.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The grants kept, one file per server, readable by their owner only.
#[derive(Debug, Clone)]
pub(crate) struct Grants {
    dir: PathBuf,
}

impl Grants {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, server: &str) -> PathBuf {
        self.dir.join(format!("{server}.json"))
    }

    pub(crate) fn load(&self, server: &str) -> Option<Grant> {
        let text = std::fs::read_to_string(self.path(server)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub(crate) fn save(&self, server: &str, grant: &Grant) -> Result<(), String> {
        let text = serde_json::to_string_pretty(grant).map_err(|e| e.to_string())?;
        write_private(&self.path(server), &text)
            .map_err(|e| format!("its grant could not be kept: {e}"))
    }

    pub(crate) fn forget(&self, server: &str) {
        let _ = std::fs::remove_file(self.path(server));
    }
}

/// Writes `text` at `path`, readable and writable by its owner only, from
/// the moment it exists.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("part");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    std::io::Write::write_all(&mut options.open(&partial)?, text.as_bytes())?;
    std::fs::rename(&partial, path)
}

/// Finds the authorization server of the resource at `url`, and refuses one
/// that universe cannot use.
pub(crate) async fn discover(client: &reqwest::Client, url: &str) -> Result<Endpoints, String> {
    let resource = reqwest::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
    let mut metadata = None;
    for candidate in well_known(&resource, "oauth-protected-resource") {
        if let Some(found) = get_json(client, &candidate).await {
            metadata = Some(found);
            break;
        }
    }
    let metadata = metadata.ok_or("it says nothing of its authorization server")?;
    let issuer = metadata["authorization_servers"][0]
        .as_str()
        .ok_or("it names no authorization server")?;
    let issuer = reqwest::Url::parse(issuer).map_err(|e| format!("{issuer}: {e}"))?;
    let mut server = None;
    for candidate in well_known(&issuer, "oauth-authorization-server")
        .into_iter()
        .chain(well_known(&issuer, "openid-configuration"))
    {
        if let Some(found) = get_json(client, &candidate).await {
            server = Some(found);
            break;
        }
    }
    let server = server.ok_or("its authorization server says nothing of itself")?;
    let s256 = server["code_challenge_methods_supported"]
        .as_array()
        .is_some_and(|m| m.iter().any(|m| m == "S256"));
    if !s256 {
        return Err("its authorization server does not take PKCE with S256".to_owned());
    }
    let field = |name: &str| server[name].as_str().map(str::to_owned);
    Ok(Endpoints {
        authorize: field("authorization_endpoint").ok_or("no authorization endpoint")?,
        token: field("token_endpoint").ok_or("no token endpoint")?,
        register: field("registration_endpoint").ok_or(
            "its authorization server registers no client: only clients it knows can use it",
        )?,
        resource: metadata["resource"]
            .as_str()
            .map_or_else(|| url.to_owned(), str::to_owned),
    })
}

/// Where `url`'s metadata of `kind` may be: with its path, then at its
/// origin (RFC 8414 and RFC 9728).
fn well_known(url: &reqwest::Url, kind: &str) -> Vec<String> {
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_end_matches('/');
    let mut out = Vec::new();
    if !path.is_empty() {
        out.push(format!("{origin}/.well-known/{kind}{path}"));
    }
    out.push(format!("{origin}/.well-known/{kind}"));
    out
}

async fn get_json(client: &reqwest::Client, url: &str) -> Option<Value> {
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

/// Registers universe with the authorization server, a public client that
/// keeps no secret; returns its id.
async fn register(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    redirect: &str,
) -> Result<String, String> {
    let response = client
        .post(&endpoints.register)
        .json(&json!({
            "client_name": "universe",
            "redirect_uris": [redirect],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .map_err(|e| format!("registering: {e}"))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("registering was refused: HTTP {}", status.as_u16()));
    }
    body["client_id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "registering gave no client id".to_owned())
}

/// `bytes` random bytes as URL-safe text.
fn random_text(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0u8; bytes];
    aws_lc_rs::rand::SystemRandom::new()
        .fill(&mut buffer)
        .map_err(|_| "no random bytes".to_owned())?;
    Ok(URL_SAFE_NO_PAD.encode(buffer))
}

/// The PKCE challenge of `verifier`, as the tests' server checks it.
#[cfg(test)]
pub(crate) fn challenge_of(verifier: &str) -> String {
    challenge(verifier)
}

/// The PKCE challenge of `verifier`, by S256.
fn challenge(verifier: &str) -> String {
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest.as_ref())
}

/// The page the person consents on.
fn authorize_url(
    endpoints: &Endpoints,
    client_id: &str,
    redirect: &str,
    challenge: &str,
    state: &str,
) -> Result<String, String> {
    let mut url = reqwest::Url::parse(&endpoints.authorize)
        .map_err(|e| format!("{}: {e}", endpoints.authorize))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("resource", &endpoints.resource);
    Ok(url.into())
}

/// The person's consent for the resource at `url`: `shown` is told the page
/// to open, and `open` opens it, the browser by default. Returns the grant.
pub(crate) async fn consent(
    client: &reqwest::Client,
    url: &str,
    shown: impl Fn(&str),
    open: impl Fn(&str) -> bool,
) -> Result<Grant, String> {
    let endpoints = discover(client, url).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("no port to hear the consent on: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let client_id = register(client, &endpoints, &redirect).await?;
    let verifier = random_text(32)?;
    let state = random_text(16)?;
    let page = authorize_url(
        &endpoints,
        &client_id,
        &redirect,
        &challenge(&verifier),
        &state,
    )?;
    shown(&page);
    // Without a browser, the page shown is for the person to open.
    let _ = open(&page);
    let code = tokio::time::timeout(CONSENT_WITHIN, callback(&listener, &state))
        .await
        .map_err(|_| "no consent within five minutes".to_owned())??;
    let response = client
        .post(&endpoints.token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
            ("resource", endpoints.resource.as_str()),
        ]))
        .send()
        .await
        .map_err(|e| format!("asking for a token: {e}"))?;
    granted(
        response,
        client_id,
        endpoints.token,
        endpoints.resource,
        None,
    )
    .await
}

/// `grant` renewed with its refresh token.
pub(crate) async fn renew(client: &reqwest::Client, grant: &Grant) -> Result<Grant, String> {
    let refresh = grant
        .refresh_token
        .as_deref()
        .ok_or("the grant cannot be renewed")?;
    let response = client
        .post(&grant.token_endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", grant.client_id.as_str()),
            ("resource", grant.resource.as_str()),
        ]))
        .send()
        .await
        .map_err(|e| format!("renewing the grant: {e}"))?;
    granted(
        response,
        grant.client_id.clone(),
        grant.token_endpoint.clone(),
        grant.resource.clone(),
        grant.refresh_token.clone(),
    )
    .await
}

/// `pairs` as the body of a form, `application/x-www-form-urlencoded`.
fn form(pairs: &[(&str, &str)]) -> String {
    let encode = |text: &str| -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    char::from(b).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The grant a token endpoint answered; a renewal that gives no new refresh
/// token keeps `refresh`.
async fn granted(
    response: reqwest::Response,
    client_id: String,
    token_endpoint: String,
    resource: String,
    refresh: Option<String>,
) -> Result<Grant, String> {
    let status = response.status();
    let body: Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        let why = body["error"].as_str().unwrap_or("refused");
        return Err(format!("no token: HTTP {} ({why})", status.as_u16()));
    }
    let access_token = body["access_token"]
        .as_str()
        .ok_or("the answer holds no token")?
        .to_owned();
    Ok(Grant {
        client_id,
        token_endpoint,
        resource,
        access_token,
        refresh_token: body["refresh_token"]
            .as_str()
            .map(str::to_owned)
            .or(refresh),
        expires_at: body["expires_in"].as_u64().map(|s| now() + s),
    })
}

/// Waits for the browser to come back with the code, and answers it.
async fn callback(listener: &tokio::net::TcpListener, state: &str) -> Result<String, String> {
    loop {
        let (mut socket, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let request = read_head(&mut socket).await;
        let Some(target) = request
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("GET "))
            .and_then(|rest| rest.split(' ').next())
        else {
            continue;
        };
        // A browser asks for its icon too: only the callback counts.
        let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
            continue;
        };
        if url.path() != "/callback" {
            let _ = socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            continue;
        }
        let param = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        let outcome = match (param("code"), param("state"), param("error")) {
            (_, _, Some(error)) => Err(format!("consent refused: {error}")),
            (Some(code), Some(given), None) if given == state => Ok(code),
            (Some(_), _, None) => Err("the consent came back for another request".to_owned()),
            _ => Err("the consent came back without a code".to_owned()),
        };
        let page = match &outcome {
            Ok(_) => "universe has your consent: this page can be closed.".to_owned(),
            Err(why) => format!("universe did not get your consent: {why}."),
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        return outcome;
    }
}

/// What a browser sent up to the end of its headers, however TCP cut it,
/// at most 16 KiB.
async fn read_head(socket: &mut tokio::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 2048];
    while buffer.len() < 16 * 1024 && !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

/// Opens `url` in the person's browser: `open` on macOS, `xdg-open`
/// elsewhere. Whether it could be started.
pub(crate) fn open_browser(url: &str) -> bool {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let child = std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match child {
        Ok(mut child) => {
            // Waited for on a thread of its own, so that it leaves no
            // zombie behind.
            std::thread::spawn(move || child.wait());
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_s256_of_the_verifier() {
        // RFC 7636's example, its challenge worked out apart with Python's
        // hashlib and base64.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        // 32 random bytes make the 43 characters the RFC asks for.
        let verifier = random_text(32).unwrap();
        assert_eq!(verifier.len(), 43);
        assert_ne!(verifier, random_text(32).unwrap());
    }

    #[test]
    fn a_form_is_encoded_as_browsers_encode_it() {
        assert_eq!(
            form(&[
                ("code", "a b/c+d"),
                ("redirect_uri", "http://127.0.0.1:9/cb")
            ]),
            "code=a%20b%2Fc%2Bd&redirect_uri=http%3A%2F%2F127.0.0.1%3A9%2Fcb"
        );
    }

    #[test]
    fn metadata_is_looked_for_with_the_path_then_at_the_origin() {
        let url = reqwest::Url::parse("https://mcp.example.com/mcp/").unwrap();
        assert_eq!(
            well_known(&url, "oauth-protected-resource"),
            [
                "https://mcp.example.com/.well-known/oauth-protected-resource/mcp",
                "https://mcp.example.com/.well-known/oauth-protected-resource",
            ]
        );
        let issuer = reqwest::Url::parse("https://auth.example.com").unwrap();
        assert_eq!(
            well_known(&issuer, "oauth-authorization-server"),
            ["https://auth.example.com/.well-known/oauth-authorization-server"]
        );
    }

    #[tokio::test]
    async fn a_callback_cut_in_two_by_the_network_still_gives_its_code() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            socket.write_all(b"GET /callback?code=c-9&").await.unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
            socket
                .write_all(b"state=s-1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .await
                .unwrap();
            let mut answer = String::new();
            let _ = socket.read_to_string(&mut answer).await;
            assert!(answer.contains("universe has your consent"));
        });
        assert_eq!(callback(&listener, "s-1").await, Ok("c-9".to_owned()));
    }

    #[test]
    fn a_grant_is_kept_for_its_owner_only_and_never_shows_its_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let grants = Grants::new(dir.path().join("mcp-auth"));
        let grant = Grant {
            client_id: "c-1".into(),
            token_endpoint: "https://auth.example.com/token".into(),
            resource: "https://mcp.example.com/mcp".into(),
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
            expires_at: Some(1_000),
        };
        grants.save("docs", &grant).unwrap();
        assert_eq!(grants.load("docs"), Some(grant.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.path().join("mcp-auth/docs.json");
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let shown = format!("{grant:?}");
        assert!(!shown.contains("secret-access") && !shown.contains("secret-refresh"));
        // Renewed a minute before it ends.
        assert!(!grant.is_ending(900));
        assert!(grant.is_ending(941));
        grants.forget("docs");
        assert_eq!(grants.load("docs"), None);
    }
}
