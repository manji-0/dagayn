//! Finding the node a `query_graph` target names, and the answers given
//! when there is none or more than one.

use std::path::Path;

use dagayn_graph::{GraphEdge, GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use super::description;
use super::notes::guidance;
use super::rows::node_dict;
use crate::answerability::Answerability;
use crate::pypath::{pure_join, realpath};
use crate::{OpenGraph, Ordered, Payload};

/// `_NAME_RESOLUTION_SEARCH_LIMIT`.
pub(super) const NAME_SEARCH_LIMIT: i64 = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Resolution {
    Exact,
    ExactName,
    Fuzzy,
    ExternalPackage,
}

impl Resolution {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::ExactName => "exact_name",
            Self::Fuzzy => "fuzzy",
            Self::ExternalPackage => "external_package",
        }
    }
}

/// `looks_like_file_target` (`dagayn.bare_name_resolution`).
pub(crate) fn looks_like_file_target(target: &str) -> bool {
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
pub(super) fn is_external_package_target(store: &GraphStore, target: &str) -> Option<bool> {
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
pub(super) fn joined(root: &Path, target: &str) -> Option<String> {
    Some(pure_join(root.to_str()?, target))
}

/// `file_path_candidates`: the joined path, its non-strict `resolve()`, and
/// an absolute target as given. `None` only where Python raises (a NUL).
pub(super) fn file_path_candidates(root: &Path, target: &str) -> Option<Vec<String>> {
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

pub(super) fn file_is_indexed(store: &GraphStore, candidates: &[String]) -> Option<bool> {
    for path in candidates {
        if !store.get_nodes_by_file(path).ok()?.is_empty() || store.get_node(path).ok()?.is_some() {
            return Some(true);
        }
    }
    Some(false)
}

/// The early `not_found` answer of `resolve_query_target`; the
/// `file_summary` form names the pattern and speaks of a file.
pub(super) fn not_found(
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
        .put(
            "next",
            json!([crate::next::call(
                "semantic_search_nodes_tool",
                json!({"query": target, "detail_level": "minimal"}),
                "find the node by name or meaning",
            )]),
        )
        .put("answerability", answerability.full())
        .put("missingness", json!(missingness))
        .put("guidance", guidance)
        .put("_repo", graph.repo_context())
        .into_payload()
}

/// The early `ambiguous` answer of `resolve_query_target`.
pub(super) fn ambiguous(
    arguments: &Map<String, Value>,
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
        .put(
            "next",
            crate::next::retries(
                "query_graph_tool",
                arguments,
                &[
                    ("pattern", None),
                    ("depth", Some(json!(1))),
                    ("detail_level", Some(json!("standard"))),
                    ("import_kind", None),
                    ("path_prefix", None),
                ],
                candidates,
            ),
        )
        .put("answerability", answerability.full())
        .put("missingness", json!(missingness))
        .put("_repo", graph.repo_context())
        .into_payload()
}
