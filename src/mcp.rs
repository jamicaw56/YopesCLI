//! MCP client: stdio servers listed in `~/.yopes/mcp.json`. Each server's
//! tools join the tool list as `mcp__<server>__<tool>` and need approval
//! on every call.
//!
//! Servers start in a background thread on the first request, so a slow
//! server adds its tools on a later request instead of stalling the first one.
//!
//! ponytail: servers stay up for the process; no restart after a crash, no
//! resources or prompts. Add when a server needs them.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Mutex, Once, OnceLock, mpsc},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};

const PREFIX: &str = "mcp__";
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct FileConfig {
    #[serde(default, rename = "mcpServers")]
    servers: BTreeMap<String, ServerSpec>,
}

#[derive(Deserialize)]
struct ServerSpec {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

struct Server {
    name: String,
    child: Child,
    stdin: ChildStdin,
    replies: mpsc::Receiver<String>,
    next_id: u64,
}

impl Server {
    fn start(name: &str, spec: &ServerSpec) -> Result<Self> {
        let mut child = Command::new(&spec.command)
            .args(&spec.args)
            .envs(&spec.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("starting MCP server {name}"))?;
        let stdin = child.stdin.take().context("MCP server has no stdin")?;
        let stdout = child.stdout.take().context("MCP server has no stdout")?;
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            name: name.to_owned(),
            child,
            stdin,
            replies,
            next_id: 0,
        };
        server.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "yopes", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
        server.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(server)
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        writeln!(self.stdin, "{message}")
            .and_then(|()| self.stdin.flush())
            .with_context(|| format!("writing to MCP server {}", self.name))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .replies
                .recv_timeout(left)
                .map_err(|_| anyhow!("MCP server {} gave no reply to {method}", self.name))?;
            // Notifications and other ids are not this request's reply.
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("MCP {method} failed: {error}");
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Tool {
    name: String,
    remote: String,
    server: usize,
    description: String,
    schema: Value,
}

#[derive(Default)]
struct Registry {
    servers: Vec<Mutex<Server>>,
    tools: Vec<Tool>,
    errors: Vec<String>,
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(load)
}

/// Start loading the servers off the request path, once.
fn start() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(|| {
            registry();
        });
    });
}

fn load() -> Registry {
    let mut registry = Registry::default();
    let path = crate::config::yopes_home().join("mcp.json");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return registry,
        Err(error) => {
            registry.errors.push(format!("{}: {error}", path.display()));
            return registry;
        }
    };
    let file: FileConfig = match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(error) => {
            registry.errors.push(format!("{}: {error}", path.display()));
            return registry;
        }
    };
    for (name, spec) in &file.servers {
        let started = Server::start(name, spec).and_then(|mut server| {
            let listed = server.request("tools/list", json!({}))?;
            Ok((server, listed))
        });
        match started {
            Ok((server, listed)) => {
                let index = registry.servers.len();
                for tool in listed["tools"].as_array().into_iter().flatten() {
                    let Some(remote) = tool["name"].as_str() else {
                        continue;
                    };
                    let schema = if tool["inputSchema"].is_object() {
                        tool["inputSchema"].clone()
                    } else {
                        json!({"type": "object"})
                    };
                    registry.tools.push(Tool {
                        name: format!("{PREFIX}{}__{}", sanitize(name), sanitize(remote)),
                        remote: remote.to_owned(),
                        server: index,
                        description: tool["description"].as_str().unwrap_or_default().to_owned(),
                        schema,
                    });
                }
                registry.servers.push(Mutex::new(server));
            }
            Err(error) => registry
                .errors
                .push(format!("MCP server {name}: {error:#}")),
        }
    }
    registry
}

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

pub fn is_mcp(name: &str) -> bool {
    name.starts_with(PREFIX)
}

/// Tool schemas for every MCP tool, in the provider's function-tool shape.
pub fn definitions() -> Vec<Value> {
    start();
    // Servers still starting add no tools yet; a later request picks them up.
    let Some(registry) = REGISTRY.get() else {
        return Vec::new();
    };
    registry
        .tools
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

pub fn call(name: &str, arguments: &str) -> Result<String> {
    let registry = registry();
    let Some(tool) = registry.tools.iter().find(|tool| tool.name == name) else {
        let problems: String = registry.errors.iter().map(|e| format!("; {e}")).collect();
        bail!("unknown MCP tool `{name}`{problems}");
    };
    let arguments: Value = if arguments.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(arguments).context("invalid MCP tool arguments")?
    };
    let mut server = registry.servers[tool.server]
        .lock()
        .map_err(|_| anyhow!("MCP server lock poisoned"))?;
    let result = server.request(
        "tools/call",
        json!({"name": tool.remote, "arguments": arguments}),
    )?;
    let text = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if result["isError"].as_bool() == Some(true) {
        bail!("{text}");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_provider_safe_and_config_parses() {
        assert_eq!(sanitize("my.server/x"), "my_server_x");
        assert!(is_mcp("mcp__fs__read"));
        assert!(!is_mcp("read_file"));
        let file: FileConfig = serde_json::from_str(
            r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","x"],"env":{"A":"1"}}}}"#,
        )
        .unwrap();
        assert_eq!(file.servers["fs"].args, ["-y", "x"]);
    }
}
