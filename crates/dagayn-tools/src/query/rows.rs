//! Result rows and the edge and node dictionaries other tools share.

use std::collections::HashMap;

use dagayn_graph::{GraphEdge, GraphNode};
use serde_json::{Map, Value, json};

pub(crate) fn python_dumps_len(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(number) => number.to_string().len(),
        Value::String(text) => python_string_len(text),
        Value::Array(items) => {
            2 + items.iter().map(python_dumps_len).sum::<usize>()
                + 2 * items.len().saturating_sub(1)
        }
        Value::Object(map) => {
            2 + map
                .iter()
                .map(|(key, item)| python_string_len(key) + 2 + python_dumps_len(item))
                .sum::<usize>()
                + 2 * map.len().saturating_sub(1)
        }
    }
}

fn python_string_len(text: &str) -> usize {
    2 + text
        .chars()
        .map(|c| match c {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            ' '..='~' => 1,
            c if (c as u32) > 0xFFFF => 12,
            _ => 6,
        })
        .sum::<usize>()
}

/// `_QUERY_MINIMAL_FIELDS`, in order.
const MINIMAL_FIELDS: &[&str] = &[
    "name",
    "kind",
    "file_path",
    "qualified_name",
    "line_start",
    "line_end",
    "confidence",
    "coverage_source",
    "source",
    "target",
    "matched_endpoint",
    "relationship_role",
    "inverse_label",
    "evidence_type",
    "file",
    "truncated",
    "source_stale",
    "read_error",
    "omitted_chars",
    "omitted_lines",
    "signature",
    "span_line_start",
    "span_line_end",
    "importer",
    "import_target",
    "import_kind",
    "relation",
    "unresolved",
    "lines",
    "line",
    "match",
    "confidence_tier",
    "resolved_by",
    "depth",
    "via",
];

/// A `callers_of` row's `relation` where the row uses the target as a
/// value instead of calling it.
pub(super) const VALUE_REFERENCE: &str = "references";
/// The merged row's `relation` for a node that both calls the target and
/// uses it as a value.
const CALLS_AND_REFERENCES: &str = "calls,references";
/// `_sanitize_name`: control characters other than tab and newline dropped,
/// then at most 256 characters.
pub(crate) fn sanitize(name: &str) -> String {
    name.chars()
        .filter(|c| *c == '\t' || *c == '\n' || (*c as u32) >= 0x20)
        .take(256)
        .collect()
}

/// A result row in Python's key order.
pub(crate) type Row = Vec<(&'static str, Value)>;

pub(super) fn get<'a>(row: &'a Row, key: &str) -> Option<&'a Value> {
    row.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

fn set(row: &mut Row, key: &'static str, value: Value) {
    match row.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => row.push((key, value)),
    }
}

pub(super) fn object(row: Row) -> Value {
    Value::Object(row.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// `node_to_dict`.
pub(crate) fn node_row(node: &GraphNode) -> Row {
    vec![
        ("id", json!(node.id)),
        ("kind", json!(node.kind)),
        ("name", json!(sanitize(&node.name))),
        ("qualified_name", json!(sanitize(&node.qualified_name))),
        ("file_path", json!(node.file_path)),
        ("line_start", json!(node.line_start)),
        ("line_end", json!(node.line_end)),
        ("language", json!(node.language)),
        (
            "parent_name",
            json!(node.parent_name.as_deref().map(sanitize)),
        ),
        ("is_test", json!(node.is_test)),
    ]
}

pub(crate) fn node_dict(node: &GraphNode) -> Value {
    object(node_row(node))
}

/// What settled the edge's target (`"scip"`: a SCIP index), when something
/// other than the extractor did.
pub(super) fn resolved_by(edge: &GraphEdge) -> Option<&str> {
    edge.extra.get("resolved_by").and_then(Value::as_str)
}

/// `edge_to_dict`.
pub(crate) fn edge_dict(edge: &GraphEdge) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("id".into(), json!(edge.id));
    out.insert("kind".into(), json!(edge.kind));
    out.insert("source".into(), json!(sanitize(&edge.source_qualified)));
    out.insert("target".into(), json!(sanitize(&edge.target_qualified)));
    out.insert("file_path".into(), json!(edge.file_path));
    out.insert("line".into(), json!(edge.line));
    out.insert("confidence".into(), json!(edge.confidence));
    out.insert(
        "confidence_tier".into(),
        json!(edge.confidence_tier.as_str()),
    );
    if let Some(by) = resolved_by(edge) {
        out.insert("resolved_by".into(), json!(by));
    }
    if edge.kind == "CROSS_ARTIFACT"
        && let Some(extra) = edge.extra.as_object()
        && !extra.is_empty()
    {
        let kept: Map<String, Value> = [
            "relationship_role",
            "bridge_kind",
            "evidence_kind",
            "evidence_source",
            "source_language",
            "target_language",
            "confidence_tier",
        ]
        .iter()
        .filter_map(|key| {
            extra
                .get(*key)
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect();
        out.insert("extra".into(), Value::Object(kept));
    }
    out
}

/// `result_evidence_type`.
fn evidence_type(row: &Row) -> Value {
    if let Some(evidence) = get(row, "evidence_type")
        && evidence.as_str().is_some_and(|text| !text.is_empty())
    {
        return evidence.clone();
    }
    let kind = get(row, "kind").and_then(Value::as_str).unwrap_or("");
    let path = get(row, "file_path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .or_else(|| get(row, "file").and_then(Value::as_str))
        .unwrap_or("")
        .to_lowercase();
    let authored = kind.starts_with("Doc")
        || [".md", ".markdown", ".mdx"]
            .iter()
            .any(|ext| path.ends_with(ext));
    json!(if authored { "authored" } else { "extracted" })
}

/// `_compact_row`.
pub(super) fn compact(row: &Row, with_evidence: bool) -> Row {
    let mut out: Row = row
        .iter()
        .filter(|(key, value)| {
            let dropped = match *key {
                "id" | "language" => true,
                "parent_name" => value.is_null(),
                "is_test" => *value == Value::Bool(false),
                _ => false,
            };
            !dropped
        })
        .cloned()
        .collect();
    if with_evidence {
        set(&mut out, "evidence_type", evidence_type(row));
    }
    let file_path = get(&out, "file_path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let qualified = get(&out, "qualified_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !file_path.is_empty() && qualified.starts_with(&file_path) {
        out.retain(|(key, _)| *key != "file_path");
    }
    if get(&out, "importer").is_some() && get(&out, "importer") == get(&out, "file") {
        out.retain(|(key, _)| *key != "importer");
    }
    out
}

/// `_merge_rows`.
pub(super) fn merge(rows: Vec<Row>) -> Vec<Row> {
    let mut order: Vec<String> = Vec::new();
    let mut merged: HashMap<String, Row> = HashMap::new();
    for row in rows {
        let qualified = get(&row, "qualified_name")
            .and_then(Value::as_str)
            .unwrap_or("");
        let key = if qualified.is_empty() {
            format!(
                "{}|{}",
                get(&row, "importer").and_then(Value::as_str).unwrap_or(""),
                get(&row, "file").and_then(Value::as_str).unwrap_or("")
            )
        } else {
            qualified.to_string()
        };
        let line = get(&row, "line").cloned().filter(|line| !line.is_null());
        match merged.get_mut(&key) {
            None => {
                let mut first: Row = row.into_iter().filter(|(k, _)| *k != "line").collect();
                first.push(("lines", json!(line.into_iter().collect::<Vec<_>>())));
                order.push(key.clone());
                merged.insert(key, first);
            }
            Some(existing) => {
                if let Some(line) = line
                    && let Some((_, Value::Array(lines))) =
                        existing.iter_mut().find(|(k, _)| *k == "lines")
                    && !lines.contains(&line)
                {
                    lines.push(line);
                }
                // One file can link to a document and declare a directive on
                // it: the merged row names both kinds.
                if let Some(kind) = get(&row, "import_kind").and_then(Value::as_str) {
                    match existing.iter_mut().find(|(k, _)| *k == "import_kind") {
                        Some((_, Value::String(kinds))) => {
                            if !kinds.split(',').any(|known| known == kind) {
                                kinds.push(',');
                                kinds.push_str(kind);
                            }
                        }
                        _ => existing.push(("import_kind", json!(kind))),
                    }
                }
                // A function that calls the target and also stores it as a
                // value says both.
                if get(&row, "relation") != get(existing, "relation") {
                    set(existing, "relation", json!(CALLS_AND_REFERENCES));
                }
                // A merged row says what settled its calls only when every
                // call it merges was settled that way.
                if get(&row, "resolved_by") != get(existing, "resolved_by") {
                    existing.retain(|(k, _)| *k != "resolved_by");
                }
            }
        }
    }
    order
        .into_iter()
        .filter_map(|key| merged.remove(&key))
        .collect()
}

pub(super) fn project_minimal(row: Row) -> Value {
    let map: HashMap<&str, Value> = row.into_iter().collect();
    Value::Object(
        MINIMAL_FIELDS
            .iter()
            .filter_map(|field| map.get(field).map(|v| (field.to_string(), v.clone())))
            .collect(),
    )
}

/// The file a result row is in: its `file_path`, its `file` (an import
/// row), or the file part of its qualified name.
pub(super) fn row_file(row: &Row) -> Option<&str> {
    get(row, "file_path")
        .or_else(|| get(row, "file"))
        .and_then(Value::as_str)
        .or_else(|| {
            get(row, "qualified_name")
                .and_then(Value::as_str)
                .map(|name| name.split_once("::").map_or(name, |(file, _)| file))
        })
}

/// `directive` or `link` for a Markdown import edge, `lazy_export` for a
/// Python import a lazy-export table names (run at first attribute access,
/// not an import statement), `path` for a Rust file reached by a path in
/// code (`crate::a::B::f()`) and no `use`, nothing for an ordinary import
/// statement.
pub(crate) fn edge_import_kind(edge: &GraphEdge) -> Option<&str> {
    if let Some(kind) = edge
        .extra
        .get("markdown_import_kind")
        .and_then(Value::as_str)
    {
        return Some(kind);
    }
    if edge.extra.get("path_use") == Some(&Value::Bool(true)) {
        return Some("path");
    }
    (edge.extra.get("lazy_export") == Some(&Value::Bool(true))).then_some("lazy_export")
}

/// Whether an import edge passes the `import_kind` filter (no filter: all).
/// `statement` keeps the edges with no kind: ordinary import statements.
pub(crate) fn import_kind_matches(edge: &GraphEdge, filter: Option<&str>) -> bool {
    filter.is_none_or(|kind| match kind {
        "statement" => edge_import_kind(edge).is_none(),
        _ => edge_import_kind(edge) == Some(kind),
    })
}

pub(crate) fn cross_artifact_role(edge: &GraphEdge) -> Option<&str> {
    if edge.kind != "CROSS_ARTIFACT" {
        return None;
    }
    edge.extra.get("relationship_role").and_then(Value::as_str)
}
