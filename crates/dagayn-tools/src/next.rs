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

/// `_hints` that say what `next` says, for a reply whose generic hints
/// would point elsewhere.
pub(crate) fn as_hints(next: &Value) -> Value {
    let steps: Vec<Value> = next
        .as_array()
        .into_iter()
        .flatten()
        .map(|call| json!({"tool": call["tool"], "suggestion": call["why"]}))
        .collect();
    json!({"next_steps": steps, "related": [], "warnings": []})
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
