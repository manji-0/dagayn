//! `query_graph_tool` (`dagayn.tools.query.query_graph`), for
//! `callers_of`, `callees_of` (depth 1), and `source_of` on a node named or
//! found by name, at the `standard` and `minimal` detail levels.
//!
//! Python's: every other pattern, `depth > 1`, `full`, external-package and
//! builtin targets, and an answer large enough for `apply_output_budget` to
//! trim.

use std::collections::{HashMap, HashSet};

use dagayn_build::{Vcs, detect_vcs};
use dagayn_graph::{GraphEdge, GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::source::{SourceCoverage, source_row};
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
    let mut out = node_pairs(node);
    out.push(("line", json!(edge.line)));
    if edge.target_qualified.contains("::") {
        out.push(("confidence_tier", json!(edge.confidence_tier.as_str())));
    } else {
        out.push(("confidence_tier", json!("MEDIUM")));
        out.push(("match", json!("bare_name")));
    }
    if minimal {
        out.push(("evidence_type", json!(evidence_type(node))));
    }
    out
}

/// `node_to_dict` compacted by `_compact_row` (ordered pairs).
fn node_pairs(node: &GraphNode) -> Vec<(&'static str, Value)> {
    let qualified = sanitize(&node.qualified_name);
    let mut out: Vec<(&'static str, Value)> = vec![
        ("kind", json!(node.kind)),
        ("name", json!(sanitize(&node.name))),
        ("qualified_name", json!(qualified)),
    ];
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
    out
}

/// `result_evidence_type` for a row without one.
fn evidence_type(node: &GraphNode) -> &'static str {
    let lower = node.file_path.to_lowercase();
    if node.kind.starts_with("Doc")
        || [".md", ".markdown", ".mdx"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    {
        "authored"
    } else {
        "extracted"
    }
}

/// `source_of`: the live span of the node.
fn source_rows(node: &GraphNode, root: &std::path::Path, minimal: bool) -> Option<Found> {
    let (extra, coverage) = source_row(node, root)?;
    let mut pairs = node_pairs(node);
    pairs.extend(extra);
    if minimal {
        pairs.push(("evidence_type", json!(evidence_type(node))));
    }
    Some(Found {
        rows: vec![project(pairs, minimal)],
        raw_count: 1,
        edge_tiers: Vec::new(),
        unresolved: Vec::new(),
        source: Some(coverage),
    })
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
    if pattern == "source_of" {
        return json!([{
            "claim": format!("Live source span for '{target}'."),
            "evidence": [{
                "type": "computed",
                "pattern": pattern,
                "target": target,
                "result_count": result_count,
                "exact_match_count": exact_count,
            }],
            "confidence": "medium",
            "missingness": [{
                "reason_code": "live_source_span",
                "severity": "low",
                "claim_effect": "text is a worktree slice of the graph span; surrounding helpers and imports are omitted",
            }],
            "action": "query_graph_tool pattern=\"callers_of\" -- inspect callers after reading the span",
            "reason_codes": ["live_source_fetch"],
            "counts": {"result_count": result_count},
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
    let kind = match pattern {
        "callers_of" => Pattern::Callers,
        "callees_of" => Pattern::Callees,
        "source_of" => Pattern::Source,
        _ => return None,
    };
    if kind == Pattern::Callers && !target.contains("::") && BUILTIN_CALL_NAMES.contains(&target) {
        return None;
    }
    let root = explicit_repo(context, args.optional_string("repo_root")?)?;
    if !matches!(detect_vcs(&root), Vcs::Git | Vcs::None) {
        return None;
    }
    let graph = open_graph(&root)?;
    let store = &graph.store;

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

    // `resolve_query_target`.
    let exact = match store.get_node(target).ok()? {
        Some(node) => Some(node),
        None => store.get_node(&root.join(target).to_string_lossy()).ok()?,
    };
    let (node, resolution) = match exact {
        Some(node) => (node, Resolution::Exact),
        None => {
            let hits = store.search_nodes(target, NAME_SEARCH_LIMIT).ok()?;
            let named: Vec<&GraphNode> = hits.iter().filter(|hit| hit.name == target).collect();
            if kind == Pattern::Callers
                && named.is_empty()
                && is_external_package_target(store, target)?
            {
                // The callers of a package symbol, which has no node.
                return None;
            }
            if looks_like_file_target(target) {
                return Some(not_found(pattern, target, &answerability, &graph));
            }
            let candidates: Vec<&GraphNode> = if named.is_empty() {
                hits.iter().take(5).collect()
            } else {
                named.iter().copied().take(5).collect()
            };
            if named.len() == 1 {
                (named[0].clone(), Resolution::ExactName)
            } else if candidates.len() == 1 {
                (candidates[0].clone(), Resolution::Fuzzy)
            } else if candidates.len() > 1 {
                return Some(ambiguous(
                    pattern,
                    target,
                    &candidates,
                    &answerability,
                    &graph,
                ));
            } else {
                return Some(not_found(pattern, target, &answerability, &graph));
            }
        }
    };
    // A name-resolved target is reported by its qualified name.
    let original_target = target;
    let target: &str = if resolution == Resolution::Exact {
        target
    } else {
        &node.qualified_name
    };
    let exact_count = if resolution == Resolution::Fuzzy {
        0
    } else {
        1
    };
    let found = match kind {
        Pattern::Source => source_rows(&node, &root, minimal)?,
        Pattern::Callers | Pattern::Callees => edge_rows(store, &node, kind, minimal)?,
    };

    // `query_zero_result_fields`.
    let (confidence, zero_result_reason) = if found.raw_count > 0 {
        let high = !found.edge_tiers.is_empty()
            && found
                .edge_tiers
                .iter()
                .all(|tier| matches!(*tier, "EXACT" | "EXTRACTED" | "HIGH"));
        (if high { "high" } else { "medium" }, Value::Null)
    } else if !found.unresolved.is_empty() {
        ("medium", json!("unresolved_endpoints_only"))
    } else {
        ("low", json!("not_found_in_current_graph"))
    };
    // `exactness_action`.
    let next_action = if exact_count == 0 {
        if found.raw_count > 0 {
            json!({
                "tool": "query_graph_tool",
                "suggestion": "fetch live source with pattern=\"source_of\" for the chosen qualified_name",
            })
        } else {
            json!({
                "tool": "semantic_search_nodes_tool",
                "suggestion": "broaden the query or verify the graph is up to date",
            })
        }
    } else if kind == Pattern::Source {
        json!({
            "tool": "query_graph_tool",
            "suggestion": "inspect callers_of/callees_of after reading the live span; Read the file only for surrounding context or edits",
        })
    } else {
        json!({
            "tool": "query_graph_tool",
            "suggestion": "fetch live source with pattern=\"source_of\", then callers_of/callees_of",
        })
    };

    let rows = found.rows;
    let summary = format!("Found {} result(s) for {pattern}('{target}')", rows.len());
    let mut payload = Ordered::default()
        .put("status", "ok")
        .put("pattern", pattern)
        .put("target", target)
        .put("unresolved_count", found.unresolved.len())
        .put("unresolved_targets", json!(found.unresolved))
        .put("confidence", confidence)
        .put("zero_result_reason", zero_result_reason)
        .put("next_action", next_action)
        .put("resolution", resolution.as_str())
        .put("exact_match_count", exact_count);
    if resolution != Resolution::Exact {
        payload = payload
            .put("resolved_target", target)
            .put("original_target", original_target);
    }
    payload = payload
        .put("summary", summary)
        .put("result_count", rows.len())
        .put("answerability", answerability.compact())
        .put("results", json!(rows));
    if !minimal {
        payload = payload.put("guidance", guidance(pattern, target, found.raw_count, 1));
    }
    // `apply_output_budget`: an answer it would trim is Python's.
    let budget = if minimal { 4000 } else { 8000 };
    if python_dumps_len(&payload.value()) / 4 > budget {
        return None;
    }
    payload = payload.put("results_complete", true);
    let mut missingness = answerability.missingness();
    // `_attach_source_of_coverage`.
    if let Some(coverage) = found.source {
        missingness.extend(coverage.missingness());
        if coverage.read_error.is_some() || coverage.stale {
            payload = payload.replace("status", json!("degraded"));
        }
        payload = payload.put("source_coverage", coverage.value());
    }
    Some(
        payload
            .put("missingness", json!(missingness))
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}

/// `_NAME_RESOLUTION_SEARCH_LIMIT`.
const NAME_SEARCH_LIMIT: i64 = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Resolution {
    Exact,
    ExactName,
    Fuzzy,
}

impl Resolution {
    fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::ExactName => "exact_name",
            Self::Fuzzy => "fuzzy",
        }
    }
}

/// `looks_like_file_target` (`dagayn.bare_name_resolution`).
fn looks_like_file_target(target: &str) -> bool {
    let path = target.split("::").next().unwrap_or(target);
    if path.contains('/') || path.contains('\\') {
        return true;
    }
    let lower = path.to_lowercase();
    [
        ".md",
        ".markdown",
        ".py",
        ".tf",
        ".tfvars",
        ".rs",
        ".js",
        ".mjs",
        ".cjs",
        ".ts",
        ".mts",
        ".cts",
        ".tsx",
        ".jsx",
        ".java",
        ".go",
        ".rb",
        ".php",
        ".cs",
        ".cpp",
        ".hpp",
        ".c",
        ".h",
        ".swift",
        ".kt",
        ".scala",
        ".dart",
        ".ipynb",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

/// `is_external_package_target`: some edge into it is marked external.
fn is_external_package_target(store: &GraphStore, target: &str) -> Option<bool> {
    if target.is_empty() {
        return Some(false);
    }
    Some(
        store
            .get_edges_by_target(target)
            .ok()?
            .iter()
            .any(|edge| edge.extra.get("external") == Some(&Value::Bool(true))),
    )
}

/// `node_to_dict`, every field.
fn node_dict(node: &GraphNode) -> Value {
    json!({
        "id": node.id,
        "kind": node.kind,
        "name": sanitize(&node.name),
        "qualified_name": sanitize(&node.qualified_name),
        "file_path": node.file_path,
        "line_start": node.line_start,
        "line_end": node.line_end,
        "language": node.language,
        "parent_name": node.parent_name.as_deref().map(sanitize),
        "is_test": node.is_test,
    })
}

/// The early `not_found` answer of `resolve_query_target`.
fn not_found(
    pattern: &str,
    target: &str,
    answerability: &Answerability,
    graph: &crate::OpenGraph,
) -> Payload {
    let mut missingness = answerability.missingness();
    missingness.push(json!({
        "reason_code": "target_not_found_in_graph",
        "severity": "medium",
        "claim_effect": "absence is graph-limited, not proof the symbol does not exist",
    }));
    let suggestion = "semantic_search_nodes_tool -- verify the target and refresh the graph";
    Ordered::default()
        .put("status", "not_found")
        .put(
            "summary",
            format!("No node found matching '{target}' in the current graph."),
        )
        .put("result_count", 0)
        .put("results", json!([]))
        .put("zero_result_reason", "target_not_found_in_graph")
        .put(
            "next_action",
            json!({
                "tool": "semantic_search_nodes_tool",
                "suggestion": "broaden the query or verify the graph is up to date",
            }),
        )
        .put("answerability", answerability.full())
        .put("missingness", json!(missingness))
        .put("guidance", guidance(pattern, target, 0, 0))
        .put(
            "_hints",
            json!({
                "next_steps": [{"tool": "semantic_search_nodes_tool", "suggestion": suggestion}],
                "related": [],
                "warnings": ["not_found_in_current_graph"],
            }),
        )
        .put("_repo", graph.repo_context())
        .into_payload()
}

/// The early `ambiguous` answer of `resolve_query_target`.
fn ambiguous(
    pattern: &str,
    target: &str,
    candidates: &[&GraphNode],
    answerability: &Answerability,
    graph: &crate::OpenGraph,
) -> Payload {
    let mut missingness = answerability.missingness();
    missingness.push(json!({
        "reason_code": "ambiguous_target",
        "severity": "medium",
        "claim_effect": "relationship query was not run for a unique node",
    }));
    Ordered::default()
        .put("status", "ambiguous")
        .put("pattern", pattern)
        .put("target", target)
        .put(
            "summary",
            format!("Multiple matches for '{target}'. Please use a qualified name."),
        )
        .put("result_count", 0)
        .put("results", json!([]))
        .put(
            "candidates",
            json!(
                candidates
                    .iter()
                    .map(|node| node_dict(node))
                    .collect::<Vec<_>>()
            ),
        )
        .put("candidates_truncated", candidates.len() >= 5)
        .put("answerability", answerability.full())
        .put("missingness", json!(missingness))
        .put("_repo", graph.repo_context())
        .into_payload()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Callers,
    Callees,
    Source,
}

/// What a pattern found: the response rows, the row count before merging,
/// the tiers of the edges it followed, unresolved endpoints, and for
/// `source_of` the coverage of the live span.
struct Found {
    rows: Vec<Value>,
    raw_count: usize,
    edge_tiers: Vec<&'static str>,
    unresolved: Vec<String>,
    source: Option<SourceCoverage>,
}

fn project(pairs: Vec<(&'static str, Value)>, minimal: bool) -> Value {
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
}

/// `callers_of` / `callees_of`: one row per related node, edge lines merged.
fn edge_rows(store: &GraphStore, node: &GraphNode, kind: Pattern, minimal: bool) -> Option<Found> {
    let callers = kind == Pattern::Callers;
    let mut edges: Vec<GraphEdge> = if callers {
        store.get_edges_by_target(&node.qualified_name).ok()?
    } else {
        store.get_edges_by_source(&node.qualified_name).ok()?
    }
    .into_iter()
    .filter(|edge| edge.kind == "CALLS")
    .collect();
    // No direct callers: the calls that name it bare and can mean it.
    let fallback = callers && edges.is_empty();
    if fallback {
        edges = store.bare_name_callers(node).ok()?;
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
    Some(Found {
        rows: merge(rows)
            .into_iter()
            .map(|pairs| project(pairs, minimal))
            .collect(),
        raw_count,
        // `annotate_bare_name_edges`, on the fallback's edges only: a
        // bare-named edge counts as MEDIUM.
        edge_tiers: edges
            .iter()
            .map(|edge| {
                if !fallback || edge.target_qualified.contains("::") {
                    edge.confidence_tier.as_str()
                } else {
                    "MEDIUM"
                }
            })
            .collect(),
        unresolved,
        source: None,
    })
}
