//! MCP client: servers from `~/.yopes/mcp.json` (global) and `.mcp.json` in
//! the launch directory (project). A server speaks stdio, streamable HTTP or
//! SSE; its tools join the tool list as `mcp__<server>__<tool>` and ask for
//! approval on every call. A server that declares resources also gets
//! `mcp__<server>__list_resources` and `mcp__<server>__read_resource`.
//!
//! Servers start in a background thread at launch (`start`), so a slow server
//! never stalls the first frame: the landing page reads `report()` while they
//! connect and shows each one's state. A call that fails on a dead process or
//! a dropped connection reconnects once and retries.
//!
//! ponytail: servers stay up for the process and prompts are not exposed; add
//! them when a server needs them.

use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};

const PREFIX: &str = "mcp__";
/// What the client asks for. A server that knows an older version answers with
/// that one instead, and its answer is what later requests announce.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// Connecting and listing tools has to be quick: a hung server must not hold
/// the tool list hostage.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// Seconds a `tools/call` may run before it is reported as a timeout.
const DEFAULT_CALL_TIMEOUT: u64 = 60;
/// Provider function names stop at 64 characters.
const MAX_TOOL_NAME: usize = 64;
/// How many `nextCursor` pages one listing may follow.
const MAX_PAGES: usize = 50;

// ---------------------------------------------------------------- config

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scope {
    Global,
    Project,
}

impl Scope {
    fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Stdio,
    Http,
    Sse,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }
}

#[derive(Clone, Default, Deserialize)]
struct ServerSpec {
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    cwd: Option<String>,
    url: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    /// `transport` is the other spelling found in the wild.
    transport: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// Seconds a `tools/call` may run.
    timeout: Option<u64>,
    #[serde(default)]
    disabled: bool,
}

impl ServerSpec {
    fn kind(&self, name: &str) -> Result<Kind> {
        let declared = self
            .kind
            .as_deref()
            .or(self.transport.as_deref())
            .map(|kind| kind.trim().to_ascii_lowercase());
        match declared.as_deref() {
            None | Some("") => Ok(if self.url.is_some() {
                Kind::Http
            } else {
                Kind::Stdio
            }),
            Some("stdio") => Ok(Kind::Stdio),
            Some("http" | "streamable-http" | "streamable_http") => Ok(Kind::Http),
            Some("sse" | "http+sse") => Ok(Kind::Sse),
            Some(other) => {
                bail!("MCP server {name}: unknown transport `{other}` (use stdio, http or sse)")
            }
        }
    }
}

#[derive(Default, Deserialize)]
struct FileConfig {
    #[serde(default, rename = "mcpServers")]
    servers: BTreeMap<String, ServerSpec>,
}

/// One server ready to start: the spec with every `${VAR}` expanded and its
/// paths resolved.
#[derive(Clone, Debug)]
struct Prepared {
    name: String,
    scope: Scope,
    kind: Kind,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    url: String,
    headers: BTreeMap<String, String>,
    timeout: Duration,
    disabled: bool,
}

/// The config files in the order they are read: the global file first, so a
/// project server of the same name replaces it.
fn config_paths() -> Vec<(PathBuf, Scope)> {
    let mut paths = vec![(crate::config::yopes_home().join("mcp.json"), Scope::Global)];
    // The launch directory, not a tab's: one set of servers serves the process.
    if let Ok(directory) = env::current_dir() {
        paths.push((directory.join(".mcp.json"), Scope::Project));
    }
    paths
}

fn read_config(path: &Path) -> Result<Option<FileConfig>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| path.display().to_string()),
    };
    let file = match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(error) => return Err(error).with_context(|| path.display().to_string()),
    };
    Ok(Some(file))
}

/// `${VAR}` reads an environment variable, `${VAR:-fallback}` a default. A
/// name with neither is reported, so a typo fails at startup instead of
/// sending an empty token.
fn expand(text: &str, missing: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let (key, fallback) = match after[..end].split_once(":-") {
            Some((key, fallback)) => (key, Some(fallback)),
            None => (&after[..end], None),
        };
        match env::var(key) {
            Ok(value) => out.push_str(&value),
            Err(_) => match fallback {
                Some(fallback) => out.push_str(fallback),
                None => missing.push(key.to_owned()),
            },
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn prepare(name: &str, spec: &ServerSpec, base: &Path, scope: Scope) -> Result<Prepared> {
    let kind = spec.kind(name)?;
    let mut missing = Vec::new();
    let command = expand(spec.command.as_deref().unwrap_or_default(), &mut missing);
    let url = expand(spec.url.as_deref().unwrap_or_default(), &mut missing);
    let args = spec
        .args
        .iter()
        .map(|arg| expand(arg, &mut missing))
        .collect();
    let env = spec
        .env
        .iter()
        .map(|(key, value)| (key.clone(), expand(value, &mut missing)))
        .collect();
    let headers = spec
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), expand(value, &mut missing)))
        .collect();
    let cwd = spec.cwd.as_deref().map(|dir| {
        let dir = expand(dir, &mut missing);
        let path = Path::new(&dir);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
    });
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        bail!(
            "MCP server {name}: unset environment variable {}",
            missing.join(", ")
        );
    }
    match kind {
        Kind::Stdio if command.trim().is_empty() => {
            bail!("MCP server {name}: `command` is required for stdio")
        }
        Kind::Http | Kind::Sse if url.trim().is_empty() => {
            bail!(
                "MCP server {name}: `url` is required for the {} transport",
                kind.label()
            )
        }
        _ => {}
    }
    Ok(Prepared {
        name: name.to_owned(),
        scope,
        kind,
        command,
        args,
        env,
        cwd,
        url,
        headers,
        timeout: Duration::from_secs(spec.timeout.unwrap_or(DEFAULT_CALL_TIMEOUT).max(1)),
        disabled: spec.disabled,
    })
}

// ------------------------------------------------------------ transports

/// A JSON-RPC pipe to one server. `send` starts a request, `recv` hands back
/// whatever arrived next; matching a reply to its request is `Client`'s job.
trait Transport: Send {
    fn kind(&self) -> &'static str;
    fn send(&mut self, message: &Value, timeout: Duration) -> Result<()>;
    /// `None` means nothing arrived within the wait, not that the pipe ended.
    fn recv(&mut self, wait: Duration) -> Result<Option<Value>>;
    fn set_protocol(&mut self, _protocol: &str) {}
}

fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("creating the MCP HTTP client")
}

// ---------------------------------------------------------------- stdio

/// Lines of stderr kept for the error message when a server dies.
const STDERR_TAIL: usize = 8;

struct StdioServer {
    name: String,
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    errors: Arc<Mutex<VecDeque<String>>>,
}

impl StdioServer {
    fn start(prepared: &Prepared) -> Result<Self> {
        let name = prepared.name.clone();
        let mut command = Command::new(&prepared.command);
        command
            .args(&prepared.args)
            .envs(&prepared.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &prepared.cwd {
            command.current_dir(cwd);
        }
        // A console window flashing on every start is noise: CREATE_NO_WINDOW.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("starting MCP server {name} ({})", prepared.command))?;
        let stdin = child.stdin.take().context("MCP server has no stdin")?;
        let stdout = child.stdout.take().context("MCP server has no stdout")?;
        let stderr = child.stderr.take();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        // The last stderr lines explain a server that died on startup; they are
        // drained so a chatty server never fills the pipe and blocks.
        let errors = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = stderr {
            let errors = Arc::clone(&errors);
            std::thread::spawn(move || {
                for line in BufReader::new(stderr)
                    .lines()
                    .map_while(std::result::Result::ok)
                {
                    if let Ok(mut errors) = errors.lock() {
                        if errors.len() >= STDERR_TAIL {
                            errors.pop_front();
                        }
                        errors.push_back(line);
                    }
                }
            });
        }
        Ok(Self {
            name,
            child,
            stdin,
            lines,
            errors,
        })
    }

    fn stderr_tail(&self) -> String {
        let Ok(errors) = self.errors.lock() else {
            return String::new();
        };
        if errors.is_empty() {
            return String::new();
        }
        let tail = errors.iter().cloned().collect::<Vec<_>>().join(" | ");
        format!(": {tail}")
    }
}

impl Transport for StdioServer {
    fn kind(&self) -> &'static str {
        "stdio"
    }

    fn send(&mut self, message: &Value, _timeout: Duration) -> Result<()> {
        writeln!(self.stdin, "{message}")
            .and_then(|()| self.stdin.flush())
            .with_context(|| format!("writing to MCP server {}", self.name))
    }

    fn recv(&mut self, wait: Duration) -> Result<Option<Value>> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                // A server that prints a banner must not break the session:
                // only JSON lines are messages.
                Ok(line) => {
                    if let Ok(message) = serde_json::from_str::<Value>(&line) {
                        return Ok(Some(message));
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let tail = self.stderr_tail();
                    bail!("MCP server {} exited{tail}", self.name);
                }
            }
        }
    }
}

impl Drop for StdioServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ------------------------------------------------------------------- SSE

/// One server-sent event: its name (`message` when the stream omits one) and
/// the `data:` lines joined by newlines. `None` means the stream ended.
fn read_event(reader: &mut impl BufRead) -> io::Result<Option<(String, String)>> {
    let mut event = String::from("message");
    let mut data: Vec<String> = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            // A final event without its blank line still counts.
            return Ok((!data.is_empty()).then(|| (event, data.join("\n"))));
        }
        let text = line.trim_end_matches(['\n', '\r']);
        if text.is_empty() {
            if data.is_empty() {
                event = String::from("message");
                continue;
            }
            return Ok(Some((event, data.join("\n"))));
        }
        if let Some(rest) = text.strip_prefix("event:") {
            event = rest.trim().to_owned();
        } else if let Some(rest) = text.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
        }
        // `id:`, `retry:` and `:` comments carry nothing this client needs.
    }
}

/// The endpoint an SSE stream announces is usually a path on the same origin.
fn resolve_url(base: &str, endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.to_owned();
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        return format!("{base}{endpoint}");
    };
    let origin = rest.split('/').next().unwrap_or(rest);
    if endpoint.starts_with('/') {
        format!("{scheme}://{origin}{endpoint}")
    } else {
        format!("{scheme}://{origin}/{endpoint}")
    }
}

// ------------------------------------------------------- streamable HTTP

/// The current MCP HTTP transport: one POST per message, the answer either a
/// JSON body or an event stream that ends at the reply.
struct Streamable {
    name: String,
    url: String,
    headers: BTreeMap<String, String>,
    client: reqwest::blocking::Client,
    session: Option<String>,
    protocol: Option<String>,
    inbox: VecDeque<Value>,
}

impl Streamable {
    fn start(prepared: &Prepared) -> Result<Self> {
        Ok(Self {
            name: prepared.name.clone(),
            url: prepared.url.clone(),
            headers: prepared.headers.clone(),
            client: http_client()?,
            session: None,
            protocol: None,
            inbox: VecDeque::new(),
        })
    }

    fn request(&self, message: &Value, timeout: Duration) -> reqwest::blocking::RequestBuilder {
        let mut request = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .timeout(timeout)
            .body(message.to_string());
        for (key, value) in &self.headers {
            request = request.header(key.as_str(), value.as_str());
        }
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session.as_str());
        }
        if let Some(protocol) = &self.protocol {
            request = request.header("mcp-protocol-version", protocol.as_str());
        }
        request
    }
}

impl Transport for Streamable {
    fn kind(&self) -> &'static str {
        "http"
    }

    fn set_protocol(&mut self, protocol: &str) {
        self.protocol = Some(protocol.to_owned());
    }

    fn send(&mut self, message: &Value, timeout: Duration) -> Result<()> {
        let response = self
            .request(message, timeout)
            .send()
            .with_context(|| format!("contacting MCP server {}", self.name))?;
        let status = response.status();
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            self.session = Some(session.to_owned());
        }
        if !status.is_success() {
            let detail = response.text().unwrap_or_default();
            let detail = detail.trim();
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(": {}", detail.chars().take(200).collect::<String>())
            };
            bail!("MCP server {} answered {status}{detail}", self.name);
        }
        let streaming = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"));
        if streaming {
            // Read up to the reply: the stream may also carry notifications,
            // which are queued in the order they arrived.
            let mut reader = BufReader::new(response);
            while let Some((_, data)) = read_event(&mut reader)
                .with_context(|| format!("reading the MCP server {} stream", self.name))?
            {
                let Ok(value) = serde_json::from_str::<Value>(&data) else {
                    continue;
                };
                let reply = value.get("id").is_some();
                self.inbox.push_back(value);
                if reply {
                    return Ok(());
                }
            }
            return Ok(());
        }
        let body = response
            .text()
            .with_context(|| format!("reading the MCP server {} reply", self.name))?;
        if body.trim().is_empty() {
            // 202 Accepted: a notification has no reply.
            return Ok(());
        }
        let value: Value = serde_json::from_str(&body)
            .with_context(|| format!("MCP server {} sent invalid JSON", self.name))?;
        match value {
            Value::Array(items) => self.inbox.extend(items),
            other => self.inbox.push_back(other),
        }
        Ok(())
    }

    fn recv(&mut self, _wait: Duration) -> Result<Option<Value>> {
        // Every reply is read by `send`, so this never blocks.
        Ok(self.inbox.pop_front())
    }
}

// ---------------------------------------------------------- legacy SSE

/// The 2024-11-05 HTTP transport: a GET stream announces the POST endpoint and
/// carries every reply; requests are posted to that endpoint.
struct Sse {
    name: String,
    endpoint: String,
    headers: BTreeMap<String, String>,
    client: reqwest::blocking::Client,
    inbound: mpsc::Receiver<Value>,
    inbox: VecDeque<Value>,
}

impl Sse {
    fn start(prepared: &Prepared) -> Result<Self> {
        let name = prepared.name.clone();
        let client = http_client()?;
        let base = prepared.url.clone();
        let headers = prepared.headers.clone();
        let (endpoint_sender, endpoint_receiver) =
            mpsc::channel::<std::result::Result<String, String>>();
        let (message_sender, inbound) = mpsc::channel::<Value>();
        let thread_client = client.clone();
        let thread_base = base.clone();
        std::thread::spawn(move || {
            let outcome = (|| -> Result<()> {
                let mut request = thread_client
                    .get(&thread_base)
                    .header("accept", "text/event-stream");
                for (key, value) in &headers {
                    request = request.header(key.as_str(), value.as_str());
                }
                let response = request.send().context("opening the SSE stream")?;
                if !response.status().is_success() {
                    bail!("the server answered {}", response.status());
                }
                let mut reader = BufReader::new(response);
                while let Some((event, data)) =
                    read_event(&mut reader).context("reading the SSE stream")?
                {
                    if event == "endpoint" {
                        let url = resolve_url(&thread_base, data.trim());
                        let _ = endpoint_sender.send(Ok(url));
                        continue;
                    }
                    if let Ok(message) = serde_json::from_str::<Value>(&data)
                        && message_sender.send(message).is_err()
                    {
                        return Ok(());
                    }
                }
                Ok(())
            })();
            if let Err(error) = outcome {
                let _ = endpoint_sender.send(Err(format!("{error:#}")));
            }
        });
        let endpoint = match endpoint_receiver.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(endpoint)) => endpoint,
            Ok(Err(error)) => bail!("MCP server {name}: {error}"),
            Err(_) => bail!(
                "MCP server {name} announced no SSE endpoint within {}s",
                STARTUP_TIMEOUT.as_secs()
            ),
        };
        Ok(Self {
            name,
            endpoint,
            headers: prepared.headers.clone(),
            client,
            inbound,
            inbox: VecDeque::new(),
        })
    }
}

impl Transport for Sse {
    fn kind(&self) -> &'static str {
        "sse"
    }

    fn send(&mut self, message: &Value, timeout: Duration) -> Result<()> {
        let mut request = self
            .client
            .post(&self.endpoint)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .timeout(timeout)
            .body(message.to_string());
        for (key, value) in &self.headers {
            request = request.header(key.as_str(), value.as_str());
        }
        let response = request
            .send()
            .with_context(|| format!("contacting MCP server {}", self.name))?;
        let status = response.status();
        if !status.is_success() {
            bail!("MCP server {} answered {status}", self.name);
        }
        // A server may answer on the POST instead of the stream.
        let json = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("json"));
        if json {
            let body = response.text().unwrap_or_default();
            if let Ok(value) = serde_json::from_str::<Value>(&body) {
                self.inbox.push_back(value);
            }
        }
        Ok(())
    }

    fn recv(&mut self, wait: Duration) -> Result<Option<Value>> {
        if let Some(value) = self.inbox.pop_front() {
            return Ok(Some(value));
        }
        match self.inbound.recv_timeout(wait) {
            Ok(value) => Ok(Some(value)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("MCP server {} closed its SSE stream", self.name)
            }
        }
    }
}

// ---------------------------------------------------------------- client

/// A server that answered with a failure instead of a result. A retry would
/// fail the same way, so `call` reports this one to the model as-is.
#[derive(Debug)]
struct ReplyError(String);

impl std::fmt::Display for ReplyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ReplyError {}

fn reply_error(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(ReplyError(message.into()))
}

fn describe_error(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("no message");
    match error.get("code").and_then(Value::as_i64) {
        Some(code) => format!("{message} (code {code})"),
        None => message.to_owned(),
    }
}

fn mime(item: &Value) -> &str {
    item.get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or("unknown type")
}

fn resource_text(resource: &Value) -> String {
    if let Some(text) = resource.get("text").and_then(Value::as_str) {
        return text.to_owned();
    }
    let uri = resource
        .get("uri")
        .and_then(Value::as_str)
        .unwrap_or("resource");
    format!("[binary resource {uri}]")
}

/// A `tools/call` result as text: text parts joined, binary parts named.
fn render_content(result: &Value) -> String {
    let mut out = Vec::new();
    for item in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "text" => out.push(
                item.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            ),
            "image" => out.push(format!("[image {} omitted]", mime(item))),
            "audio" => out.push(format!("[audio {} omitted]", mime(item))),
            "resource" => out.push(
                item.get("resource")
                    .map(resource_text)
                    .unwrap_or_else(|| "[resource omitted]".to_owned()),
            ),
            other => out.push(format!("[{other} result]")),
        }
    }
    if out.is_empty() {
        // Some servers put the payload here instead of in `content`.
        if let Some(structured) = result.get("structuredContent") {
            out.push(structured.to_string());
        }
    }
    out.join("\n")
}

fn render_resources(result: &Value) -> String {
    let mut out = Vec::new();
    for resource in result
        .get("resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let uri = resource
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = resource
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let description = resource
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut line = uri.to_owned();
        if !name.is_empty() {
            line.push_str(" — ");
            line.push_str(name);
        }
        if !description.is_empty() {
            line.push_str(" — ");
            line.push_str(description);
        }
        out.push(line);
    }
    if out.is_empty() {
        "(no resources)".to_owned()
    } else {
        out.join("\n")
    }
}

fn render_contents(result: &Value) -> String {
    let mut out = Vec::new();
    for content in result
        .get("contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(text) = content.get("text").and_then(Value::as_str) {
            out.push(text.to_owned());
        } else if let Some(uri) = content.get("uri").and_then(Value::as_str) {
            out.push(format!("[binary resource {uri}]"));
        }
    }
    if out.is_empty() {
        "(empty resource)".to_owned()
    } else {
        out.join("\n")
    }
}

/// What one tool call reaches: a remote tool, or one of the two resource
/// bridges built for a server that declares resources.
#[derive(Clone, Debug)]
enum Target {
    Tool(String),
    ListResources,
    ReadResource,
}

struct Client {
    name: String,
    transport: Box<dyn Transport>,
    protocol: String,
    capabilities: Value,
    timeout: Duration,
    next_id: u64,
    /// Set by `notifications/tools/list_changed`: the tool list is stale.
    tools_changed: bool,
}

impl Client {
    fn connect(prepared: &Prepared) -> Result<Self> {
        let transport: Box<dyn Transport> = match prepared.kind {
            Kind::Stdio => Box::new(StdioServer::start(prepared)?),
            Kind::Http => Box::new(Streamable::start(prepared)?),
            Kind::Sse => Box::new(Sse::start(prepared)?),
        };
        let mut client = Self {
            name: prepared.name.clone(),
            transport,
            protocol: PROTOCOL_VERSION.to_owned(),
            capabilities: Value::Null,
            timeout: prepared.timeout,
            next_id: 0,
            tools_changed: false,
        };
        let result = client.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "yopes", "version": env!("CARGO_PKG_VERSION")},
            }),
            STARTUP_TIMEOUT,
        )?;
        if let Some(protocol) = result.get("protocolVersion").and_then(Value::as_str) {
            client.protocol = protocol.to_owned();
            client.transport.set_protocol(protocol);
        }
        client.capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
        client.notify("notifications/initialized", json!({}))?;
        Ok(client)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.transport.send(
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
            STARTUP_TIMEOUT,
        )
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.transport.send(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
            timeout,
        )?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!(
                    "MCP server {} did not answer {method} within {}s",
                    self.name,
                    timeout.as_secs()
                );
            }
            let Some(message) = self.transport.recv(left)? else {
                // A streamed transport waits inside `recv`; the HTTP one
                // returned from `send` already and has nothing left to wait for.
                if self.transport.kind() == "http" {
                    bail!("MCP server {} sent no reply to {method}", self.name);
                }
                continue;
            };
            if message.get("method").is_some() {
                self.on_notification(&message);
                continue;
            }
            // Notifications and other requests' replies are not this answer.
            if message.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(reply_error(format!(
                    "MCP {method} failed: {}",
                    describe_error(error)
                )));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn on_notification(&mut self, message: &Value) {
        if message.get("method").and_then(Value::as_str) == Some("notifications/tools/list_changed")
        {
            self.tools_changed = true;
        }
    }

    /// Every page of a listing, following `nextCursor`.
    fn list(&mut self, method: &str, key: &str, timeout: Duration) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let result = self.request(method, params, timeout)?;
            items.extend(
                result
                    .get(key)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(items)
    }

    /// Every tool this server offers, resources included when it declares them.
    fn tools(&mut self, slot: usize) -> Result<Vec<Tool>> {
        let listed = self.list("tools/list", "tools", STARTUP_TIMEOUT)?;
        let mut tools: Vec<Tool> = Vec::new();
        for item in listed {
            let Some(remote) = item.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(name) = tool_name(&self.name, remote) else {
                continue;
            };
            if tools.iter().any(|tool| tool.name == name) {
                continue;
            }
            tools.push(Tool {
                name,
                target: Target::Tool(remote.to_owned()),
                slot,
                description: item
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                schema: if item.get("inputSchema").is_some_and(Value::is_object) {
                    item["inputSchema"].clone()
                } else {
                    json!({"type": "object"})
                },
            });
        }
        if self.capabilities.get("resources").is_some() {
            let server = self.name.clone();
            if let Some(name) = tool_name(&server, "list_resources") {
                tools.push(Tool {
                    name,
                    target: Target::ListResources,
                    slot,
                    description: format!("List the resources MCP server {server} exposes."),
                    schema: json!({"type": "object", "properties": {}}),
                });
            }
            if let Some(name) = tool_name(&server, "read_resource") {
                tools.push(Tool {
                    name,
                    target: Target::ReadResource,
                    slot,
                    description: format!(
                        "Read one resource from MCP server {server}: pass the uri that \
                         list_resources returned."
                    ),
                    schema: json!({
                        "type": "object",
                        "properties": {"uri": {"type": "string", "description": "Resource URI"}},
                        "required": ["uri"]
                    }),
                });
            }
        }
        Ok(tools)
    }

    fn call(&mut self, target: &Target, arguments: &Value) -> Result<String> {
        match target {
            Target::Tool(remote) => {
                let result = self.request(
                    "tools/call",
                    json!({"name": remote, "arguments": arguments}),
                    self.timeout,
                )?;
                let text = render_content(&result);
                if result.get("isError").and_then(Value::as_bool) == Some(true) {
                    return Err(reply_error(if text.is_empty() {
                        format!("MCP tool {remote} failed")
                    } else {
                        text
                    }));
                }
                Ok(if text.is_empty() {
                    result.to_string()
                } else {
                    text
                })
            }
            Target::ListResources => {
                let result = self.request("resources/list", json!({}), self.timeout)?;
                Ok(render_resources(&result))
            }
            Target::ReadResource => {
                let Some(uri) = arguments.get("uri").and_then(Value::as_str) else {
                    return Err(reply_error("read_resource needs a `uri` argument"));
                };
                let result = self.request("resources/read", json!({"uri": uri}), self.timeout)?;
                Ok(render_contents(&result))
            }
        }
    }
}

// -------------------------------------------------------------- registry

#[derive(Clone, Debug)]
pub(crate) enum State {
    Starting,
    Ready { tools: usize },
    Failed(String),
    Disabled,
}

/// One server as the UI shows it.
#[derive(Clone, Debug)]
pub struct ServerStatus {
    pub name: String,
    pub scope: &'static str,
    pub transport: &'static str,
    pub state: State,
}

impl ServerStatus {
    /// What the landing page prints after the server name.
    pub fn detail(&self) -> String {
        state_label(&self.state)
    }

    /// Whether the row is a failure the user has to act on.
    pub fn is_failure(&self) -> bool {
        matches!(self.state, State::Failed(_))
    }
}

/// A read-only snapshot for the landing page and `/mcp`.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub servers: Vec<ServerStatus>,
    pub tools: usize,
    pub loading: bool,
    pub problems: Vec<String>,
}

struct Tool {
    name: String,
    target: Target,
    slot: usize,
    description: String,
    schema: Value,
}

struct Slot {
    prepared: Prepared,
    client: Option<Client>,
}

struct Runtime {
    slots: Mutex<Vec<Arc<Mutex<Slot>>>>,
    statuses: Mutex<Vec<ServerStatus>>,
    tools: Mutex<Vec<Tool>>,
    problems: Mutex<Vec<String>>,
    loading: AtomicBool,
    /// When a server last changed state: the ticker keeps drawing for a moment
    /// after, so the frame that shows the change is never skipped.
    changed: Mutex<Instant>,
}

impl Runtime {
    fn touch(&self) {
        if let Ok(mut changed) = self.changed.lock() {
            *changed = Instant::now();
        }
    }

    fn set_state(&self, index: usize, state: State) {
        if let Ok(mut statuses) = self.statuses.lock() {
            if let Some(status) = statuses.get_mut(index) {
                status.state = state;
            }
        }
        self.touch();
    }

    /// Forget every server: dropping the slots kills their processes.
    fn reset(&self) {
        if let Ok(mut slots) = self.slots.lock() {
            slots.clear();
        }
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.clear();
        }
        if let Ok(mut tools) = self.tools.lock() {
            tools.clear();
        }
        if let Ok(mut problems) = self.problems.lock() {
            problems.clear();
        }
        self.touch();
    }
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| Runtime {
        slots: Mutex::new(Vec::new()),
        statuses: Mutex::new(Vec::new()),
        tools: Mutex::new(Vec::new()),
        problems: Mutex::new(Vec::new()),
        loading: AtomicBool::new(false),
        // Nothing has changed yet: only a load or a state change turns the
        // landing page's ticker on.
        changed: Mutex::new(
            Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
        ),
    })
}

/// Start loading the configured servers, once, off the caller's thread.
pub fn start() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        spawn_load(false);
    });
}

/// Re-read both config files and restart every server. False when a load is
/// already running.
pub fn reload() -> bool {
    spawn_load(true)
}

fn spawn_load(reset: bool) -> bool {
    let runtime = runtime();
    if runtime.loading.swap(true, Ordering::SeqCst) {
        return false;
    }
    std::thread::spawn(move || {
        if reset {
            runtime.reset();
        }
        load(runtime);
        runtime.loading.store(false, Ordering::SeqCst);
        runtime.touch();
    });
    true
}

fn load(runtime: &Runtime) {
    let mut prepared: Vec<Prepared> = Vec::new();
    let mut problems = Vec::new();
    for (path, scope) in config_paths() {
        let file = match read_config(&path) {
            Ok(Some(file)) => file,
            Ok(None) => continue,
            Err(error) => {
                problems.push(format!("{error:#}"));
                continue;
            }
        };
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        for (name, spec) in file.servers {
            match prepare(&name, &spec, base, scope) {
                Ok(server) => match prepared.iter_mut().find(|other| other.name == server.name) {
                    // A project server of the same name replaces the global one.
                    Some(existing) => *existing = server,
                    None => prepared.push(server),
                },
                Err(error) => problems.push(format!("{error:#}")),
            }
        }
    }
    if let Ok(mut stored) = runtime.problems.lock() {
        *stored = problems;
    }
    install(runtime, prepared);
}

/// Publish the servers and connect each one, so the landing page shows every
/// state as it happens.
fn install(runtime: &Runtime, prepared: Vec<Prepared>) {
    let slots = prepared
        .iter()
        .map(|server| {
            Arc::new(Mutex::new(Slot {
                prepared: server.clone(),
                client: None,
            }))
        })
        .collect::<Vec<_>>();
    let statuses = prepared
        .iter()
        .map(|server| ServerStatus {
            name: server.name.clone(),
            scope: server.scope.label(),
            transport: server.kind.label(),
            state: if server.disabled {
                State::Disabled
            } else {
                State::Starting
            },
        })
        .collect::<Vec<_>>();
    if let Ok(mut stored) = runtime.slots.lock() {
        *stored = slots;
    }
    if let Ok(mut stored) = runtime.statuses.lock() {
        *stored = statuses;
    }
    if let Ok(mut stored) = runtime.tools.lock() {
        stored.clear();
    }
    runtime.touch();
    for (index, server) in prepared.iter().enumerate() {
        if server.disabled {
            continue;
        }
        if let Err(error) = connect_slot(runtime, index) {
            runtime.set_state(index, State::Failed(format!("{error:#}")));
        }
    }
}

fn slot_at(runtime: &Runtime, index: usize) -> Result<Arc<Mutex<Slot>>> {
    runtime
        .slots
        .lock()
        .map_err(|_| anyhow!("MCP lock poisoned"))?
        .get(index)
        .cloned()
        .ok_or_else(|| anyhow!("MCP server {index} is not loaded"))
}

fn publish_tools(runtime: &Runtime, index: usize, tools: Vec<Tool>) {
    if let Ok(mut stored) = runtime.tools.lock() {
        stored.retain(|tool| tool.slot != index);
        stored.extend(tools);
    }
    runtime.touch();
}

/// Start one server and take its tool list.
fn connect_slot(runtime: &Runtime, index: usize) -> Result<()> {
    let slot = slot_at(runtime, index)?;
    let prepared = {
        let slot = slot.lock().map_err(|_| anyhow!("MCP lock poisoned"))?;
        slot.prepared.clone()
    };
    runtime.set_state(index, State::Starting);
    let mut client = Client::connect(&prepared)?;
    let tools = client.tools(index)?;
    let count = tools.len();
    {
        let mut slot = slot.lock().map_err(|_| anyhow!("MCP lock poisoned"))?;
        slot.client = Some(client);
    }
    publish_tools(runtime, index, tools);
    runtime.set_state(index, State::Ready { tools: count });
    Ok(())
}

fn invoke(runtime: &Runtime, index: usize, target: &Target, arguments: &Value) -> Result<String> {
    let slot = slot_at(runtime, index)?;
    let mut guard = slot.lock().map_err(|_| anyhow!("MCP lock poisoned"))?;
    let name = guard.prepared.name.clone();
    let Some(client) = guard.client.as_mut() else {
        bail!("MCP server {name} is not connected");
    };
    let text = client.call(target, arguments)?;
    let refresh = std::mem::take(&mut client.tools_changed);
    let listed = if refresh {
        Some(client.tools(index)?)
    } else {
        None
    };
    drop(guard);
    if let Some(tools) = listed {
        let count = tools.len();
        publish_tools(runtime, index, tools);
        runtime.set_state(index, State::Ready { tools: count });
    }
    Ok(text)
}

fn drop_client(runtime: &Runtime, index: usize, error: &anyhow::Error) {
    if let Ok(slot) = slot_at(runtime, index) {
        if let Ok(mut slot) = slot.lock() {
            // Dropping the client kills a stdio child and closes its session.
            slot.client = None;
        }
    }
    runtime.set_state(index, State::Failed(format!("{error:#}")));
}

// ------------------------------------------------------------------- API

/// Provider function names allow only letters, digits, `_` and `-`.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `mcp__server__tool`, or None when the name would overflow the provider's
/// limit: a truncated name could collide with another tool's.
fn tool_name(server: &str, remote: &str) -> Option<String> {
    let name = format!("{PREFIX}{}__{}", sanitize(server), sanitize(remote));
    (name.len() <= MAX_TOOL_NAME).then_some(name)
}

pub fn is_mcp(name: &str) -> bool {
    name.starts_with(PREFIX)
}

/// Tool schemas for every MCP tool, in the provider's function-tool shape.
pub fn definitions() -> Vec<Value> {
    start();
    // Servers still starting add no tools yet; a later request picks them up.
    let Ok(tools) = runtime().tools.lock() else {
        return Vec::new();
    };
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.schema,
                }
            })
        })
        .collect()
}

/// What the landing page and `/mcp` read. Never waits on a running call.
pub fn report() -> Report {
    let runtime = runtime();
    Report {
        servers: runtime
            .statuses
            .lock()
            .map(|statuses| statuses.clone())
            .unwrap_or_default(),
        tools: runtime
            .tools
            .lock()
            .map(|tools| tools.len())
            .unwrap_or_default(),
        loading: runtime.loading.load(Ordering::Relaxed),
        problems: runtime
            .problems
            .lock()
            .map(|problems| problems.clone())
            .unwrap_or_default(),
    }
}

/// Whether the landing page has a reason to keep repainting: a load is running,
/// or a server changed state a moment ago.
pub fn pending() -> bool {
    let runtime = runtime();
    runtime.loading.load(Ordering::Relaxed)
        || runtime
            .changed
            .lock()
            .is_ok_and(|changed| changed.elapsed() < Duration::from_millis(600))
}

fn clip(text: &str, width: usize) -> String {
    let flat = text.replace(['\r', '\n'], " ");
    let mut characters = flat.chars();
    let mut result = characters.by_ref().take(width).collect::<String>();
    if characters.next().is_some() {
        result.pop();
        result.push('…');
    }
    result
}

fn state_label(state: &State) -> String {
    match state {
        State::Starting => "starting".to_owned(),
        State::Ready { tools: 1 } => "1 tool".to_owned(),
        State::Ready { tools } => format!("{tools} tools"),
        State::Failed(error) => format!("failed: {}", clip(error, 120)),
        State::Disabled => "disabled".to_owned(),
    }
}

/// One line for `/mcp`: every server with its state.
pub fn summary() -> String {
    let report = report();
    if report.servers.is_empty() && report.problems.is_empty() {
        return "No MCP servers configured · add stdio or HTTP servers under `mcpServers` in \
                ~/.yopes/mcp.json or .mcp.json in the launch directory"
            .to_owned();
    }
    let ready = report
        .servers
        .iter()
        .filter(|server| matches!(server.state, State::Ready { .. }))
        .count();
    let total = report.servers.len();
    let mut line = if report.loading {
        format!("MCP starting · {ready}/{total} servers")
    } else {
        format!("MCP {ready}/{total} servers · {} tools", report.tools)
    };
    for server in &report.servers {
        line.push_str(&format!(
            " · {} {} ({}, {})",
            server.name,
            state_label(&server.state),
            server.transport,
            server.scope
        ));
    }
    for problem in &report.problems {
        line.push_str(&format!(" · config error: {}", clip(problem, 160)));
    }
    line
}

/// Run one `mcp__*` tool call.
pub fn call(name: &str, arguments: &str) -> Result<String> {
    let arguments: Value = if arguments.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(arguments).context("invalid MCP tool arguments")?
    };
    let runtime = runtime();
    let (index, target) = {
        let Ok(tools) = runtime.tools.lock() else {
            bail!("MCP lock poisoned");
        };
        let Some(tool) = tools.iter().find(|tool| tool.name == name) else {
            let problems = runtime
                .problems
                .lock()
                .map(|problems| {
                    problems
                        .iter()
                        .map(|problem| format!("; {problem}"))
                        .collect::<String>()
                })
                .unwrap_or_default();
            bail!("unknown MCP tool `{name}`{problems}");
        };
        (tool.slot, tool.target.clone())
    };
    match invoke(runtime, index, &target, &arguments) {
        Ok(text) => Ok(text),
        // The server answered with a failure: retrying cannot help.
        Err(error) if error.downcast_ref::<ReplyError>().is_some() => Err(error),
        Err(first) => {
            // A crashed process or a dropped connection is worth one restart;
            // the retry's own error is the one reported.
            drop_client(runtime, index, &first);
            if let Err(error) = connect_slot(runtime, index) {
                bail!("{first:#}; the server did not restart: {error:#}");
            }
            invoke(runtime, index, &target, &arguments)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::net::TcpListener;

    /// The global runtime is process state: tests that install servers take
    /// this lock so they never see each other's servers.
    static LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn names_are_provider_safe_and_a_transport_is_inferred() {
        assert_eq!(sanitize("my.server/x"), "my_server_x");
        assert!(is_mcp("mcp__fs__read"));
        assert!(!is_mcp("read_file"));
        assert_eq!(
            tool_name("fs", "read_file").as_deref(),
            Some("mcp__fs__read_file")
        );
        // A name past the provider limit is dropped instead of truncated.
        assert!(tool_name(&"s".repeat(60), "tool").is_none());

        let spec = |json: &str| -> ServerSpec { serde_json::from_str(json).unwrap() };
        assert_eq!(
            spec(r#"{"command":"npx","args":["-y","x"]}"#)
                .kind("fs")
                .unwrap(),
            Kind::Stdio
        );
        assert_eq!(
            spec(r#"{"url":"https://example.com/mcp"}"#)
                .kind("web")
                .unwrap(),
            Kind::Http
        );
        assert_eq!(
            spec(r#"{"type":"sse","url":"https://example.com/sse"}"#)
                .kind("web")
                .unwrap(),
            Kind::Sse
        );
        assert_eq!(
            spec(r#"{"transport":"stdio","command":"node"}"#)
                .kind("fs")
                .unwrap(),
            Kind::Stdio
        );
        assert!(spec(r#"{"type":"carrier-pigeon"}"#).kind("odd").is_err());
    }

    #[test]
    fn environment_placeholders_expand_or_are_reported() {
        let mut missing = Vec::new();
        assert_eq!(expand("plain", &mut missing), "plain");
        assert_eq!(
            expand("${YOPES_MCP_TEST_UNSET:-fallback}", &mut missing),
            "fallback"
        );
        assert_eq!(expand("${YOPES_MCP_TEST_UNSET}", &mut missing), "");
        assert_eq!(missing, vec!["YOPES_MCP_TEST_UNSET".to_owned()]);
        // A placeholder with no closing brace stays as written.
        let mut missing = Vec::new();
        assert_eq!(expand("${broken", &mut missing), "${broken");
        assert!(missing.is_empty());

        let spec: ServerSpec =
            serde_json::from_str(r#"{"url":"https://x.test","headers":{"authorization":"Bearer ${YOPES_MCP_TEST_UNSET}"}}"#)
                .unwrap();
        let error = prepare("web", &spec, Path::new("."), Scope::Global).unwrap_err();
        assert!(
            error.to_string().contains("YOPES_MCP_TEST_UNSET"),
            "{error:#}"
        );
    }

    #[test]
    fn sse_events_drop_the_stream_framing() {
        let mut reader = std::io::Cursor::new(
            "event: endpoint\ndata: /messages\n\ndata: {\"a\":1}\ndata: {\"b\":2}\n\n: comment\n\n",
        );
        assert_eq!(
            read_event(&mut reader).unwrap(),
            Some(("endpoint".to_owned(), "/messages".to_owned()))
        );
        assert_eq!(
            read_event(&mut reader).unwrap(),
            Some(("message".to_owned(), "{\"a\":1}\n{\"b\":2}".to_owned()))
        );
        assert_eq!(read_event(&mut reader).unwrap(), None);
    }

    #[test]
    fn an_sse_endpoint_resolves_against_the_stream_origin() {
        assert_eq!(
            resolve_url("http://127.0.0.1:9000/sse", "/messages?session=1"),
            "http://127.0.0.1:9000/messages?session=1"
        );
        assert_eq!(
            resolve_url("https://host/mcp/sse", "messages"),
            "https://host/messages"
        );
        assert_eq!(
            resolve_url("https://host/sse", "https://other/endpoint"),
            "https://other/endpoint"
        );
    }

    /// A transport that answers from a script, so the protocol layer is tested
    /// without a process or a socket.
    struct Fake {
        kind: &'static str,
        script: VecDeque<Value>,
        sent: Vec<Value>,
    }

    impl Transport for Fake {
        fn kind(&self) -> &'static str {
            self.kind
        }

        fn send(&mut self, message: &Value, _timeout: Duration) -> Result<()> {
            self.sent.push(message.clone());
            Ok(())
        }

        fn recv(&mut self, _wait: Duration) -> Result<Option<Value>> {
            Ok(self.script.pop_front())
        }
    }

    fn fake_client(script: Vec<Value>) -> Client {
        Client {
            name: "filesystem".to_owned(),
            transport: Box::new(Fake {
                kind: "stdio",
                script: script.into(),
                sent: Vec::new(),
            }),
            protocol: PROTOCOL_VERSION.to_owned(),
            capabilities: json!({"resources": {}}),
            timeout: Duration::from_secs(1),
            next_id: 0,
            tools_changed: false,
        }
    }

    #[test]
    fn every_tool_page_is_read_and_resources_bridge_into_tools() {
        let mut client = fake_client(vec![
            json!({"jsonrpc":"2.0","id":1,"result":{"tools":[
                {"name":"read_file","description":"read","inputSchema":{"type":"object"}}
            ],"nextCursor":"page-2"}}),
            json!({"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"write file"}]}}),
        ]);
        let tools = client.tools(0).unwrap();
        let names = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "mcp__filesystem__read_file",
                "mcp__filesystem__write_file",
                "mcp__filesystem__list_resources",
                "mcp__filesystem__read_resource"
            ]
        );
        assert_eq!(tools[0].description, "read");
        // A tool without an input schema still gets one.
        assert_eq!(tools[1].schema, json!({"type": "object"}));
    }

    #[test]
    fn a_tool_result_is_text_and_a_tool_failure_is_not_retried() {
        let mut client = fake_client(vec![json!({"jsonrpc":"2.0","id":1,"result":{"content":[
            {"type":"text","text":"line one"},
            {"type":"image","mimeType":"image/png","data":"AAAA"}
        ]}})]);
        let text = client
            .call(&Target::Tool("read_file".to_owned()), &json!({"path":"x"}))
            .unwrap();
        assert_eq!(text, "line one\n[image image/png omitted]");

        let mut client = fake_client(vec![json!({"jsonrpc":"2.0","id":1,"result":{
            "content":[{"type":"text","text":"boom"}],"isError":true
        }})]);
        let error = client
            .call(&Target::Tool("read_file".to_owned()), &json!({}))
            .unwrap_err();
        assert_eq!(error.to_string(), "boom");
        assert!(error.downcast_ref::<ReplyError>().is_some());
    }

    #[test]
    fn a_json_rpc_error_is_a_reply_error_and_notifications_are_skipped() {
        let mut client = fake_client(vec![
            json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed","params":{}}),
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no such tool"}}),
        ]);
        let error = client
            .call(&Target::Tool("nope".to_owned()), &json!({}))
            .unwrap_err();
        assert!(
            error.to_string().contains("no such tool (code -32601)"),
            "{error:#}"
        );
        assert!(error.downcast_ref::<ReplyError>().is_some());
        // The notification was seen, so the tool list is stale.
        assert!(client.tools_changed);
    }

    // ------------------------------------------------ localhost MCP server

    /// One canned HTTP reply: status, content type, extra headers, body.
    struct Reply {
        status: &'static str,
        content_type: &'static str,
        headers: &'static str,
        body: String,
    }

    fn json_reply(body: Value) -> Reply {
        Reply {
            status: "200 OK",
            content_type: "application/json",
            headers: "",
            body: body.to_string(),
        }
    }

    fn accepted() -> Reply {
        Reply {
            status: "202 Accepted",
            content_type: "application/json",
            headers: "",
            body: String::new(),
        }
    }

    /// The requests the fake server received, in order.
    struct FakeServer {
        url: String,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl FakeServer {
        fn request(&self, index: usize) -> String {
            match self.seen.lock() {
                Ok(seen) => seen.get(index).cloned().unwrap_or_default(),
                Err(_) => String::new(),
            }
        }
    }

    fn read_http(stream: &mut std::net::TcpStream) -> String {
        let mut data = Vec::new();
        let mut buffer = [0u8; 2048];
        loop {
            let read = stream.read(&mut buffer).unwrap_or(0);
            if read == 0 {
                break;
            }
            data.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&data).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|line| {
                        let lower = line.to_ascii_lowercase();
                        lower
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if data.len() >= end + 4 + length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&data).to_string()
    }

    fn fake_server(replies: Vec<Reply>) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        std::thread::spawn(move || {
            for reply in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let request = read_http(&mut stream);
                if let Ok(mut seen) = recorded.lock() {
                    seen.push(request);
                }
                let head = format!(
                    "HTTP/1.1 {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n{}\r\n",
                    reply.status,
                    reply.content_type,
                    reply.body.len(),
                    reply.headers
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(reply.body.as_bytes());
                let _ = stream.flush();
            }
        });
        FakeServer { url, seen }
    }

    /// A server for the legacy SSE transport: the GET stream announces
    /// `/messages`, and every POST to it is answered on that open stream.
    fn fake_sse_server(tools: Value) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/sse", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let open: Arc<Mutex<Option<std::net::TcpStream>>> = Arc::new(Mutex::new(None));
        let stream = Arc::clone(&open);
        std::thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut connection) = incoming else {
                    break;
                };
                let request = read_http(&mut connection);
                if let Ok(mut seen) = recorded.lock() {
                    seen.push(request.clone());
                }
                if request.starts_with("GET ") {
                    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: keep-alive\r\n\r\n";
                    let _ = connection.write_all(head.as_bytes());
                    let _ = connection.write_all(b"event: endpoint\r\ndata: /messages\r\n\r\n");
                    let _ = connection.flush();
                    // The clone keeps the stream open once this handle drops.
                    if let Ok(clone) = connection.try_clone() {
                        if let Ok(mut open) = stream.lock() {
                            *open = Some(clone);
                        }
                    }
                    continue;
                }
                let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
                if let Ok(message) = serde_json::from_str::<Value>(body) {
                    if let Some(reply) = sse_reply(&message, &tools) {
                        let frame = format!("data: {reply}\r\n\r\n");
                        if let Ok(mut open) = stream.lock() {
                            if let Some(open) = open.as_mut() {
                                let _ = open.write_all(frame.as_bytes());
                                let _ = open.flush();
                            }
                        }
                    }
                }
                let head =
                    "HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                let _ = connection.write_all(head.as_bytes());
            }
        });
        FakeServer { url, seen }
    }

    fn sse_reply(message: &Value, tools: &Value) -> Option<Value> {
        let id = message.get("id")?.clone();
        let result = match message.get("method").and_then(Value::as_str) {
            Some("initialize") => json!({
                "protocolVersion":"2025-06-18",
                "capabilities":{"tools":{}},
                "serverInfo":{"name":"fake","version":"0"}
            }),
            Some("tools/list") => json!({ "tools": tools }),
            Some("tools/call") => json!({"content":[{"type":"text","text":"hi from sse"}]}),
            _ => return None,
        };
        Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }

    fn http_prepared(name: &str, url: &str) -> Prepared {
        Prepared {
            name: name.to_owned(),
            scope: Scope::Project,
            kind: Kind::Http,
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            url: url.to_owned(),
            headers: BTreeMap::new(),
            timeout: Duration::from_secs(5),
            disabled: false,
        }
    }

    fn handshake(tools: Value) -> Vec<Reply> {
        vec![
            Reply {
                status: "200 OK",
                content_type: "application/json",
                headers: "mcp-session-id: abc123\r\n",
                body: json!({"jsonrpc":"2.0","id":1,"result":{
                    "protocolVersion":"2025-06-18",
                    "capabilities":{"tools":{}},
                    "serverInfo":{"name":"fake","version":"0"}
                }})
                .to_string(),
            },
            accepted(),
            json_reply(
                json!({"jsonrpc":"2.0","id":2,"result":{"tools":tools.as_array().cloned().unwrap_or_default()}}),
            ),
        ]
    }

    #[test]
    fn a_streamable_http_server_connects_gets_listed_and_answers_a_call() {
        let _guard = lock();
        let mut replies = handshake(json!([
            {"name":"echo","description":"Echo back","inputSchema":{"type":"object"}}
        ]));
        replies.push(json_reply(json!({"jsonrpc":"2.0","id":3,"result":{
            "content":[{"type":"text","text":"hello"}]
        }})));
        let server = fake_server(replies);

        let runtime = runtime();
        runtime.reset();
        install(runtime, vec![http_prepared("filesystem", &server.url)]);

        let report = report();
        assert_eq!(report.servers.len(), 1);
        assert_eq!(report.servers[0].name, "filesystem");
        assert_eq!(report.servers[0].transport, "http");
        assert!(
            matches!(report.servers[0].state, State::Ready { tools: 1 }),
            "{:?}",
            report.servers[0].state
        );
        assert_eq!(report.tools, 1);

        // The handshake says who we are, and the session id is echoed back.
        assert!(server.request(0).contains("yopes"), "{}", server.request(0));
        assert!(
            server
                .request(1)
                .to_ascii_lowercase()
                .contains("mcp-session-id: abc123"),
            "{}",
            server.request(1)
        );
        assert!(server.request(1).contains("notifications/initialized"));

        let text = call("mcp__filesystem__echo", r#"{"value":"hi"}"#).unwrap();
        assert_eq!(text, "hello");
        assert!(
            server
                .request(3)
                .contains("\"arguments\":{\"value\":\"hi\"}")
        );
        runtime.reset();
    }

    #[test]
    fn a_legacy_sse_server_announces_an_endpoint_and_answers_on_the_stream() {
        let _guard = lock();
        let server = fake_sse_server(json!([{"name":"echo","description":"Echo"}]));
        let runtime = runtime();
        runtime.reset();
        let mut prepared = http_prepared("events", &server.url);
        prepared.kind = Kind::Sse;
        install(runtime, vec![prepared]);
        let report = report();
        assert_eq!(report.servers[0].transport, "sse");
        assert!(
            matches!(report.servers[0].state, State::Ready { tools: 1 }),
            "{:?}",
            report.servers[0].state
        );
        assert_eq!(call("mcp__events__echo", "{}").unwrap(), "hi from sse");
        // The endpoint the stream announced is where the requests went.
        assert!(
            server.request(1).starts_with("POST /messages"),
            "{}",
            server.request(1)
        );
        runtime.reset();
    }

    #[test]
    fn an_event_stream_reply_is_read_to_the_matching_message() {
        let _guard = lock();
        let server = fake_server(vec![
            Reply {
                status: "200 OK",
                content_type: "text/event-stream",
                headers: "mcp-session-id: stream-1\r\n",
                body: concat!(
                    "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n\n",
                    "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{\"tools\":{}}}}\n\n"
                )
                .to_owned(),
            },
            accepted(),
            Reply {
                status: "200 OK",
                content_type: "text/event-stream",
                headers: "",
                body: "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"a\"},{\"name\":\"b\"}]}}\n\n"
                    .to_owned(),
            },
        ]);
        let runtime = runtime();
        runtime.reset();
        install(runtime, vec![http_prepared("stream", &server.url)]);
        let report = report();
        assert!(
            matches!(report.servers[0].state, State::Ready { tools: 2 }),
            "{:?}",
            report.servers[0].state
        );
        runtime.reset();
    }

    #[test]
    fn a_server_that_cannot_start_is_reported_and_kept_out_of_the_tool_list() {
        let _guard = lock();
        let runtime = runtime();
        runtime.reset();
        let broken = Prepared {
            name: "broken".to_owned(),
            scope: Scope::Global,
            kind: Kind::Stdio,
            command: "yopes-definitely-not-a-real-binary".to_owned(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            url: String::new(),
            headers: BTreeMap::new(),
            timeout: Duration::from_secs(5),
            disabled: false,
        };
        install(runtime, vec![broken]);
        let report = report();
        let State::Failed(error) = &report.servers[0].state else {
            panic!("{:?}", report.servers[0].state);
        };
        assert!(error.contains("broken"), "{error}");
        assert!(report.tools == 0);
        assert!(summary().contains("failed"));
        runtime.reset();
    }

    /// The landing page reads `report()`, so a connected server has to show up
    /// there: render it and look at the rows.
    #[test]
    fn the_landing_page_shows_every_server_and_its_state() {
        let _guard = lock();
        let server = fake_server(handshake(json!([
            {"name":"echo","description":"Echo"},{"name":"sum","description":"Add"}
        ])));
        let runtime = runtime();
        runtime.reset();
        let mut prepared = vec![http_prepared("filesystem", &server.url)];
        prepared.push(Prepared {
            name: "notes".to_owned(),
            scope: Scope::Global,
            kind: Kind::Stdio,
            command: "yopes-definitely-not-a-real-binary".to_owned(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            url: String::new(),
            headers: BTreeMap::new(),
            timeout: Duration::from_secs(5),
            disabled: false,
        });
        install(runtime, prepared);

        let mut app = crate::app::test_app();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 32)).expect("terminal");
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .expect("draw");
        let screen = (0..32)
            .map(|row| {
                let width = terminal.backend().buffer().area.width;
                (0..width)
                    .map(|column| {
                        terminal.backend().buffer()[(column, row)]
                            .symbol()
                            .to_string()
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("mcp"), "{screen}");
        assert!(screen.contains("filesystem"), "{screen}");
        assert!(screen.contains("2 tools"), "{screen}");
        assert!(screen.contains("notes"), "{screen}");
        assert!(screen.contains("failed"), "{screen}");
        // The quick keys still fit under the servers.
        assert!(screen.contains("quick keys"), "{screen}");
        runtime.reset();
    }
}
