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
            "tools": [{"name": "a_tool"}, {"name": "b_tool"}],
            "prompts": [{"name": "a_prompt"}],
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
