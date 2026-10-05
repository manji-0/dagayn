use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::{Backend, Config, Native, PROXY_INIT_ID, Surface, pipe, serve};

/// Answers every request with `{"echo": <method>}` and records what it read.
#[derive(Clone, Default)]
struct Echo {
    received: Arc<Mutex<Vec<Value>>>,
    boots: Arc<Mutex<usize>>,
}

impl Backend for Echo {
    fn boot(&mut self, requests: OwnedFd, replies: OwnedFd) -> io::Result<()> {
        *self.boots.lock().unwrap() += 1;
        let received = Arc::clone(&self.received);
        std::thread::spawn(move || {
            let mut replies = File::from(replies);
            for line in BufReader::new(File::from(requests)).lines() {
                let message: Value = serde_json::from_str(&line.unwrap()).unwrap();
                received.lock().unwrap().push(message.clone());
                if let (Some(id), Some(method)) = (message.get("id"), message.get("method")) {
                    let reply = json!({"jsonrpc": "2.0", "id": id, "result": {"echo": method}});
                    writeln!(replies, "{reply}").unwrap();
                }
            }
        });
        Ok(())
    }
}

fn surface() -> Surface {
    Surface::from_json(
        &json!({
            "initialize": {
                "capabilities": {"tools": {"listChanged": true}},
                "instructions": "use the graph",
                "serverInfo": {"name": "dagayn", "version": "ignored"},
            },
            "discover": {
                "ttlMs": 0,
                "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {"listChanged": false}},
                "resultType": "complete",
            },
            "tools": [{"name": "a_tool"}, {"name": "b_tool"}],
            "prompts": [{"name": "a_prompt"}],
            "prompt_replies": {
                "a_prompt": {
                    "default": {"messages": [{"content": {"text": "on HEAD~1"}}]},
                    "arguments": {
                        "base": {
                            "empty": {"messages": [{"content": {"text": "on <base>"}}]},
                            "template": {"messages": [{"content": {
                                "text": format!("on {}", super::PROMPT_ARGUMENT_PLACEHOLDER)
                            }}]},
                        },
                    },
                },
            },
        })
        .to_string(),
    )
    .unwrap()
}

/// Answers `a_tool` with its arguments.
struct EchoA;

impl Native for EchoA {
    fn call_tool(&self, name: &str, arguments: &Value) -> Option<(String, Value)> {
        (name == "a_tool").then(|| (arguments.to_string(), arguments.clone()))
    }
}

fn run(messages: &[Value], allowed: Option<&[&str]>, backend: Echo) -> Vec<Value> {
    run_with(messages, allowed, backend, &super::NoNative)
}

fn run_with(
    messages: &[Value],
    allowed: Option<&[&str]>,
    backend: Echo,
    native: &dyn Native,
) -> Vec<Value> {
    let (input_read, mut input_write) = pipe().unwrap();
    let (output_read, output_write) = pipe().unwrap();
    for message in messages {
        writeln!(input_write, "{message}").unwrap();
    }
    drop(input_write);
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let mut output_read = output_read;
        output_read.read_to_string(&mut text).unwrap();
        text
    });
    let config = Config {
        surface: surface(),
        allowed_tools: allowed.map(|names| {
            names
                .iter()
                .map(|name| name.to_string())
                .collect::<HashSet<_>>()
        }),
        version: "7.1.1".to_string(),
    };
    serve(config, input_read, output_write, backend, native).unwrap();
    reader
        .join()
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn init(version: &str) -> Value {
    request(
        0,
        "initialize",
        json!({"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "t"}}),
    )
}

fn initialized() -> Value {
    json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
}

#[test]
fn a_listing_session_never_boots_the_backend() {
    let backend = Echo::default();
    let replies = run(
        &[
            init("2025-06-18"),
            initialized(),
            request(1, "tools/list", json!({})),
            request(2, "prompts/list", json!({})),
            request(3, "ping", json!({})),
            request(4, "resources/list", json!({})),
            request(5, "resources/templates/list", json!({})),
            request(6, "logging/setLevel", json!({"level": "info"})),
        ],
        Some(&["b_tool"]),
        backend.clone(),
    );
    assert_eq!(*backend.boots.lock().unwrap(), 0);
    let result = &replies[0]["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(
        result["serverInfo"],
        json!({"name": "dagayn", "version": "7.1.1"})
    );
    assert_eq!(result["instructions"], "use the graph");
    assert_eq!(replies[1]["result"], json!({"tools": [{"name": "b_tool"}]}));
    assert_eq!(
        replies[2]["result"],
        json!({"prompts": [{"name": "a_prompt"}]})
    );
    assert_eq!(replies[3]["result"], json!({}));
    assert_eq!(replies[4]["result"], json!({"resources": []}));
    assert_eq!(replies[5]["result"], json!({"resourceTemplates": []}));
    assert_eq!(replies[6]["result"], json!({}));
}

#[test]
fn an_unknown_version_gets_the_newest_handshake_version() {
    let replies = run(&[init("1999-01-01")], None, Echo::default());
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
    let replies = run(&[init("2024-11-05")], None, Echo::default());
    assert_eq!(replies[0]["result"]["protocolVersion"], "2024-11-05");
}

#[test]
fn a_tool_call_boots_the_backend_with_the_session_replayed() {
    let backend = Echo::default();
    let call = request(7, "tools/call", json!({"name": "a_tool", "arguments": {}}));
    let replies = run(
        &[
            init("2025-06-18"),
            initialized(),
            call.clone(),
            request(8, "tools/call", json!({"name": "b_tool", "arguments": {}})),
            request(9, "tools/list", json!({})),
            json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 8}}),
        ],
        None,
        backend.clone(),
    );
    assert_eq!(*backend.boots.lock().unwrap(), 1);
    let received = backend.received.lock().unwrap().clone();
    assert_eq!(received[0]["id"], PROXY_INIT_ID);
    assert_eq!(received[0]["params"]["clientInfo"]["name"], "t");
    assert_eq!(received[1], initialized());
    assert_eq!(received[2], call);
    assert_eq!(received[4]["method"], "notifications/cancelled");
    assert_eq!(received.len(), 5, "tools/list stays local: {received:?}");
    let ids: Vec<&Value> = replies.iter().map(|reply| &reply["id"]).collect();
    assert!(!ids.contains(&&json!(PROXY_INIT_ID)), "{replies:?}");
    assert!(
        replies.contains(&json!({"jsonrpc": "2.0", "id": 7, "result": {"echo": "tools/call"}}))
    );
    assert!(
        replies.contains(&json!({"jsonrpc": "2.0", "id": 8, "result": {"echo": "tools/call"}}))
    );
}

#[test]
fn a_request_before_initialize_goes_to_the_backend_as_sent() {
    let backend = Echo::default();
    let replies = run(
        &[request(1, "tools/list", json!({}))],
        None,
        backend.clone(),
    );
    let received = backend.received.lock().unwrap().clone();
    assert_eq!(received, vec![request(1, "tools/list", json!({}))]);
    assert_eq!(
        replies,
        vec![json!({"jsonrpc": "2.0", "id": 1, "result": {"echo": "tools/list"}})]
    );
}

#[test]
fn initialize_after_an_early_request_goes_to_the_backend_and_listings_resume() {
    let backend = Echo::default();
    let replies = run(
        &[
            request(1, "ping", json!({})),
            init("2025-06-18"),
            initialized(),
            request(2, "tools/list", json!({})),
        ],
        None,
        backend.clone(),
    );
    let received = backend.received.lock().unwrap().clone();
    assert_eq!(
        received,
        vec![
            request(1, "ping", json!({})),
            init("2025-06-18"),
            initialized()
        ]
    );
    // Local replies may overtake relayed ones.
    let by_id = |id: i64| replies.iter().find(|reply| reply["id"] == id).unwrap();
    assert_eq!(by_id(0)["result"], json!({"echo": "initialize"}));
    assert_eq!(
        by_id(2)["result"]["tools"].as_array().map(Vec::len),
        Some(2)
    );
}

#[test]
fn parameters_and_unknown_methods_go_to_the_backend() {
    let backend = Echo::default();
    let replies = run(
        &[
            init("2025-06-18"),
            request(1, "tools/list", json!({"cursor": "x"})),
            request(2, "no/such_method", json!({})),
            request(3, "logging/setLevel", json!({})),
        ],
        None,
        backend.clone(),
    );
    let methods: Vec<&Value> = replies[1..]
        .iter()
        .map(|reply| &reply["result"]["echo"])
        .collect();
    assert_eq!(
        methods,
        ["tools/list", "no/such_method", "logging/setLevel"]
    );
}

#[test]
fn native_tools_answer_exposed_calls_without_booting() {
    let backend = Echo::default();
    let call = |id: i64, name: &str, params_extra: Value| {
        let mut params = json!({"name": name, "arguments": {"k": 1}});
        if let (Some(params), Some(extra)) = (params.as_object_mut(), params_extra.as_object()) {
            params.extend(extra.clone());
        }
        request(id, "tools/call", params)
    };
    let replies = run_with(
        &[
            init("2025-06-18"),
            initialized(),
            call(1, "a_tool", json!({"_meta": {"progressToken": 7}})),
        ],
        Some(&["a_tool"]),
        backend.clone(),
        &EchoA,
    );
    assert_eq!(*backend.boots.lock().unwrap(), 0);
    assert_eq!(
        replies[1]["result"],
        json!({"content": [{"type": "text", "text": "{\"k\":1}"}],
               "structuredContent": {"k": 1}, "isError": false})
    );

    // Not exposed, not answered, or not plain: the backend's.
    let backend = Echo::default();
    let replies = run_with(
        &[
            init("2025-06-18"),
            call(1, "a_tool", json!({})),
            call(2, "b_tool", json!({})),
            call(3, "a_tool", json!({"task": "x"})),
            call(
                4,
                "a_tool",
                json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}),
            ),
        ],
        Some(&["b_tool"]),
        backend.clone(),
        &EchoA,
    );
    assert_eq!(*backend.boots.lock().unwrap(), 1);
    let echoed = replies
        .iter()
        .filter(|reply| reply["result"]["echo"] == "tools/call")
        .count();
    assert_eq!(echoed, 4, "{replies:?}");
}

#[test]
fn prompts_are_filled_in_from_the_recorded_replies() {
    let backend = Echo::default();
    let get = |id: i64, arguments: Value| {
        request(
            id,
            "prompts/get",
            json!({"name": "a_prompt", "arguments": arguments}),
        )
    };
    let replies = run(
        &[
            init("2025-06-18"),
            initialized(),
            get(1, json!({})),
            get(2, json!({"base": ""})),
            get(3, json!({"base": "main \"x\"", "undeclared": "ignored"})),
        ],
        None,
        backend.clone(),
    );
    assert_eq!(*backend.boots.lock().unwrap(), 0);
    let text = |index: usize| replies[index]["result"]["messages"][0]["content"]["text"].clone();
    assert_eq!(text(1), "on HEAD~1");
    assert_eq!(text(2), "on <base>");
    assert_eq!(text(3), "on main \"x\"");

    // An unknown prompt, a non-string argument, or `prompts/get` before
    // `initialize` is the backend's.
    let backend = Echo::default();
    let replies = run(
        &[
            request(9, "prompts/get", json!({"name": "a_prompt"})),
            init("2025-06-18"),
            request(1, "prompts/get", json!({"name": "nope"})),
            get(2, json!({"base": 3})),
        ],
        None,
        backend.clone(),
    );
    assert_eq!(*backend.boots.lock().unwrap(), 1);
    assert!(
        replies
            .iter()
            .filter(|reply| reply.get("result").and_then(|r| r.get("echo")).is_some())
            .count()
            >= 3,
        "{replies:?}"
    );
}

/// Answers every tool, and calls `a_tool` a graph writer.
struct Writer;

impl Native for Writer {
    fn call_tool(&self, name: &str, _arguments: &Value) -> Option<(String, Value)> {
        Some((format!("{{\"{name}\":1}}"), json!({ name: 1 })))
    }

    fn writes_graph(&self, name: &str) -> bool {
        name == "a_tool"
    }
}

#[test]
fn graph_writers_are_native_only_before_the_backend_runs() {
    let backend = Echo::default();
    let call =
        |id: i64, name: &str| request(id, "tools/call", json!({"name": name, "arguments": {}}));
    let replies = run_with(
        &[
            init("2025-06-18"),
            initialized(),
            call(1, "a_tool"),
            // Booting the backend: a request the front end does not answer.
            request(2, "resources/read", json!({"uri": "x"})),
            call(3, "a_tool"),
            call(4, "b_tool"),
        ],
        None,
        backend.clone(),
        &Writer,
    );
    assert_eq!(*backend.boots.lock().unwrap(), 1);
    let by_id = |id: i64| {
        replies
            .iter()
            .find(|reply| reply["id"] == json!(id))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(
        by_id(1)["result"]["structuredContent"],
        json!({"a_tool": 1})
    );
    assert_eq!(
        by_id(3)["result"],
        json!({"echo": "tools/call"}),
        "the writer goes to the backend"
    );
    assert_eq!(
        by_id(4)["result"]["structuredContent"],
        json!({"b_tool": 1})
    );
}

fn modern(id: i64, method: &str, params: Value) -> Value {
    let mut params = params;
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "t"},
    });
    request(id, method, params)
}

#[test]
fn a_modern_connection_is_answered_without_initialize() {
    let backend = Echo::default();
    let replies = run_with(
        &[
            modern(1, "server/discover", json!({})),
            modern(2, "tools/list", json!({})),
            modern(3, "resources/list", json!({})),
            modern(
                4,
                "tools/call",
                json!({"name": "a_tool", "arguments": {"x": 1}}),
            ),
        ],
        Some(&["a_tool"]),
        backend.clone(),
        &EchoA,
    );
    let stamp =
        json!({"io.modelcontextprotocol/serverInfo": {"name": "dagayn", "version": "7.1.1"}});
    assert_eq!(replies.len(), 4);
    for reply in &replies {
        assert_eq!(reply["result"]["_meta"], stamp, "{reply}");
    }
    assert_eq!(
        replies[0]["result"]["supportedVersions"],
        json!(["2026-07-28"])
    );
    assert_eq!(replies[1]["result"]["tools"], json!([{"name": "a_tool"}]));
    assert_eq!(replies[1]["result"]["resultType"], "complete");
    assert_eq!(replies[2]["result"]["resources"], json!([]));
    assert_eq!(replies[3]["result"]["structuredContent"], json!({"x": 1}));
    assert_eq!(replies[3]["result"]["resultType"], "complete");
    assert_eq!(*backend.boots.lock().unwrap(), 0);
}

#[test]
fn a_modern_connection_boots_the_backend_in_its_era() {
    let backend = Echo::default();
    let replies = run_with(
        &[
            modern(1, "server/discover", json!({})),
            // Declined natively, then an `initialize` the backend refuses.
            modern(2, "tools/call", json!({"name": "b_tool", "arguments": {}})),
            init("2025-06-18"),
            // Another revision, or a modern request after a handshake, is
            // the backend's to refuse.
            request(4, "tools/list", json!({})),
        ],
        Some(&["a_tool", "b_tool"]),
        backend.clone(),
        &EchoA,
    );
    let received = backend.received.lock().unwrap();
    assert_eq!(received[0]["id"], PROXY_INIT_ID);
    assert_eq!(received[0]["method"], "server/discover");
    assert_eq!(
        received[0]["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
        "2026-07-28"
    );
    let methods: Vec<&str> = received[1..]
        .iter()
        .map(|message| message["method"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["tools/call", "initialize", "tools/list"]);
    // The replayed discover's reply is not relayed.
    assert!(replies.iter().all(|reply| reply["id"] != PROXY_INIT_ID));
    assert_eq!(replies.len(), 4);
}

#[test]
fn an_unknown_revision_or_a_handshake_connection_delegates_modern_requests() {
    let backend = Echo::default();
    let mut other = modern(1, "tools/list", json!({}));
    other["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2099-01-01");
    let replies = run(&[other], None, backend.clone());
    assert_eq!(replies[0]["result"], json!({"echo": "tools/list"}));

    let backend = Echo::default();
    let replies = run(
        &[
            init("2025-06-18"),
            initialized(),
            modern(1, "tools/list", json!({})),
        ],
        None,
        backend.clone(),
    );
    assert_eq!(replies[1]["result"], json!({"echo": "tools/list"}));
}
