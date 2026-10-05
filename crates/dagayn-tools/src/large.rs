//! `find_large_functions_tool` (`dagayn.tools.query.find_large_functions`).

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::query::node_dict;
use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo};

/// `Optional[str]` that only a JSON string or null satisfies.
fn optional_text<'a>(arguments: &'a Map<String, Value>, key: &str) -> Option<Option<&'a str>> {
    match arguments.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(value)),
        Some(_) => None,
    }
}

pub(crate) fn find_large_functions(
    context: &Context,
    arguments: &Map<String, Value>,
) -> Option<Payload> {
    let args = Args::new(
        arguments,
        &[
            "min_lines",
            "kind",
            "file_path_pattern",
            "limit",
            "repo_root",
        ],
    )?;
    let min_lines = args.integer("min_lines", 50)?;
    let limit = args.integer("limit", 50)?;
    let kind = optional_text(arguments, "kind")?;
    let pattern = optional_text(arguments, "file_path_pattern")?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let nodes = graph
        .store
        .get_nodes_by_size(min_lines, None, kind, pattern, limit)
        .ok()?;

    let results: Vec<Value> = nodes
        .iter()
        .map(|node| {
            let mut out = match node_dict(node) {
                Value::Object(map) => map,
                _ => Map::new(),
            };
            let line_count = if node.line_start != 0 && node.line_end != 0 {
                node.line_end - node.line_start + 1
            } else {
                0
            };
            out.insert("line_count".into(), json!(line_count));
            // Relative for readability, as `Path.relative_to` allows.
            let path = Path::new(&node.file_path);
            let relative = if path.is_absolute() {
                path.strip_prefix(&graph.root)
                    .map(|rel| rel.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| node.file_path.clone())
            } else {
                node.file_path.clone()
            };
            out.insert("relative_path".into(), json!(relative));
            // A File node's name is its absolute path.
            let absolute_name = out
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| Path::new(name).is_absolute());
            if node.kind == "File" && absolute_name {
                out.insert("name".into(), json!(relative));
            }
            Value::Object(out)
        })
        .collect();

    let mut header = format!("Found {} node(s) with >= {min_lines} lines", results.len());
    if let Some(kind) = kind.filter(|kind| !kind.is_empty()) {
        header.push_str(&format!(" (kind={kind})"));
    }
    if let Some(pattern) = pattern.filter(|pattern| !pattern.is_empty()) {
        header.push_str(&format!(" matching '{pattern}'"));
    }
    header.push(':');
    let mut lines = vec![header];
    let text = |item: &Value, key: &str| match item.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    for item in results.iter().take(10) {
        lines.push(format!(
            "  {:>4} lines | {:>8} | {} ({}:{})",
            text(item, "line_count"),
            text(item, "kind"),
            text(item, "name"),
            text(item, "relative_path"),
            text(item, "line_start"),
        ));
    }
    if results.len() > 10 {
        lines.push(format!("  ... and {} more", results.len() - 10));
    }
    Some(
        Ordered::default()
            .put("status", "ok")
            .put("summary", lines.join("\n"))
            .put("total_found", results.len())
            .put("min_lines", min_lines)
            .put("results", Value::Array(results))
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}
