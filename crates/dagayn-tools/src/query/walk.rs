//! Running a pattern: the edges each one follows, and the transitive walks.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use dagayn_graph::{GraphEdge, GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use super::resolve::{file_path_candidates, joined};
use super::rows::{
    Row, VALUE_REFERENCE, cross_artifact_role, edge_dict, edge_import_kind, get,
    import_kind_matches, node_row, resolved_by,
};
use crate::pypath::realpath;
use crate::source::{SourceCoverage, source_row};

/// `_TRANSITIVE_ROW_LIMIT`.
pub(super) const TRANSITIVE_ROW_LIMIT: usize = 500;
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
pub(super) struct Found {
    pub(super) rows: Vec<Row>,
    pub(super) edges: Vec<Map<String, Value>>,
    pub(super) unresolved: Vec<String>,
    pub(super) reachability: Option<Value>,
    pub(super) source: Option<SourceCoverage>,
    /// `tests_for`: what `review_tool`'s `untested_change` decides from.
    pub(super) test_reach: Option<Value>,
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
                if let Some(by) = resolved_by(edge) {
                    row.push(("resolved_by", json!(by)));
                }
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
    let mut row = vec![
        ("importer", json!(edge.source_qualified)),
        ("file", json!(edge.file_path)),
        ("line", json!(edge.line)),
        ("confidence_tier", json!(edge.confidence_tier.as_str())),
    ];
    push_import_kind(&mut row, edge);
    row
}

/// A Markdown import edge is a dependency directive or an ordinary link; the
/// row says which (`import_kind`), so a question about directives can filter.
fn push_import_kind(row: &mut Row, edge: &GraphEdge) {
    if let Some(kind) = edge_import_kind(edge) {
        row.push(("import_kind", json!(kind)));
    }
}

/// `imports_of` past hop 1: the files each imported file imports in turn,
/// one row per newly reached file, up to `depth` hops.
fn expand_imports(
    store: &GraphStore,
    root: &Path,
    found: &mut Found,
    start: &str,
    depth: i64,
    import_kind: Option<&str>,
) -> Option<()> {
    for row in &mut found.rows {
        row.push(("depth", json!(1)));
    }
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(start.to_string());
    let mut frontier: Vec<String> = Vec::new();
    for row in &found.rows {
        if get(row, "unresolved") == Some(&Value::Bool(true)) {
            continue;
        }
        if let Some(target) = get(row, "import_target").and_then(Value::as_str)
            && seen.insert(target.to_string())
        {
            frontier.push(target.to_string());
        }
    }
    let mut hop = 2;
    let mut added = 0;
    let mut truncated = false;
    while !frontier.is_empty() && hop <= depth && !truncated {
        let (outgoing, _) = store.get_edges_by_endpoints(&frontier).ok()?;
        let mut layer: Vec<GraphEdge> = Vec::new();
        for via in &frontier {
            for edge in outgoing.get(via).into_iter().flatten() {
                if edge.kind != "IMPORTS_FROM"
                    || !import_kind_matches(edge, import_kind)
                    || seen.contains(&edge.target_qualified)
                {
                    continue;
                }
                seen.insert(edge.target_qualified.clone());
                layer.push(edge.clone());
            }
        }
        if added + layer.len() > TRANSITIVE_ROW_LIMIT {
            layer.truncate(TRANSITIVE_ROW_LIMIT - added);
            truncated = true;
        }
        let mut next = Vec::new();
        for edge in &layer {
            let unresolved = is_unresolved_import(store, root, &edge.target_qualified)?;
            let mut row = vec![
                ("import_target", json!(edge.target_qualified)),
                ("line", json!(edge.line)),
                ("unresolved", json!(unresolved)),
            ];
            push_import_kind(&mut row, edge);
            row.push(("depth", json!(hop)));
            row.push(("via", json!(edge.source_qualified)));
            found.rows.push(row);
            found.edges.push(edge_dict(edge));
            if unresolved {
                found.unresolve(vec![edge.target_qualified.clone()]);
            } else {
                next.push(edge.target_qualified.clone());
            }
        }
        added += layer.len();
        frontier = next;
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
    import_kind: Option<&str>,
) -> Option<()> {
    for row in &mut found.rows {
        row.push(("depth", json!(1)));
    }
    let first_hop: Vec<(String, bool)> = found
        .rows
        .iter()
        .map(|row| {
            let key = if importers { "file" } else { "qualified_name" };
            let name = get(row, key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            (name, get(row, "relation") == Some(&json!(VALUE_REFERENCE)))
        })
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(start.to_string());
    seen.extend(first_hop.iter().map(|(key, _)| key.clone()));
    let mut frontier: Vec<String> = Vec::new();
    // A place that only stores the target as a value does not call it, so
    // its own callers are not the target's.
    for (key, reference) in first_hop {
        if !reference && !frontier.contains(&key) {
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
                if edge.kind != kind
                    || (importers && !import_kind_matches(edge, import_kind))
                    || seen.contains(&key)
                {
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
pub(super) fn run_pattern(
    store: &GraphStore,
    root: &Path,
    pattern: &str,
    target: &str,
    node: Option<&GraphNode>,
    depth: i64,
    import_kind: Option<&str>,
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
            let incoming = edges_of(false)?;
            let edges: Vec<GraphEdge> = incoming
                .iter()
                .filter(|e| e.kind == "CALLS")
                .cloned()
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
            // Where a Go method is used as a value (`d.step =
            // (*compressor).storeFast`): its calls through that value are
            // not edges, so the place that stores it stands in for them
            // (#235). Other languages record such references too, but
            // their replies do not show them yet.
            if node.is_some_and(|node| node.language == "go") {
                let references: Vec<GraphEdge> = incoming
                    .into_iter()
                    .filter(|e| {
                        e.kind == "REFERENCES"
                            && e.extra.get("value_reference") == Some(&Value::Bool(true))
                    })
                    .collect();
                let mut rows = found.edge_rows(store, &references, true)?;
                for row in &mut rows {
                    row.push(("relation", json!(VALUE_REFERENCE)));
                }
                found.rows.extend(rows);
                found.edges.extend(references.iter().map(edge_dict));
            }
            if depth > 1 {
                expand_transitive(store, &mut found, qualified, depth, false, None)?;
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
                if edge.kind != "IMPORTS_FROM" || !import_kind_matches(&edge, import_kind) {
                    continue;
                }
                let unresolved = is_unresolved_import(store, root, &edge.target_qualified)?;
                let mut row = vec![
                    ("import_target", json!(edge.target_qualified)),
                    ("line", json!(edge.line)),
                    ("unresolved", json!(unresolved)),
                ];
                push_import_kind(&mut row, &edge);
                found.rows.push(row);
                found.edges.push(edge_dict(&edge));
                if unresolved {
                    found.unresolve(vec![edge.target_qualified.clone()]);
                }
            }
            if depth > 1 {
                expand_imports(store, root, &mut found, qualified, depth, import_kind)?;
            }
        }
        "importers_of" => {
            // A directory (a Go package) is imported through its files.
            let files = match node {
                Some(node) => vec![node.file_path.clone()],
                None => store.files_in_directory(target).ok()?,
            };
            for file in &files {
                for edge in store.get_edges_by_target(file).ok()? {
                    if edge.kind == "IMPORTS_FROM" && import_kind_matches(&edge, import_kind) {
                        found.rows.push(importer_row(&edge));
                        found.edges.push(edge_dict(&edge));
                    }
                }
            }
            if depth > 1 {
                expand_transitive(store, &mut found, qualified, depth, true, import_kind)?;
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
            // The nearest test as `untested_change` counts it, a little past
            // the limit it trusts, so a test just beyond it shows.
            let limit = crate::findings::caller_test_depth(&node.language);
            let search = limit.map_or(usize::MAX, |limit| limit + 4);
            found.test_reach = Some(
                match crate::findings::nearest_test(store, root, &node.qualified_name, search)? {
                    Some((hops, test)) => json!({
                        "nearest_test": test,
                        "hops": hops,
                        "counts_as_tested": limit.is_none_or(|limit| hops <= limit),
                        "hop_limit": limit,
                    }),
                    None => json!({
                        "nearest_test": null,
                        "hops": null,
                        "counts_as_tested": false,
                        "hop_limit": limit,
                    }),
                },
            );
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
