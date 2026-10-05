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

/// The protocol revision that drops `initialize` for a per-request `_meta`
/// envelope (`mcp_types._v2026_07_28`).
pub const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";

const PROTOCOL_VERSION_META: &str = "io.modelcontextprotocol/protocolVersion";
const SERVER_INFO_META: &str = "io.modelcontextprotocol/serverInfo";

/// The protocol revision a request's `_meta` envelope names, if it has one
/// (`mcp.server.runner._has_modern_envelope`).
fn envelope_version(params: Option<&Value>) -> Option<Option<&str>> {
    let meta = params?.get("_meta")?.as_object()?;
    meta.contains_key(PROTOCOL_VERSION_META)
        .then(|| meta.get(PROTOCOL_VERSION_META).and_then(Value::as_str))
}

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
    /// The 2026-07-28 `server/discover` result, without its `_meta` stamp.
    pub discover: Option<Value>,
    /// Recorded `prompts/get` results by prompt: `default` (no arguments)
    /// and, per argument, `empty` and `template` (the value replaced by
    /// [`PROMPT_ARGUMENT_PLACEHOLDER`]).
    pub prompt_replies: Map<String, Value>,
}

/// `tools/mcp_snapshot.py`'s `PROMPT_ARGUMENT_PLACEHOLDER`.
pub const PROMPT_ARGUMENT_PLACEHOLDER: &str = "\u{0}dagayn-prompt-argument\u{0}";

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
            discover: value.get("discover").cloned(),
            prompt_replies: value
                .get("prompt_replies")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        })
    }
}

/// Every string in `value` with the placeholder replaced by `argument`.
fn fill_placeholder(value: &mut Value, argument: &str) {
    match value {
        Value::String(text) if text.contains(PROMPT_ARGUMENT_PLACEHOLDER) => {
            *text = text.replace(PROMPT_ARGUMENT_PLACEHOLDER, argument);
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| fill_placeholder(item, argument)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|item| fill_placeholder(item, argument)),
        _ => {}
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

    /// Whether `name` writes the graph database. Such a call is answered
    /// only before the backend runs: its SQLite connections and a native
    /// writer in the same process cannot see each other's locks.
    fn writes_graph(&self, _name: &str) -> bool {
        false
    }
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
    send(&mut out, line)
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
    /// The `_meta` envelope of the 2026-07-28 request the client opened the
    /// connection with: it has no `initialize`, and every request carries
    /// one. Replayed to the backend as `server/discover` so it serves the
    /// same era.
    modern: Option<Value>,
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
                if self.init_params.is_none() && self.proxy.is_none() && self.modern.is_none() =>
            {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let result = self.initialize_result(&params);
                self.init_params = Some(params);
                reply(&self.output, id, result)
            }
            // The backend already serves a session that began before
            // `initialize`: it answers this one, and the listings are local
            // again from here on.
            (Some("initialize"), Some(_))
                if self.init_params.is_none() && self.modern.is_none() =>
            {
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
                if let Some(version) = envelope_version(params) {
                    // The first request decides the connection's era, once
                    // (`serve_dual_era_loop`); a modern request on a
                    // handshake connection, or a revision this front end
                    // does not speak, gets the backend's error.
                    let fresh = self.init_params.is_none() && self.proxy.is_none();
                    if (self.modern.is_some() || fresh) && version == Some(MODERN_PROTOCOL_VERSION)
                    {
                        if self.modern.is_none() {
                            self.modern = params.and_then(|params| params.get("_meta")).cloned();
                        }
                        if let Some(result) = self.modern_result(method, params) {
                            return reply(&self.output, id, result);
                        }
                    }
                    return self.delegate(line);
                }
                let result = match method {
                    "tools/call" => self.native_call(params),
                    "prompts/get" => self.prompt_get(params),
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
        if envelope_version(params).is_some() {
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
            "tools/list" => Some(json!({"tools": self.exposed_tools()})),
            "prompts/list" => Some(json!({"prompts": surface.prompts})),
            "resources/list" => Some(json!({"resources": []})),
            "resources/templates/list" => Some(json!({"resourceTemplates": []})),
            _ => None,
        }
    }

    /// A `prompts/get` result from the recorded replies: after `initialize`,
    /// for params that carry nothing but the name, string arguments, and a
    /// plain `_meta`. Arguments the prompt does not declare are ignored, as
    /// fastmcp ignores them; more than one given argument is the backend's.
    fn prompt_get(&self, params: Option<&Value>) -> Option<Value> {
        self.init_params.as_ref()?;
        let params = params?.as_object()?;
        if !params
            .keys()
            .all(|key| matches!(key.as_str(), "name" | "arguments" | "_meta"))
            || params.get("_meta").is_some_and(|meta| {
                meta.as_object()
                    .is_none_or(|meta| meta.contains_key(PROTOCOL_VERSION_META))
            })
        {
            return None;
        }
        let name = params.get("name")?.as_str()?;
        let replies = self.config.surface.prompt_replies.get(name)?;
        let declared = replies.get("arguments")?.as_object()?;
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(arguments)) => arguments.clone(),
            Some(_) => return None,
        };
        if !arguments.values().all(Value::is_string) {
            return None;
        }
        let given: Vec<(&String, &str)> = arguments
            .iter()
            .filter(|(key, _)| declared.contains_key(*key))
            .filter_map(|(key, value)| value.as_str().map(|value| (key, value)))
            .collect();
        let result = match given.as_slice() {
            [] => replies.get("default")?.clone(),
            [(argument, "")] => declared.get(*argument)?.get("empty")?.clone(),
            [(argument, value)] => {
                if value.contains(PROMPT_ARGUMENT_PLACEHOLDER) {
                    return None;
                }
                let mut template = declared.get(*argument)?.get("template")?.clone();
                fill_placeholder(&mut template, value);
                template
            }
            _ => return None,
        };
        if std::env::var_os("DAGAYN_MCP_TRACE").is_some() {
            eprintln!("dagayn: answered prompt {name} in Rust");
        }
        Some(result)
    }

    /// A `tools/call` result from [`Native`], shaped as fastmcp shapes a
    /// tool's dict: the JSON as text and as `structuredContent`. Only for an
    /// exposed tool, after `initialize`, and for params that carry nothing
    /// but the name, the arguments, and a plain `_meta`.
    fn native_call(&self, params: Option<&Value>) -> Option<Value> {
        self.init_params.as_ref()?;
        let (text, value) = self.native_answer(params)?;
        Some(json!({
            "content": [{"type": "text", "text": text}],
            "structuredContent": value,
            "isError": false,
        }))
    }

    /// The text and value of a `tools/call` this front end answers: an
    /// exposed tool, params that carry nothing but the name, the arguments,
    /// and `_meta`, and a [`Native`] answer.
    fn native_answer(&self, params: Option<&Value>) -> Option<(String, Value)> {
        let params = params?.as_object()?;
        if !params
            .keys()
            .all(|key| matches!(key.as_str(), "name" | "arguments" | "_meta"))
        {
            return None;
        }
        if self.modern.is_none() && envelope_version(Some(&Value::Object(params.clone()))).is_some()
        {
            return None;
        }
        let name = params.get("name")?.as_str()?;
        let listed = self
            .config
            .surface
            .tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name));
        if !(listed && self.allows(name)) {
            return None;
        }
        if self.proxy.is_some() && self.native.writes_graph(name) {
            return None;
        }
        let empty = json!({});
        let arguments = params.get("arguments").unwrap_or(&empty);
        let answer = self.native.call_tool(name, arguments)?;
        if std::env::var_os("DAGAYN_MCP_TRACE").is_some() {
            eprintln!("dagayn: answered {name} in Rust");
        }
        Some(answer)
    }

    /// `serverInfo`, as the 2026-07-28 protocol stamps it on every result.
    fn server_info_stamp(&self) -> Value {
        json!({SERVER_INFO_META: {
            "name": self.config.surface.server_name,
            "version": self.config.version,
        }})
    }

    /// A 2026-07-28 request this front end answers, shaped as the Python
    /// SDK's runner shapes it: `server/discover` from the recorded result,
    /// the listings (params with nothing but `_meta`, so no cursor), and a
    /// native `tools/call`, each with `resultType` and the `serverInfo`
    /// stamp. `None` delegates.
    fn modern_result(&self, method: &str, params: Option<&Value>) -> Option<Value> {
        let only_meta = params
            .and_then(Value::as_object)
            .is_some_and(|map| map.keys().all(|key| key == "_meta"));
        let listing = |key: &str, items: Value| -> Value {
            json!({
                "cacheScope": "private",
                key: items,
                "resultType": "complete",
                "ttlMs": 0,
                "_meta": self.server_info_stamp(),
            })
        };
        match method {
            "server/discover" if only_meta => {
                let mut result = self.config.surface.discover.clone()?;
                result
                    .as_object_mut()?
                    .insert("_meta".into(), self.server_info_stamp());
                Some(result)
            }
            "tools/list" if only_meta => Some(listing("tools", json!(self.exposed_tools()))),
            "prompts/list" if only_meta => {
                Some(listing("prompts", json!(self.config.surface.prompts)))
            }
            "resources/list" if only_meta => Some(listing("resources", json!([]))),
            "tools/call" => {
                let (text, value) = self.native_answer(params)?;
                Some(json!({
                    "content": [{"type": "text", "text": text}],
                    "isError": false,
                    "resultType": "complete",
                    "structuredContent": value,
                    "_meta": self.server_info_stamp(),
                }))
            }
            _ => None,
        }
    }

    /// Whether the session's allow-list admits the tool `name`.
    fn allows(&self, name: &str) -> bool {
        self.config
            .allowed_tools
            .as_ref()
            .is_none_or(|allowed| allowed.contains(name))
    }

    /// The tools this session exposes, in listing order.
    fn exposed_tools(&self) -> Vec<Value> {
        self.config
            .surface
            .tools
            .iter()
            .filter(|tool| self.allows(tool.get("name").and_then(Value::as_str).unwrap_or("")))
            .cloned()
            .collect()
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
        if let Some(meta) = &self.modern {
            // The backend decides its era from the first request it reads.
            let discover = json!({
                "jsonrpc": "2.0",
                "id": PROXY_INIT_ID,
                "method": "server/discover",
                "params": {"_meta": meta},
            });
            send(&mut requests_write, &discover.to_string())?;
        } else if let Some(params) = &self.init_params {
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
/// `initialize` or `server/discover`.
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
        modern: None,
        proxy: None,
    };
    let result = BufReader::new(input).lines().try_for_each(|line| {
        let line = line?;
        if line.trim().is_empty() {
            return Ok(());
        }
        session.handle(&line)
    });
    if let Some(proxy) = session.proxy.take() {
        drop(proxy.requests);
        let _ = proxy.relay.join();
    }
    result
}

#[cfg(test)]
mod tests;
