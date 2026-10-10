//! The `next` field: at most three calls an agent can make unchanged, each
//! with the reason to make it
//! (docs/plans/AGENT-WORKFLOW-TARGET.md#target-contract).

use dagayn_graph::GraphNode;
use serde_json::{Map, Value, json};

/// The most calls a `next` list names.
pub(crate) const MAX_NEXT: usize = 3;

/// One call: the tool, its complete arguments, and why to make it.
pub(crate) fn call(tool: &str, args: Value, why: impl Into<String>) -> Value {
    json!({"tool": tool, "args": args, "why": why.into()})
}

/// Production code before tests, each group in its own order: an agent
/// asking about a name usually means the code, not its test double.
pub(crate) fn tests_last<'a>(
    candidates: impl IntoIterator<Item = &'a GraphNode>,
) -> Vec<&'a GraphNode> {
    let mut ordered: Vec<&GraphNode> = candidates.into_iter().collect();
    ordered.sort_by_key(|node| node.is_test);
    ordered
}

/// The retries of an ambiguous `target`: the call as it was made, once per
/// candidate, `target` set to the candidate's qualified name. Of the other
/// arguments only those in `kept` stay, and only when they differ from the
/// default `kept` pairs them with, so the call reads the same whether a
/// client or the Python server (which fills in every default) made it.
pub(crate) fn retries(
    tool: &str,
    arguments: &Map<String, Value>,
    kept: &[(&str, Option<Value>)],
    candidates: &[&GraphNode],
) -> Value {
    let calls: Vec<Value> = candidates
        .iter()
        .take(MAX_NEXT)
        .map(|node| {
            // The names and paths the reply's `candidates` show.
            let shown = crate::query::node_dict(node);
            let mut args: Map<String, Value> = kept
                .iter()
                .filter_map(|(key, default)| {
                    let value = arguments.get(*key)?;
                    (default.as_ref() != Some(value)).then(|| (key.to_string(), value.clone()))
                })
                .collect();
            args.insert("target".into(), shown["qualified_name"].clone());
            let place = format!(
                "{}:{}",
                shown["file_path"].as_str().unwrap_or(&node.file_path),
                node.line_start
            );
            call(
                tool,
                Value::Object(args),
                format!("'{}' as the {} at {place}", node.name, node.kind),
            )
        })
        .collect();
    Value::Array(calls)
}

/// `source_of` (a file's summary for a file) for the first hits of a
/// search, in rank order: a hit is a lead until its live span is read. The
/// Python search builds the same list (`dagayn/tools/query.py::_read_hits`).
pub(crate) fn read_hits(results: &[Value]) -> Value {
    let calls: Vec<Value> = results
        .iter()
        .filter_map(|hit| {
            let target = hit.get("qualified_name")?.as_str()?;
            let kind = hit.get("kind").and_then(Value::as_str).unwrap_or("node");
            let read = if kind == "File" {
                "file_summary"
            } else {
                "source_of"
            };
            Some(call(
                "query_graph_tool",
                json!({"pattern": read, "target": target}),
                format!("read the {kind} this search ranked"),
            ))
        })
        .take(MAX_NEXT)
        .collect();
    Value::Array(calls)
}

/// What to read after a `query_graph_tool` answer: after a span, who calls
/// it; after a relationship, the first related nodes' spans (a file's
/// summary for a file), in the order the reply lists them.
pub(crate) fn after_query(pattern: &str, target: &str, rows: &[Value]) -> Value {
    if pattern == "source_of" {
        if rows.is_empty() {
            return json!([]);
        }
        return json!([call(
            "query_graph_tool",
            json!({"pattern": "callers_of", "target": target}),
            "who calls what you just read",
        )]);
    }
    let mut seen: Vec<&str> = Vec::new();
    let calls: Vec<Value> = rows
        .iter()
        .filter_map(|row| {
            let name = row.get("qualified_name")?.as_str()?;
            if name == target || seen.contains(&name) {
                return None;
            }
            seen.push(name);
            let kind = row.get("kind").and_then(Value::as_str).unwrap_or("node");
            let read = if kind == "File" {
                "file_summary"
            } else {
                "source_of"
            };
            Some(call(
                "query_graph_tool",
                json!({"pattern": read, "target": name}),
                format!("read the {kind} {pattern} found"),
            ))
        })
        .take(MAX_NEXT)
        .collect();
    Value::Array(calls)
}

/// Where to look first for the first findings, in their order: a test
/// command to run, a symbol's span, a cycle's imports, or a file's summary.
/// A finding names its own place; `next` only makes the first ones runnable.
pub(crate) fn from_findings(findings: &[Value]) -> Value {
    let calls: Vec<Value> = findings
        .iter()
        .filter_map(|finding| {
            let why = finding
                .get("claim")
                .or_else(|| finding.get("action"))
                .and_then(Value::as_str)
                .unwrap_or("check this finding");
            if let Some(command) = finding.get("command").and_then(Value::as_str) {
                return Some(call("shell", json!({"command": command}), why));
            }
            let symbol = finding
                .get("qualified_name")
                .and_then(Value::as_str)
                .or_else(|| {
                    finding["targets"]
                        .as_array()?
                        .first()?
                        .as_str()
                        .filter(|target| target.contains("::"))
                });
            let file = finding.get("file").and_then(Value::as_str);
            let (pattern, target) = match (symbol, file) {
                (Some(symbol), _) => ("source_of", symbol),
                (None, Some(file)) if finding["kind"] == "import_cycle" => ("imports_of", file),
                (None, Some(file)) => ("file_summary", file),
                (None, None) => return None,
            };
            Some(call(
                "query_graph_tool",
                json!({"pattern": pattern, "target": target}),
                why,
            ))
        })
        .take(MAX_NEXT)
        .collect();
    Value::Array(calls)
}

/// `source_of` for the first entry points, each read once.
pub(crate) fn read_entry_points(entries: &[Value]) -> Value {
    let mut seen: Vec<&str> = Vec::new();
    let calls: Vec<Value> = entries
        .iter()
        .filter_map(|entry| {
            let name = entry.get("entry_point")?.as_str()?;
            if seen.contains(&name) {
                return None;
            }
            seen.push(name);
            let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("entry");
            Some(call(
                "query_graph_tool",
                json!({"pattern": "source_of", "target": name}),
                format!("read the {kind} entry point"),
            ))
        })
        .take(MAX_NEXT)
        .collect();
    Value::Array(calls)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, file: &str, is_test: bool) -> GraphNode {
        GraphNode {
            id: 0,
            kind: "Function".into(),
            name: name.into(),
            qualified_name: format!("{file}::{name}"),
            file_path: file.into(),
            line_start: 3,
            line_end: 4,
            language: "python".into(),
            parent_name: None,
            params: None,
            return_type: None,
            is_test,
            file_hash: None,
            extra: Value::Null,
            signature: None,
        }
    }

    #[test]
    fn retries_name_each_candidate_tests_last() {
        let nodes = [
            node("helper", "tests/test_app.py", true),
            node("helper", "app.py", false),
            node("helper", "lib.py", false),
            node("helper", "more.py", false),
        ];
        let ordered = tests_last(&nodes);
        let arguments: Map<String, Value> = serde_json::from_value(json!({
            "pattern": "callers_of",
            "target": "helper",
            "depth": 2,
            "detail_level": "standard",
            "repo_root": "/r",
        }))
        .unwrap();
        let kept = [
            ("pattern", None),
            ("depth", Some(json!(1))),
            ("detail_level", Some(json!("standard"))),
        ];
        let next = retries("query_graph_tool", &arguments, &kept, &ordered);
        let calls = next.as_array().unwrap();
        assert_eq!(calls.len(), MAX_NEXT);
        assert_eq!(calls[0]["tool"], "query_graph_tool");
        assert_eq!(
            calls[0]["args"],
            json!({"pattern": "callers_of", "target": "app.py::helper", "depth": 2})
        );
        assert_eq!(calls[0]["why"], "'helper' as the Function at app.py:3");
        assert!(
            calls
                .iter()
                .all(|c| c["args"]["target"] != "tests/test_app.py::helper")
        );
    }
}
