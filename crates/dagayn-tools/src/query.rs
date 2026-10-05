//! `query_graph_tool` (`dagayn.tools.query.query_graph`): every pattern
//! (`tests_for` through [`crate::coverage`]), at every detail level and
//! depth, on a node named exactly or found by name.
//!
//! The Python tool opens the graph and leaves every answer to this.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use dagayn_graph::{GraphEdge, GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::pypath::{pure_join, realpath};
use crate::source::{SourceCoverage, source_row};
use crate::{Args, Context, OpenGraph, Ordered, Payload, open_graph, resolve_repo};

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

/// `QUERY_PATTERNS`.
/// `QUERY_PATTERNS`' keys, in Python's order.
const PATTERNS: [&str; 12] = [
    "callers_of",
    "callees_of",
    "imports_of",
    "importers_of",
    "docs_for",
    "implementations_of",
    "bridges_from",
    "children_of",
    "tests_for",
    "inheritors_of",
    "file_summary",
    "source_of",
];

/// The error `query_graph` answers for an unknown pattern or a depth it
/// does not apply, checked in Python's order.
fn argument_error(pattern: &str, depth: i64) -> Option<String> {
    if description(pattern).is_none() {
        let listed: Vec<String> = PATTERNS.iter().map(|name| format!("'{name}'")).collect();
        return Some(format!(
            "Unknown pattern '{pattern}'. Available: [{}]",
            listed.join(", ")
        ));
    }
    if depth != 1 && !matches!(pattern, "callers_of" | "importers_of") {
        return Some(format!(
            "depth applies only to ['callers_of', 'importers_of']; '{pattern}' returns direct \
             relationships only."
        ));
    }
    (depth < 1).then(|| format!("depth must be 1 or more, got {depth}."))
}

fn description(pattern: &str) -> Option<&'static str> {
    Some(match pattern {
        "callers_of" => "Find all functions that call a given function",
        "callees_of" => "Find all functions called by a given function",
        "imports_of" => "Find all imports of a given file or module",
        "importers_of" => "Find all files that import a given file or module",
        "docs_for" => "Find documentation linked to a code, Terraform, or artifact node",
        "implementations_of" => "Find implementation artifacts linked to a document node",
        "bridges_from" => {
            "Find high-confidence CROSS_ARTIFACT bridges from a node (Terraform maps_entrypoint / invokes_binary, and similar)"
        }
        "children_of" => "Find all nodes contained in a file or class",
        "tests_for" => "Find all tests for a given function or class",
        "inheritors_of" => "Find all classes that inherit from a given class",
        "file_summary" => "Get a summary of all nodes in a file",
        "source_of" => {
            "Fetch the live worktree source span for one node (function, class, section, or file)"
        }
        _ => return None,
    })
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
    "unresolved",
    "lines",
    "line",
    "match",
    "confidence_tier",
    "depth",
    "via",
];

/// Patterns whose rows are one per edge and merge by endpoint.
const MERGED: &[&str] = &["callers_of", "callees_of", "inheritors_of", "importers_of"];
/// `MAX_QUERY_DEPTH` and `_TRANSITIVE_ROW_LIMIT`.
const MAX_DEPTH: i64 = 6;
const TRANSITIVE_ROW_LIMIT: usize = 500;
/// `_NAME_RESOLUTION_SEARCH_LIMIT`.
const NAME_SEARCH_LIMIT: i64 = 200;

const DOC_TO_ARTIFACT: &[(&str, &str)] = &[
    ("implemented_by", "implements_contract"),
    ("describes_symbol", "described_by"),
    ("discusses_artifact", "discussed_by"),
    ("raises_issue_for", "has_issue_note"),
];
const ARTIFACT_TO_DOC: &[(&str, &str)] = &[
    ("implements_contract", "implemented_by"),
    ("explained_by", "explains"),
    ("has_runbook", "runbook_for"),
    ("problem_described_by", "describes_problem_in"),
    ("discussed_by", "discusses"),
];
const INFRA_TO_CODE: &[(&str, &str)] = &[
    ("maps_entrypoint", "entrypoint_for"),
    ("invokes_binary", "invoked_by"),
];

fn lookup(table: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

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

fn get<'a>(row: &'a Row, key: &str) -> Option<&'a Value> {
    row.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

fn set(row: &mut Row, key: &'static str, value: Value) {
    match row.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => row.push((key, value)),
    }
}

fn object(row: Row) -> Value {
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

/// `annotate_bare_name_edges`.
fn annotate_bare(edges: &mut [Map<String, Value>]) {
    for edge in edges {
        let bare = !edge
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or("")
            .contains("::");
        if bare {
            edge.insert("match".into(), json!("bare_name"));
            edge.insert("confidence_tier".into(), json!("MEDIUM"));
            edge.insert("confidence".into(), json!(0.6));
        }
    }
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
fn compact(row: &Row, with_evidence: bool) -> Row {
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
fn merge(rows: Vec<Row>) -> Vec<Row> {
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
            }
        }
    }
    order
        .into_iter()
        .filter_map(|key| merged.remove(&key))
        .collect()
}

fn project_minimal(row: Row) -> Value {
    let map: HashMap<&str, Value> = row.into_iter().collect();
    Value::Object(
        MINIMAL_FIELDS
            .iter()
            .filter_map(|field| map.get(field).map(|v| (field.to_string(), v.clone())))
            .collect(),
    )
}

/// `guidance_actions_to_hints` for the items `guidance` builds.
fn hints(guidance: &Value) -> Value {
    let mut next_steps = Vec::new();
    let mut warnings = Vec::new();
    for item in guidance.as_array().into_iter().flatten() {
        let action = item.get("action").and_then(Value::as_str).unwrap_or("");
        if action.is_empty() {
            continue;
        }
        let head = action.split(" -- ").next().unwrap_or(action);
        let tool = head.split(' ').next().unwrap_or(head);
        let tool = tool.split('(').next().unwrap_or(tool);
        next_steps.push(
            json!({"tool": if head.is_empty() { "manual" } else { tool }, "suggestion": action}),
        );
        for missing in item
            .get("missingness")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let severity = missing
                .get("severity")
                .and_then(Value::as_str)
                .unwrap_or("info");
            if let Some(code) = missing.get("reason_code").and_then(Value::as_str)
                && matches!(severity, "medium" | "high")
            {
                warnings.push(code.to_string());
            }
        }
        if next_steps.len() >= 3 {
            break;
        }
    }
    json!({"next_steps": next_steps, "related": [], "warnings": warnings})
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Resolution {
    Exact,
    ExactName,
    Fuzzy,
    ExternalPackage,
}

impl Resolution {
    fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::ExactName => "exact_name",
            Self::Fuzzy => "fuzzy",
            Self::ExternalPackage => "external_package",
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

fn is_external_edge(edge: &GraphEdge) -> bool {
    edge.extra.get("external") == Some(&Value::Bool(true))
}

/// `is_external_package_target`.
fn is_external_package_target(store: &GraphStore, target: &str) -> Option<bool> {
    if target.is_empty() {
        return Some(false);
    }
    Some(
        store
            .get_edges_by_target(target)
            .ok()?
            .iter()
            .any(is_external_edge),
    )
}

/// `str(root / target)`.
fn joined(root: &Path, target: &str) -> Option<String> {
    Some(pure_join(root.to_str()?, target))
}

/// `file_path_candidates`: the joined path, its non-strict `resolve()`, and
/// an absolute target as given. `None` only where Python raises (a NUL).
fn file_path_candidates(root: &Path, target: &str) -> Option<Vec<String>> {
    let joined = joined(root, target)?;
    let resolved = realpath(&joined)?;
    let mut out: Vec<String> = Vec::new();
    for candidate in [
        Some(joined),
        Some(resolved),
        target.starts_with('/').then(|| target.to_string()),
    ]
    .into_iter()
    .flatten()
    {
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    Some(out)
}

fn file_is_indexed(store: &GraphStore, candidates: &[String]) -> Option<bool> {
    for path in candidates {
        if !store.get_nodes_by_file(path).ok()?.is_empty() || store.get_node(path).ok()?.is_some() {
            return Some(true);
        }
    }
    Some(false)
}

/// `is_unresolved_import_target`.
fn is_unresolved_import(store: &GraphStore, root: &Path, target: &str) -> Option<bool> {
    if target.starts_with("<unresolved:") {
        return Some(true);
    }
    let resolved = realpath(&joined(root, target)?)?;
    Some(store.get_nodes_by_file(&resolved).ok()?.is_empty())
}

/// What a pattern found, in Python's terms: `state.results`, `edges_out`,
/// `unresolved_targets`, `reachability`, and for `source_of` the coverage.
#[derive(Default)]
struct Found {
    rows: Vec<Row>,
    edges: Vec<Map<String, Value>>,
    unresolved: Vec<String>,
    reachability: Option<Value>,
    source: Option<SourceCoverage>,
}

impl Found {
    fn unresolve(&mut self, names: Vec<String>) {
        for name in names {
            if !self.unresolved.contains(&name) {
                self.unresolved.push(name);
            }
        }
    }

    /// `_edge_rows`: the endpoint node of each edge, with its line and tier.
    fn edge_rows(
        &mut self,
        store: &GraphStore,
        edges: &[GraphEdge],
        source: bool,
    ) -> Option<Vec<Row>> {
        let endpoint = |edge: &GraphEdge| {
            if source {
                edge.source_qualified.clone()
            } else {
                edge.target_qualified.clone()
            }
        };
        if edges.is_empty() {
            return Some(Vec::new());
        }
        let names: Vec<String> = edges.iter().map(endpoint).collect();
        let nodes = store.get_nodes_by_qualified_names(&names).ok()?;
        let mut rows = Vec::new();
        let mut unresolved = Vec::new();
        for edge in edges {
            let Some(node) = nodes.get(&endpoint(edge)) else {
                unresolved.push(endpoint(edge));
                continue;
            };
            let mut row = node_row(node);
            row.push(("line", json!(edge.line)));
            if edge.target_qualified.contains("::") {
                row.push(("confidence_tier", json!(edge.confidence_tier.as_str())));
            } else {
                row.push(("confidence_tier", json!("MEDIUM")));
                row.push(("match", json!("bare_name")));
            }
            rows.push(row);
        }
        self.unresolve(unresolved);
        Some(rows)
    }
}

fn importer_row(edge: &GraphEdge) -> Row {
    vec![
        ("importer", json!(edge.source_qualified)),
        ("file", json!(edge.file_path)),
        ("line", json!(edge.line)),
        ("confidence_tier", json!(edge.confidence_tier.as_str())),
    ]
}

pub(crate) fn cross_artifact_role(edge: &GraphEdge) -> Option<&str> {
    if edge.kind != "CROSS_ARTIFACT" {
        return None;
    }
    edge.extra.get("relationship_role").and_then(Value::as_str)
}

/// `is_low_confidence_unresolved_markdown_code_span`.
fn is_noisy_code_span(edge: &GraphEdge) -> bool {
    edge.kind == "CROSS_ARTIFACT"
        && cross_artifact_role(edge) == Some("describes_symbol")
        && edge.target_qualified.starts_with("<unresolved:")
        && edge.confidence_tier.as_str() == "LOW"
        && matches!(
            edge.extra
                .get("evidence_kind")
                .and_then(Value::as_str)
                .unwrap_or(""),
            "markdown_code_span" | ""
        )
}

/// `documentation_result`.
fn documentation_row(edge: &GraphEdge, endpoint: &str, inverse_label: Option<&str>) -> Row {
    let role = cross_artifact_role(edge);
    let tier = edge.confidence_tier.as_str();
    let authored = matches!(role, Some("implements_contract" | "implemented_by"));
    let evidence = if authored {
        "authored"
    } else if matches!(tier, "EXTRACTED" | "HIGH") {
        "extracted"
    } else {
        "heuristic_reachable"
    };
    let mut row: Row = vec![
        ("source", json!(edge.source_qualified)),
        ("target", json!(edge.target_qualified)),
        ("matched_endpoint", json!(endpoint)),
        ("relationship_role", json!(role)),
        ("evidence_type", json!(evidence)),
        ("file", json!(edge.file_path)),
        ("line", json!(edge.line)),
        ("confidence", json!(edge.confidence)),
        ("confidence_tier", json!(tier)),
    ];
    if let Some(label) = inverse_label {
        row.push(("inverse_label", json!(label)));
    }
    row
}

/// `_expand_transitive` for `callers_of` (`importers = false`) and
/// `importers_of`.
fn expand_transitive(
    store: &GraphStore,
    found: &mut Found,
    start: &str,
    depth: i64,
    importers: bool,
) -> Option<()> {
    for row in &mut found.rows {
        row.push(("depth", json!(1)));
    }
    let first_hop: Vec<String> = found
        .rows
        .iter()
        .map(|row| {
            let key = if importers { "file" } else { "qualified_name" };
            get(row, key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        })
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(start.to_string());
    seen.extend(first_hop.iter().cloned());
    let mut frontier: Vec<String> = Vec::new();
    for key in first_hop {
        if !frontier.contains(&key) {
            frontier.push(key);
        }
    }
    let kind = if importers { "IMPORTS_FROM" } else { "CALLS" };
    let next_key = |edge: &GraphEdge| {
        if importers {
            edge.file_path.clone()
        } else {
            edge.source_qualified.clone()
        }
    };
    let mut hop = 2;
    let mut added = 0;
    let mut truncated = false;
    while !frontier.is_empty() && hop <= depth && !truncated {
        let (_, incoming) = store.get_edges_by_endpoints(&frontier).ok()?;
        let mut layer: Vec<GraphEdge> = Vec::new();
        for via in &frontier {
            for edge in incoming.get(via).into_iter().flatten() {
                let key = next_key(edge);
                if edge.kind != kind || seen.contains(&key) {
                    continue;
                }
                seen.insert(key);
                layer.push(edge.clone());
            }
        }
        if added + layer.len() > TRANSITIVE_ROW_LIMIT {
            layer.truncate(TRANSITIVE_ROW_LIMIT - added);
            truncated = true;
        }
        let mut rows = if importers {
            layer.iter().map(importer_row).collect()
        } else {
            found.edge_rows(store, &layer, true)?
        };
        let vias: HashMap<String, String> = layer
            .iter()
            .map(|edge| (next_key(edge), edge.target_qualified.clone()))
            .collect();
        for row in &mut rows {
            row.push(("depth", json!(hop)));
            let key = get(row, "qualified_name")
                .and_then(Value::as_str)
                .filter(|q| !q.is_empty())
                .or_else(|| get(row, "file").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            row.push(("via", json!(vias.get(&key).cloned().unwrap_or_default())));
        }
        found.rows.extend(rows);
        found.edges.extend(layer.iter().map(edge_dict));
        added += layer.len();
        frontier = layer.iter().map(next_key).collect();
        hop += 1;
    }
    found.reachability = Some(json!({
        "state": if truncated { "truncated" } else { "complete" },
        "truncated": truncated,
        "max_depth": depth,
        "nodes_visited": seen.len() - 1,
        "depth_limit_reached": !frontier.is_empty() && !truncated,
    }));
    Some(())
}

/// Run `pattern` for `node` (or `target` where the pattern works without one).
fn run_pattern(
    store: &GraphStore,
    root: &Path,
    pattern: &str,
    target: &str,
    node: Option<&GraphNode>,
    depth: i64,
) -> Option<Found> {
    let mut found = Found::default();
    let qualified = node.map_or(target, |node| node.qualified_name.as_str());
    let edges_of = |source: bool| -> Option<Vec<GraphEdge>> {
        if source {
            store.get_edges_by_source(qualified).ok()
        } else {
            store.get_edges_by_target(qualified).ok()
        }
    };
    match pattern {
        "callers_of" => {
            let edges: Vec<GraphEdge> = edges_of(false)?
                .into_iter()
                .filter(|e| e.kind == "CALLS")
                .collect();
            found.rows = found.edge_rows(store, &edges, true)?;
            found.edges = edges.iter().map(edge_dict).collect();
            if found.rows.is_empty()
                && let Some(node) = node
            {
                let fallback = store.bare_name_edges(node, "CALLS").ok()?;
                let rows = found.edge_rows(store, &fallback, true)?;
                found.rows.extend(rows);
                found.edges.extend(fallback.iter().map(edge_dict));
                annotate_bare(&mut found.edges);
            }
            if depth > 1 {
                expand_transitive(store, &mut found, qualified, depth, false)?;
            }
        }
        "callees_of" => {
            let edges: Vec<GraphEdge> = edges_of(true)?
                .into_iter()
                .filter(|e| e.kind == "CALLS")
                .collect();
            found.rows = found.edge_rows(store, &edges, false)?;
            found.edges = edges.iter().map(edge_dict).collect();
        }
        "imports_of" => {
            for edge in edges_of(true)? {
                if edge.kind != "IMPORTS_FROM" {
                    continue;
                }
                let unresolved = is_unresolved_import(store, root, &edge.target_qualified)?;
                found.rows.push(vec![
                    ("import_target", json!(edge.target_qualified)),
                    ("line", json!(edge.line)),
                    ("unresolved", json!(unresolved)),
                ]);
                found.edges.push(edge_dict(&edge));
                if unresolved {
                    found.unresolve(vec![edge.target_qualified.clone()]);
                }
            }
        }
        "importers_of" => {
            let file = node?.file_path.clone();
            for edge in store.get_edges_by_target(&file).ok()? {
                if edge.kind == "IMPORTS_FROM" {
                    found.rows.push(importer_row(&edge));
                    found.edges.push(edge_dict(&edge));
                }
            }
            if depth > 1 {
                expand_transitive(store, &mut found, qualified, depth, true)?;
            }
        }
        "docs_for" | "implementations_of" => {
            let docs = pattern == "docs_for";
            for edge in edges_of(true)? {
                if is_noisy_code_span(&edge) {
                    continue;
                }
                let role = cross_artifact_role(&edge).unwrap_or("");
                let label = if docs {
                    lookup(ARTIFACT_TO_DOC, role).map(Some)
                } else {
                    (role == "implemented_by").then_some(None)
                };
                if let Some(label) = label {
                    found
                        .rows
                        .push(documentation_row(&edge, &edge.target_qualified, label));
                    found.edges.push(edge_dict(&edge));
                }
            }
            for edge in edges_of(false)? {
                if is_noisy_code_span(&edge) {
                    continue;
                }
                let role = cross_artifact_role(&edge).unwrap_or("");
                let label = if docs {
                    lookup(DOC_TO_ARTIFACT, role)
                } else {
                    (role == "implements_contract").then_some("implemented_by")
                };
                if let Some(label) = label {
                    found.rows.push(documentation_row(
                        &edge,
                        &edge.source_qualified,
                        Some(label),
                    ));
                    found.edges.push(edge_dict(&edge));
                }
            }
        }
        "bridges_from" => {
            for edge in edges_of(true)? {
                if edge.kind != "CROSS_ARTIFACT" || is_noisy_code_span(&edge) {
                    continue;
                }
                let Some(label) = cross_artifact_role(&edge).and_then(|r| lookup(INFRA_TO_CODE, r))
                else {
                    continue;
                };
                if !matches!(
                    edge.confidence_tier.as_str(),
                    "EXACT" | "HIGH" | "EXTRACTED"
                ) {
                    continue;
                }
                found.rows.push(documentation_row(
                    &edge,
                    &edge.target_qualified,
                    Some(label),
                ));
                found.edges.push(edge_dict(&edge));
            }
        }
        "tests_for" => {
            let Some(node) = node else {
                return Some(found);
            };
            let mut state = crate::coverage::ScanState::build(store)?;
            found.rows =
                crate::coverage::infer_tests_for_node(store, &mut state, node, 25, "medium")?;
            found.edges = edges_of(true)?
                .iter()
                .filter(|e| e.kind == "TESTED_BY")
                .map(edge_dict)
                .collect();
        }
        "children_of" => {
            let edges: Vec<GraphEdge> = edges_of(true)?
                .into_iter()
                .filter(|e| e.kind == "CONTAINS")
                .collect();
            if !edges.is_empty() {
                let names: Vec<String> = edges.iter().map(|e| e.target_qualified.clone()).collect();
                let nodes = store.get_nodes_by_qualified_names(&names).ok()?;
                let mut unresolved = Vec::new();
                for name in names {
                    match nodes.get(&name) {
                        Some(child) => found.rows.push(node_row(child)),
                        None => {
                            if !unresolved.contains(&name) {
                                unresolved.push(name);
                            }
                        }
                    }
                }
                found.unresolve(unresolved);
            }
        }
        "inheritors_of" => {
            let edges: Vec<GraphEdge> = edges_of(false)?
                .into_iter()
                .filter(|e| matches!(e.kind.as_str(), "INHERITS" | "IMPLEMENTS"))
                .collect();
            found.rows = found.edge_rows(store, &edges, true)?;
            found.edges = edges.iter().map(edge_dict).collect();
            if found.rows.is_empty()
                && let Some(node) = node
            {
                let mut fallback = store.bare_name_edges(node, "INHERITS").ok()?;
                fallback.extend(store.bare_name_edges(node, "IMPLEMENTS").ok()?);
                let rows = found.edge_rows(store, &fallback, true)?;
                found.rows.extend(rows);
                found.edges.extend(fallback.iter().map(edge_dict));
                annotate_bare(&mut found.edges);
            }
        }
        "file_summary" => {
            for path in file_path_candidates(root, target)? {
                let nodes = store.get_nodes_by_file(&path).ok()?;
                if !nodes.is_empty() {
                    found.rows = nodes.iter().map(node_row).collect();
                    break;
                }
            }
        }
        "source_of" => {
            // `_pattern_source_of` returns nothing without a node.
            if let Some(node) = node {
                let (extra, coverage) = source_row(node, root)?;
                let mut row = node_row(node);
                row.extend(extra);
                found.rows.push(row);
                found.source = Some(coverage);
            }
        }
        _ => return None,
    }
    Some(found)
}

/// The early `not_found` answer of `resolve_query_target`; the
/// `file_summary` form names the pattern and speaks of a file.
fn not_found(
    pattern: &str,
    target: &str,
    file: bool,
    answerability: &Answerability,
    graph: &OpenGraph,
) -> Payload {
    let mut missingness = answerability.missingness();
    missingness.push(json!({
        "reason_code": "target_not_found_in_graph",
        "severity": "medium",
        "claim_effect": if file {
            "absence is graph-limited, not proof the file does not exist"
        } else {
            "absence is graph-limited, not proof the symbol does not exist"
        },
    }));
    let guidance = guidance(pattern, target, 0, 0);
    let mut payload = Ordered::default().put("status", "not_found");
    if file {
        payload = payload
            .put("pattern", pattern)
            .put("target", target)
            .put("description", description(pattern).unwrap_or(""))
            .put(
                "summary",
                format!("No indexed file found matching '{target}' in the current graph."),
            );
    } else {
        payload = payload.put(
            "summary",
            format!("No node found matching '{target}' in the current graph."),
        );
    }
    payload
        .put("result_count", 0)
        .put("results", json!([]))
        .put("zero_result_reason", "target_not_found_in_graph")
        .put("next_action", exactness_action(pattern, 0, 0))
        .put("answerability", answerability.full())
        .put("missingness", json!(missingness))
        .put("guidance", guidance.clone())
        .put("_hints", hints(&guidance))
        .put("_repo", graph.repo_context())
        .into_payload()
}

/// The early `ambiguous` answer of `resolve_query_target`.
fn ambiguous(
    pattern: &str,
    target: &str,
    candidates: &[&GraphNode],
    answerability: &Answerability,
    graph: &OpenGraph,
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

/// `exactness_action` (with the pattern, as `query_graph` passes it).
fn exactness_action(pattern: &str, exact_count: i64, result_count: usize) -> Value {
    if exact_count == 1 {
        if pattern == "source_of" {
            return json!({"tool": "query_graph_tool", "suggestion": "inspect callers_of/callees_of after reading the live span; Read the file only for surrounding context or edits"});
        }
        return json!({"tool": "query_graph_tool", "suggestion": "fetch live source with pattern=\"source_of\", then callers_of/callees_of"});
    }
    if result_count > 0 {
        return json!({"tool": "query_graph_tool", "suggestion": "fetch live source with pattern=\"source_of\" for the chosen qualified_name"});
    }
    json!({"tool": "semantic_search_nodes_tool", "suggestion": "broaden the query or verify the graph is up to date"})
}

/// `_transitive_next_action`.
fn transitive_next_action(reachability: &Value, results_complete: bool) -> Value {
    if reachability.get("truncated") == Some(&Value::Bool(true)) || !results_complete {
        return json!({"tool": "query_graph_tool", "suggestion": "the reachable set was cut off; lower depth or query the deepest listed nodes to see the rest"});
    }
    if reachability.get("depth_limit_reached") == Some(&Value::Bool(true)) {
        let max = reachability
            .get("max_depth")
            .cloned()
            .unwrap_or(Value::Null);
        return json!({"tool": "query_graph_tool", "suggestion": format!("nodes beyond {max} hops may exist; raise depth (max 6) or query the deepest listed nodes")});
    }
    json!({"tool": null, "suggestion": "the transitive set is closed over graph edges: no other node is reachable, so querying listed nodes again returns nothing new"})
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
        // Python treats a level it does not know as `standard`.
        Some(_) => "standard",
    };
    // Python treats any other level as `standard`.
    let (full, minimal) = (detail_level == "full", detail_level == "minimal");
    let depth = args.integer("depth", 1)?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    // Python checks the pattern and depth once the store is open, whatever
    // the checkout.
    let graph = open_graph(&root)?;
    if let Some(message) = argument_error(pattern, depth) {
        return Some(
            Ordered::default()
                .put("status", "error")
                .put("error", message)
                .put("_repo", graph.repo_context())
                .into_payload(),
        );
    }
    let depth = depth.min(MAX_DEPTH);
    let store = &graph.store;
    let answerability = graph.answerability()?;

    if pattern == "callers_of" && !target.contains("::") && BUILTIN_CALL_NAMES.contains(&target) {
        return Some(
            Ordered::default()
                .put("status", "ok")
                .put("pattern", pattern)
                .put("target", target)
                .put("description", description(pattern).unwrap_or(""))
                .put(
                    "summary",
                    format!("'{target}' is a common builtin — callers_of skipped to avoid noise."),
                )
                .put("results", json!([]))
                .put("edges", json!([]))
                .put("answerability", answerability.full())
                .put("missingness", json!(answerability.missingness()))
                .put("_repo", graph.repo_context())
                .into_payload(),
        );
    }

    // `resolve_query_target`.
    let exact = match store.get_node(target).ok()? {
        Some(node) => Some(node),
        None => store.get_node(&joined(&root, target)?).ok()?,
    };
    let (node, resolution) = match exact {
        Some(node) => (Some(node), Resolution::Exact),
        None => {
            let named_exists = || -> Option<bool> {
                Some(
                    store
                        .search_nodes(target, NAME_SEARCH_LIMIT)
                        .ok()?
                        .iter()
                        .any(|hit| hit.name == target),
                )
            };
            if pattern == "callers_of"
                && !named_exists()?
                && is_external_package_target(store, target)?
            {
                (None, Resolution::ExternalPackage)
            } else if pattern == "file_summary" && looks_like_file_target(target) {
                let candidates = file_path_candidates(&root, target)?;
                if !file_is_indexed(store, &candidates)? {
                    return Some(not_found(pattern, target, true, &answerability, &graph));
                }
                (None, Resolution::Exact)
            } else if !looks_like_file_target(target) {
                let hits = store.search_nodes(target, NAME_SEARCH_LIMIT).ok()?;
                let named: Vec<&GraphNode> = hits.iter().filter(|hit| hit.name == target).collect();
                let candidates: Vec<&GraphNode> = if named.is_empty() {
                    hits.iter().take(5).collect()
                } else {
                    named.iter().copied().take(5).collect()
                };
                if named.len() == 1 {
                    (Some(named[0].clone()), Resolution::ExactName)
                } else if candidates.len() == 1 {
                    (Some(candidates[0].clone()), Resolution::Fuzzy)
                } else if candidates.len() > 1 {
                    return Some(ambiguous(
                        pattern,
                        target,
                        &candidates,
                        &answerability,
                        &graph,
                    ));
                } else if pattern == "file_summary" {
                    (None, Resolution::Exact)
                } else {
                    return Some(not_found(pattern, target, false, &answerability, &graph));
                }
            } else {
                return Some(not_found(pattern, target, false, &answerability, &graph));
            }
        }
    };
    // A name-resolved target is reported by its qualified name.
    let original_target = target;
    let target: &str = match (&node, resolution) {
        (Some(node), Resolution::ExactName | Resolution::Fuzzy) => &node.qualified_name,
        _ => target,
    };
    let exact_count = i64::from(
        matches!(resolution, Resolution::Exact | Resolution::ExactName) && node.is_some(),
    );
    let found = run_pattern(store, &root, pattern, target, node.as_ref(), depth)?;

    // `query_zero_result_fields`.
    let raw_count = found.rows.len();
    let (confidence, zero_result_reason) = if raw_count > 0 {
        let high = !found.edges.is_empty()
            && found.edges.iter().all(|edge| {
                matches!(
                    edge.get("confidence_tier")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                    "EXACT" | "EXTRACTED" | "HIGH"
                )
            });
        (if high { "high" } else { "medium" }, Value::Null)
    } else if !found.unresolved.is_empty() {
        ("medium", json!("unresolved_endpoints_only"))
    } else {
        ("low", json!("not_found_in_current_graph"))
    };
    let next_action = exactness_action(pattern, exact_count, raw_count);
    let guidance = guidance(pattern, target, raw_count, exact_count);
    let mut summary = format!("Found {raw_count} result(s) for {pattern}('{target}')");
    if depth > 1 {
        summary.push_str(&format!(" within {depth} hops"));
    }

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
    if matches!(resolution, Resolution::ExactName | Resolution::Fuzzy) {
        payload = payload.put("resolved_target", target);
        payload = payload.put("original_target", original_target);
    }
    if let Some(reachability) = &found.reachability {
        payload = payload
            .put("depth", depth)
            .put("reachability", reachability.clone());
    }
    let budget;
    if full {
        let edges: Vec<Value> = found.edges.iter().cloned().map(Value::Object).collect();
        payload = payload
            .put("description", description(pattern).unwrap_or(""))
            .put("summary", summary)
            .put("result_count", raw_count)
            .put("answerability", answerability.full())
            .put(
                "results",
                json!(found.rows.iter().cloned().map(object).collect::<Vec<_>>()),
            )
            .put("edges", json!(edges))
            .put("guidance", guidance.clone())
            .put("_hints", hints(&guidance));
        budget = 8000;
    } else {
        let mut rows: Vec<Row> = found.rows.iter().map(|row| compact(row, minimal)).collect();
        if MERGED.contains(&pattern) {
            rows = merge(rows);
        }
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|row| {
                if minimal {
                    project_minimal(row)
                } else {
                    object(row)
                }
            })
            .collect();
        let summary = summary.replacen(
            &format!("Found {raw_count} "),
            &format!("Found {} ", rows.len()),
            1,
        );
        payload = payload
            .put("summary", summary)
            .put("result_count", rows.len())
            .put("answerability", answerability.compact())
            .put("results", json!(rows));
        if !minimal {
            payload = payload.put("guidance", guidance);
        }
        budget = if minimal { 4000 } else { 8000 };
    }
    payload = payload.apply_output_budget(budget, &["results", "edges"]);
    let results_complete = !payload
        .get("_truncation")
        .and_then(Value::as_object)
        .is_some_and(|truncation| truncation.contains_key("results"));
    payload = payload.put("results_complete", results_complete);
    if let Some(reachability) = &found.reachability {
        payload = payload.replace(
            "next_action",
            transitive_next_action(reachability, results_complete),
        );
    }
    let mut missingness = answerability.missingness();
    // `_attach_source_of_coverage`.
    if let Some(coverage) = found.source
        && raw_count > 0
    {
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
