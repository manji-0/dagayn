//! `query_graph_tool` (`dagayn.tools.query.query_graph`), for
//! `callers_of` and `callees_of` at depth 1 on a node named exactly, at the
//! `standard` and `minimal` detail levels.
//!
//! Python's: every other pattern, `depth > 1`, `full`, a target resolved by
//! name search, external-package and builtin targets, `callers_of` with no
//! direct callers (the bare-name fallback), and an answer large enough for
//! `apply_output_budget` to trim.

use std::collections::{HashMap, HashSet};

use dagayn_build::{Vcs, detect_vcs};
use dagayn_graph::{GraphEdge, GraphNode};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::{Args, Context, Ordered, Payload, explicit_repo, open_graph};

/// `_BUILTIN_CALL_NAMES`: bare names `callers_of` skips.
const BUILTIN_CALL_NAMES: &[&str] = &[
    "map",
    "filter",
    "reduce",
    "reduceRight",
    "forEach",
    "find",
    "findIndex",
    "some",
    "every",
    "includes",
    "indexOf",
    "lastIndexOf",
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "slice",
    "concat",
    "join",
    "flat",
    "flatMap",
    "sort",
    "reverse",
    "fill",
    "keys",
    "values",
    "entries",
    "from",
    "isArray",
    "of",
    "at",
];

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
    "unresolved",
    "lines",
    "line",
    "match",
    "confidence_tier",
    "depth",
    "via",
];

/// `_sanitize_name`: control characters other than tab and newline dropped,
/// then at most 256 characters.
fn sanitize(name: &str) -> String {
    name.chars()
        .filter(|c| *c == '\t' || *c == '\n' || (*c as u32) >= 0x20)
        .take(256)
        .collect()
}

/// A row as `node_to_dict` plus the edge fields `_edge_rows` adds, already
/// compacted by `_compact_row` (ordered pairs; Python's dict order).
fn row(node: &GraphNode, edge: &GraphEdge, minimal: bool) -> Vec<(&'static str, Value)> {
    let qualified = sanitize(&node.qualified_name);
    let mut out: Vec<(&'static str, Value)> = vec![
        ("kind", json!(node.kind)),
        ("name", json!(sanitize(&node.name))),
        ("qualified_name", json!(qualified)),
    ];
    // Dropped when the qualified name already starts with it.
    if node.file_path.is_empty() || !qualified.starts_with(&node.file_path) {
        out.push(("file_path", json!(node.file_path)));
    }
    out.push(("line_start", json!(node.line_start)));
    out.push(("line_end", json!(node.line_end)));
    if let Some(parent) = &node.parent_name {
        out.push(("parent_name", json!(sanitize(parent))));
    }
    if node.is_test {
        out.push(("is_test", json!(true)));
    }
    out.push(("line", json!(edge.line)));
    if edge.target_qualified.contains("::") {
        out.push(("confidence_tier", json!(edge.confidence_tier.as_str())));
    } else {
        out.push(("confidence_tier", json!("MEDIUM")));
        out.push(("match", json!("bare_name")));
    }
    if minimal {
        // `result_evidence_type`.
        let markdown = ["md", "markdown", "mdx"]
            .iter()
            .any(|ext| node.file_path.to_lowercase().ends_with(&format!(".{ext}")));
        let evidence = if node.kind.starts_with("Doc") || markdown {
            "authored"
        } else {
            "extracted"
        };
        out.push(("evidence_type", json!(evidence)));
    }
    out
}

/// `_merge_rows`: one row per qualified name, its edge lines collected.
fn merge(rows: Vec<Vec<(&'static str, Value)>>) -> Vec<Vec<(&'static str, Value)>> {
    let mut order: Vec<String> = Vec::new();
    let mut merged: HashMap<String, Vec<(&'static str, Value)>> = HashMap::new();
    for row in rows {
        let key = row
            .iter()
            .find(|(field, _)| *field == "qualified_name")
            .and_then(|(_, value)| value.as_str())
            .unwrap_or_default()
            .to_string();
        let line = row
            .iter()
            .find(|(field, _)| *field == "line")
            .map(|(_, value)| value.clone())
            .filter(|value| !value.is_null());
        match merged.get_mut(&key) {
            None => {
                let mut first: Vec<(&'static str, Value)> = row
                    .into_iter()
                    .filter(|(field, _)| *field != "line")
                    .collect();
                first.push(("lines", json!(line.into_iter().collect::<Vec<_>>())));
                order.push(key.clone());
                merged.insert(key, first);
            }
            Some(existing) => {
                if let Some(line) = line
                    && let Some((_, Value::Array(lines))) =
                        existing.iter_mut().find(|(field, _)| *field == "lines")
                    && !lines.contains(&line)
                {
                    lines.push(line);
                }
            }
        }
    }
    order
        .into_iter()
        .filter_map(|key| merged.remove(&key))
        .collect()
}

fn object(pairs: Vec<(&'static str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// The length of Python's `json.dumps(value, default=str)`: `", "` and
/// `": "` separators, every character outside printable ASCII escaped.
fn python_dumps_len(value: &Value) -> usize {
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

/// `query_graph_guidance` for a result set, sealed as `GuidanceItem` dumps it.
fn guidance(pattern: &str, target: &str, result_count: usize, exact_count: i64) -> Value {
    if result_count == 0 {
        return json!([{
            "claim": format!("No graph relationships matched '{pattern}' for '{target}'."),
            "evidence": [{"type": "computed", "pattern": pattern, "target": target, "result_count": 0}],
            "confidence": "low",
            "missingness": [{
                "reason_code": "not_found_in_current_graph",
                "severity": "medium",
                "claim_effect": "absence is graph-limited, not proof the relationship does not exist",
            }],
            "action": "semantic_search_nodes_tool -- verify the target and refresh the graph",
            "reason_codes": ["zero_result"],
            "counts": {"result_count": 0},
        }]);
    }
    json!([{
        "claim": format!(
            "Graph query '{pattern}' returned {result_count} related node(s) for '{target}'."
        ),
        "evidence": [{
            "type": "computed",
            "pattern": pattern,
            "target": target,
            "result_count": result_count,
            "exact_match_count": exact_count,
        }],
        "confidence": "medium",
        "missingness": [{
            "reason_code": "relationship_query_not_runtime_proof",
            "severity": "low",
            "claim_effect": "graph edges are static extraction, not runtime traces",
        }],
        "action": format!("query_graph_tool pattern=\"{pattern}\" -- drill into a relationship"),
        "reason_codes": ["graph_relationship_query"],
        "counts": {"result_count": result_count},
    }])
}

pub(crate) fn query_graph(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(
        arguments,
        &["pattern", "target", "repo_root", "detail_level", "depth"],
    )?;
    let pattern = args.string("pattern")?;
    let target = args.string("target")?;
    let detail_level = match arguments.get("detail_level") {
        None => "standard",
        Some(Value::String(level)) => level.as_str(),
        Some(_) => return None,
    };
    let minimal = match detail_level {
        "standard" => false,
        "minimal" => true,
        _ => return None,
    };
    if args.integer("depth", 1)? != 1 {
        return None;
    }
    let callers = match pattern {
        "callers_of" => true,
        "callees_of" => false,
        _ => return None,
    };
    if callers && !target.contains("::") && BUILTIN_CALL_NAMES.contains(&target) {
        return None;
    }
    let root = explicit_repo(context, args.optional_string("repo_root")?)?;
    if !matches!(detect_vcs(&root), Vcs::Git | Vcs::None) {
        return None;
    }
    let graph = open_graph(&root)?;
    let store = &graph.store;

    // `resolve_query_target`, exact matches only.
    let node = match store.get_node(target).ok()? {
        Some(node) => node,
        None => store
            .get_node(&root.join(target).to_string_lossy())
            .ok()??,
    };
    let qualified = node.qualified_name.clone();
    let edges: Vec<GraphEdge> = if callers {
        store.get_edges_by_target(&qualified).ok()?
    } else {
        store.get_edges_by_source(&qualified).ok()?
    }
    .into_iter()
    .filter(|edge| edge.kind == "CALLS")
    .collect();
    if callers && edges.is_empty() {
        return None;
    }
    let endpoint = |edge: &GraphEdge| {
        if callers {
            edge.source_qualified.clone()
        } else {
            edge.target_qualified.clone()
        }
    };
    let names: Vec<String> = edges.iter().map(endpoint).collect();
    let nodes = store.get_nodes_by_qualified_names(&names).ok()?;
    let mut rows = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut seen_unresolved: HashSet<String> = HashSet::new();
    for edge in &edges {
        let name = endpoint(edge);
        match nodes.get(&name) {
            Some(endpoint_node) => rows.push(row(endpoint_node, edge, minimal)),
            None => {
                if seen_unresolved.insert(name.clone()) {
                    unresolved.push(name);
                }
            }
        }
    }
    let raw_count = rows.len();
    let rows = merge(rows);
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|pairs| {
            if minimal {
                let map: HashMap<&str, Value> = pairs.into_iter().collect();
                Value::Object(
                    MINIMAL_FIELDS
                        .iter()
                        .filter_map(|field| map.get(field).map(|v| (field.to_string(), v.clone())))
                        .collect(),
                )
            } else {
                object(pairs)
            }
        })
        .collect();

    // `query_zero_result_fields`.
    let (confidence, zero_result_reason) = if raw_count > 0 {
        let high = !edges.is_empty()
            && edges.iter().all(|edge| {
                matches!(
                    edge.confidence_tier.as_str(),
                    "EXACT" | "EXTRACTED" | "HIGH"
                )
            });
        (if high { "high" } else { "medium" }, Value::Null)
    } else if !unresolved.is_empty() {
        ("medium", json!("unresolved_endpoints_only"))
    } else {
        ("low", json!("not_found_in_current_graph"))
    };
    // `exactness_action` for an exact match.
    let next_action = json!({
        "tool": "query_graph_tool",
        "suggestion": "fetch live source with pattern=\"source_of\", then callers_of/callees_of",
    });

    let stats = store.get_stats().ok()?;
    // `graph_answerability_summary` without a freshness argument: the commit
    // tier of the root the graph records, none when it records none.
    let freshness = match store.get_metadata("repo_root").ok()? {
        Some(recorded) if !recorded.is_empty() => {
            dagayn_build::commit_tier_freshness(store, std::path::Path::new(&recorded)).ok()?
        }
        Some(_) => return None,
        None => None,
    };
    let answerability = Answerability::compute(store, &stats, freshness.as_ref());

    // `state.target` stays what the client sent when it named a node exactly.
    let summary = format!("Found {} result(s) for {pattern}('{target}')", rows.len());
    let mut payload = Ordered::default()
        .put("status", "ok")
        .put("pattern", pattern)
        .put("target", target)
        .put("unresolved_count", unresolved.len())
        .put("unresolved_targets", json!(unresolved))
        .put("confidence", confidence)
        .put("zero_result_reason", zero_result_reason)
        .put("next_action", next_action)
        .put("resolution", "exact")
        .put("exact_match_count", 1)
        .put("summary", summary)
        .put("result_count", rows.len())
        .put("answerability", answerability.compact())
        .put("results", json!(rows));
    if !minimal {
        payload = payload.put("guidance", guidance(pattern, target, raw_count, 1));
    }
    // `apply_output_budget`: an answer it would trim is Python's.
    let budget = if minimal { 4000 } else { 8000 };
    if python_dumps_len(&payload.value()) / 4 > budget {
        return None;
    }
    Some(
        payload
            .put("results_complete", true)
            .put("missingness", json!(answerability.missingness()))
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}
