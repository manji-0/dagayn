//! The stdio front end of `dagayn serve`.
//!
//! [`serve`] answers the MCP methods that only describe the server
//! (`initialize`, `ping`, the tool, prompt, and resource listings) from a
//! [`Surface`] recorded from the Python server, so a session can start without
//! loading it. Every other message (`tools/call`, `prompts/get`, anything
//! unknown, anything sent before `initialize`) goes to a [`Backend`], booted
//! on first use and fed over a pipe pair: the client's own `initialize` is
//! replayed to it first under [`PROXY_INIT_ID`], and its reply to that is
//! dropped, so from then on the backend sees the session the client opened
//! and its replies and notifications reach the client unchanged.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::{Map, Value, json};

/// Protocol revisions negotiated through `initialize`, oldest to newest
/// (`mcp_types.version.HANDSHAKE_PROTOCOL_VERSIONS`).
pub const HANDSHAKE_PROTOCOL_VERSIONS: [&str; 4] =
    ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// Request id of the replayed `initialize`; a string no client numbering hits.
pub const PROXY_INIT_ID: &str = "dagayn-proxy-init";

/// The parts of the Python server's replies that do not depend on the graph.
#[derive(Clone, Debug)]
pub struct Surface {
    /// `capabilities`, `instructions`, and `serverInfo.name` of `initialize`.
    pub capabilities: Value,
    pub instructions: Option<String>,
    pub server_name: String,
    /// Every registered tool, in listing order.
    pub tools: Vec<Value>,
    pub prompts: Vec<Value>,
}

impl Surface {
    /// `{"initialize": {...}, "tools": [...], "prompts": [...]}`, as
    /// `dagayn/server/mcp_surface.json` stores it.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
        let initialize = value.get("initialize").ok_or("surface has no initialize")?;
        let list = |key: &str| -> Result<Vec<Value>, String> {
            value
                .get(key)
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| format!("surface has no {key} list"))
        };
        Ok(Self {
            capabilities: initialize
                .get("capabilities")
                .cloned()
                .ok_or("surface has no capabilities")?,
            instructions: initialize
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_string),
            server_name: initialize
                .pointer("/serverInfo/name")
                .and_then(Value::as_str)
                .ok_or("surface has no serverInfo.name")?
                .to_string(),
            tools: list("tools")?,
            prompts: list("prompts")?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub surface: Surface,
    /// Tool names the session exposes; `None` exposes every tool.
    pub allowed_tools: Option<HashSet<String>>,
    /// `serverInfo.version`.
    pub version: String,
}

/// Tools this front end answers itself.
pub trait Native {
    /// The JSON text and value of `name(arguments)`, or `None` to delegate.
    fn call_tool(&self, name: &str, arguments: &Value) -> Option<(String, Value)>;
}

/// No native tools.
pub struct NoNative;

impl Native for NoNative {
    fn call_tool(&self, _name: &str, _arguments: &Value) -> Option<(String, Value)> {
        None
    }
}

/// Where delegated messages go.
pub trait Backend {
    /// Start serving MCP over newline-delimited JSON: read requests from
    /// `requests` and write replies to `replies` until `requests` reaches
    /// EOF, then close `replies`. Must return once the backend is running.
    fn boot(&mut self, requests: OwnedFd, replies: OwnedFd) -> io::Result<()>;
}

/// The descriptors of the process's stdin and stdout, duplicated; fd 0 then
/// reads the null device and fd 1 writes to stderr, as
/// `mcp.server.stdio.stdio_server` arranges, so a child or a stray `print`
/// cannot read the requests or write into the replies.
pub fn claim_stdio() -> io::Result<(File, File)> {
    // SAFETY: plain descriptor calls; every returned descriptor is checked
    // before it is wrapped, and each wrapped one is owned exactly once.
    unsafe {
        let input = checked(libc::dup(0))?;
        let output = checked(libc::dup(1))?;
        let devnull = checked(libc::open(c"/dev/null".as_ptr(), libc::O_RDWR))?;
        checked(libc::dup2(devnull, 0))?;
        checked(libc::dup2(2, 1))?;
        libc::close(devnull);
        Ok((File::from_raw_fd(input), File::from_raw_fd(output)))
    }
}

fn checked(fd: libc::c_int) -> io::Result<libc::c_int> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

fn pipe() -> io::Result<(File, File)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `pipe` fills both slots on success; each is owned once below.
    unsafe {
        checked(libc::pipe(fds.as_mut_ptr()))?;
        Ok((File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])))
    }
}

type Output = Arc<Mutex<File>>;

fn write_line(output: &Output, line: &str) -> io::Result<()> {
    let mut out = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

fn reply(output: &Output, id: &Value, result: Value) -> io::Result<()> {
    write_line(
        output,
        &json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
    )
}

/// The backend once booted: the request pipe and the thread relaying its
/// replies.
struct Proxy {
    requests: File,
    relay: JoinHandle<()>,
}

struct Session<'n, B: Backend> {
    config: Config,
    backend: B,
    native: &'n dyn Native,
    output: Output,
    /// The client's `initialize` params, once it sent them.
    init_params: Option<Value>,
    initialized: bool,
    proxy: Option<Proxy>,
}

impl<B: Backend> Session<'_, B> {
    fn handle(&mut self, line: &str) -> io::Result<()> {
        // A line that is not JSON gets the backend's parse error; it boots the
        // backend, as any message this front end does not answer does.
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return self.delegate(line);
        };
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id");
        match (method, id) {
            (Some("initialize"), Some(id))
                if self.init_params.is_none() && self.proxy.is_none() =>
            {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let result = self.initialize_result(&params);
                self.init_params = Some(params);
                reply(&self.output, id, result)
            }
            // The backend already serves a session that began before
            // `initialize`: it answers this one, and the listings are local
            // again from here on.
            (Some("initialize"), Some(_)) if self.init_params.is_none() => {
                self.init_params = Some(message.get("params").cloned().unwrap_or(Value::Null));
                self.delegate(line)
            }
            (Some(method), None) => {
                if method == "notifications/initialized" && self.proxy.is_none() {
                    self.initialized = true;
                    return Ok(());
                }
                self.forward_if_booted(line)
            }
            (Some(method), Some(id)) => {
                let params = message.get("params");
                let result = match method {
                    "tools/call" => self.native_call(params),
                    _ => self.local_result(method, params),
                };
                match result {
                    Some(result) => reply(&self.output, id, result),
                    None => self.delegate(line),
                }
            }
            // A reply to a request the backend sent the client.
            (None, _) => self.forward_if_booted(line),
        }
    }

    fn initialize_result(&self, params: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let version = requested
            .filter(|version| HANDSHAKE_PROTOCOL_VERSIONS.contains(version))
            .unwrap_or(HANDSHAKE_PROTOCOL_VERSIONS[HANDSHAKE_PROTOCOL_VERSIONS.len() - 1]);
        let mut result = Map::new();
        result.insert("protocolVersion".into(), json!(version));
        result.insert(
            "capabilities".into(),
            self.config.surface.capabilities.clone(),
        );
        result.insert(
            "serverInfo".into(),
            json!({"name": self.config.surface.server_name, "version": self.config.version}),
        );
        if let Some(instructions) = &self.config.surface.instructions {
            result.insert("instructions".into(), json!(instructions));
        }
        Value::Object(result)
    }

    /// The reply this front end gives itself, or `None` to delegate. Only
    /// after `initialize`, never for a request in the 2026-07-28 envelope,
    /// and for the listings only without parameters (a pagination cursor goes
    /// to the backend).
    fn local_result(&self, method: &str, params: Option<&Value>) -> Option<Value> {
        self.init_params.as_ref()?;
        let modern_envelope = params
            .and_then(|params| params.pointer("/_meta"))
            .and_then(Value::as_object)
            .is_some_and(|meta| meta.contains_key("io.modelcontextprotocol/protocolVersion"));
        if modern_envelope {
            return None;
        }
        if method == "logging/setLevel" {
            // The backend keeps its own level once it runs.
            return self.proxy.is_none().then(|| json!({}));
        }
        let plain = match params {
            None | Some(Value::Null) => true,
            Some(Value::Object(map)) => map.is_empty(),
            Some(_) => false,
        };
        if !plain {
            return None;
        }
        let surface = &self.config.surface;
        match method {
            "ping" => Some(json!({})),
            "tools/list" => {
                let tools: Vec<Value> = surface
                    .tools
                    .iter()
                    .filter(|tool| {
                        let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
                        self.config
                            .allowed_tools
                            .as_ref()
                            .is_none_or(|allowed| allowed.contains(name))
                    })
                    .cloned()
                    .collect();
                Some(json!({"tools": tools}))
            }
            "prompts/list" => Some(json!({"prompts": surface.prompts})),
            "resources/list" => Some(json!({"resources": []})),
            "resources/templates/list" => Some(json!({"resourceTemplates": []})),
            _ => None,
        }
    }

    /// A `tools/call` result from [`Native`], shaped as fastmcp shapes a
    /// tool's dict: the JSON as text and as `structuredContent`. Only for an
    /// exposed tool, after `initialize`, and for params that carry nothing
    /// but the name, the arguments, and a plain `_meta`.
    fn native_call(&self, params: Option<&Value>) -> Option<Value> {
        self.init_params.as_ref()?;
        let params = params?.as_object()?;
        if !params
            .keys()
            .all(|key| matches!(key.as_str(), "name" | "arguments" | "_meta"))
        {
            return None;
        }
        let modern_envelope = params
            .get("_meta")
            .and_then(Value::as_object)
            .is_some_and(|meta| meta.contains_key("io.modelcontextprotocol/protocolVersion"));
        if modern_envelope {
            return None;
        }
        let name = params.get("name")?.as_str()?;
        let listed = self
            .config
            .surface
            .tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name));
        let exposed = listed
            && self
                .config
                .allowed_tools
                .as_ref()
                .is_none_or(|allowed| allowed.contains(name));
        if !exposed {
            return None;
        }
        let empty = json!({});
        let arguments = params.get("arguments").unwrap_or(&empty);
        let (text, value) = self.native.call_tool(name, arguments)?;
        if std::env::var_os("DAGAYN_MCP_TRACE").is_some() {
            eprintln!("dagayn: answered {name} in Rust");
        }
        Some(json!({
            "content": [{"type": "text", "text": text}],
            "structuredContent": value,
            "isError": false,
        }))
    }

    fn forward_if_booted(&mut self, line: &str) -> io::Result<()> {
        match &mut self.proxy {
            Some(proxy) => send(&mut proxy.requests, line),
            None => Ok(()),
        }
    }

    fn delegate(&mut self, line: &str) -> io::Result<()> {
        if self.proxy.is_none() {
            self.boot()?;
        }
        self.forward_if_booted(line)
    }

    fn boot(&mut self) -> io::Result<()> {
        let (requests_read, mut requests_write) = pipe()?;
        let (replies_read, replies_write) = pipe()?;
        self.backend
            .boot(OwnedFd::from(requests_read), OwnedFd::from(replies_write))?;
        let output = Arc::clone(&self.output);
        let relay = std::thread::spawn(move || relay(replies_read, &output));
        if let Some(params) = &self.init_params {
            let init = json!({
                "jsonrpc": "2.0",
                "id": PROXY_INIT_ID,
                "method": "initialize",
                "params": params,
            });
            send(&mut requests_write, &init.to_string())?;
            if self.initialized {
                send(
                    &mut requests_write,
                    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                )?;
            }
        }
        self.proxy = Some(Proxy {
            requests: requests_write,
            relay,
        });
        Ok(())
    }
}

fn send(pipe: &mut File, line: &str) -> io::Result<()> {
    pipe.write_all(line.as_bytes())?;
    pipe.write_all(b"\n")?;
    pipe.flush()
}

/// Copy the backend's lines to the client, except its reply to the replayed
/// `initialize`.
fn relay(replies: File, output: &Output) {
    for line in BufReader::new(replies).lines() {
        let Ok(line) = line else { break };
        let is_init_reply = serde_json::from_str::<Value>(&line)
            .ok()
            .is_some_and(|message| message.get("id") == Some(&json!(PROXY_INIT_ID)));
        if is_init_reply {
            continue;
        }
        if write_line(output, &line).is_err() {
            break;
        }
    }
}

/// Serve the session on `input` until EOF, writing replies to `output`.
/// Returns after the backend, if it was booted, has closed its replies, so
/// whatever the caller set up around the server (an embedding sidecar) can
/// be torn down normally.
pub fn serve(
    config: Config,
    input: File,
    output: File,
    backend: impl Backend,
    native: &dyn Native,
) -> io::Result<()> {
    let mut session = Session {
        config,
        backend,
        native,
        output: Arc::new(Mutex::new(output)),
        init_params: None,
        initialized: false,
        proxy: None,
    };
    let mut result = Ok(());
    for line in BufReader::new(input).lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                result = Err(err);
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Err(err) = session.handle(&line) {
            result = Err(err);
            break;
        }
    }
    if let Some(proxy) = session.proxy.take() {
        drop(proxy.requests);
        let _ = proxy.relay.join();
    }
    result
}

#[cfg(test)]
mod tests;
