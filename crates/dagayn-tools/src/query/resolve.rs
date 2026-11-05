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
const NAME_SEARCH_LIMIT: i64 = 200;

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

fn file_is_indexed(store: &GraphStore, candidates: &[String]) -> Option<bool> {
    for path in candidates {
        if !store.get_nodes_by_file(path).ok()?.is_empty() || store.get_node(path).ok()?.is_some() {
            return Some(true);
        }
    }
    Some(false)
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

/// A target that names more than one node: the candidates the reply shows
/// (production code first) and how many nodes the name matched.
pub(super) struct Ambiguity {
    pub(super) candidates: Vec<GraphNode>,
    pub(super) total: usize,
    /// Whether every candidate carries the name (or its respelling) exactly,
    /// as opposed to search hits that only look like it.
    pub(super) exact_name: bool,
}

/// Candidates an ambiguous reply lists.
pub(super) const SHOWN_CANDIDATES: usize = 5;

impl Ambiguity {
    fn new<'a>(
        matches: impl IntoIterator<Item = &'a GraphNode>,
        total: usize,
        exact_name: bool,
    ) -> Self {
        let mut candidates: Vec<GraphNode> = crate::next::tests_last(matches)
            .into_iter()
            .cloned()
            .collect();
        candidates.truncate(SHOWN_CANDIDATES);
        Self {
            candidates,
            total,
            exact_name,
        }
    }

    /// Whether the name matched more nodes than the reply lists.
    pub(super) fn truncated(&self) -> bool {
        self.total > self.candidates.len()
    }
}

/// How a target failed to name one node.
pub(super) enum Unresolved {
    /// The answer to give as it is (`not_found`).
    Answer(Payload),
    Ambiguous(Ambiguity),
}

/// What an ambiguous reply answers for each candidate: one entry per
/// candidate, in `candidates` order, and the ones cut.
pub(super) struct Answers {
    pub(super) answers: Vec<Value>,
    /// The rows each answer shows at most.
    pub(super) rows: usize,
    pub(super) cut: Vec<Cut>,
}

/// A candidate whose answer shows only some of its rows.
pub(super) struct Cut {
    /// Its place in `candidates`.
    pub(super) index: usize,
    /// The rows it has, and the rows its answer shows.
    pub(super) rows: u64,
    pub(super) shown: usize,
}

/// The sentence that says how to resend `target` for one candidate: the
/// qualified name is the exact form, and `path::Type.method` resolves too
/// (for Go also `path::(*Type).Method`), named where a candidate is a
/// member of a type. Go package forms such as
/// `pkg.Type.Method` are not named: they do not resolve to the method.
fn resend_hint(candidates: &[GraphNode]) -> &'static str {
    let members = candidates.iter().filter(|node| node.parent_name.is_some());
    if members.clone().any(|node| node.language == "go") {
        "Resend target as one candidate's qualified_name (path::Type.Method; \
         for Go also path::(*Type).Method)."
    } else if members.count() > 0 {
        "Resend target as one candidate's qualified_name (path::Type.method)."
    } else {
        "Resend target as one candidate's qualified_name."
    }
}

/// The early `ambiguous` answer of `resolve_query_target`, with `answers`
/// where every candidate is answered.
pub(super) fn ambiguous(
    arguments: &Map<String, Value>,
    pattern: &str,
    target: &str,
    ambiguity: &Ambiguity,
    answered: Option<&Answers>,
    answerability: &Answerability,
    graph: &OpenGraph,
) -> Payload {
    let candidates: Vec<&GraphNode> = ambiguity.candidates.iter().collect();
    let mut missingness = answerability.missingness();
    missingness.push(json!({
        "reason_code": "ambiguous_target",
        "severity": "medium",
        "claim_effect": if answered.is_some() {
            "each candidate is answered on its own; no answer is for the name alone"
        } else {
            "relationship query was not run for a unique node"
        },
    }));
    let kept_args = [
        ("pattern", None),
        ("depth", Some(json!(1))),
        ("detail_level", Some(json!("standard"))),
        ("import_kind", None),
        ("path_prefix", None),
    ];
    let hint = resend_hint(&ambiguity.candidates);
    let (summary, next) = match answered {
        None => (
            format!("Multiple matches for '{target}'. {hint}"),
            crate::next::retries("query_graph_tool", arguments, &kept_args, &candidates),
        ),
        Some(answered) => {
            let count = candidates.len();
            let mut summary = format!("'{target}' matches {count} nodes; each is answered below");
            if answered.cut.is_empty() {
                summary.push('.');
            } else {
                let cut = answered.cut.len();
                summary.push_str(&format!(
                    ", at most {} rows each; next has the full query for the {cut} cut.",
                    answered.rows
                ));
            }
            summary.push(' ');
            summary.push_str(hint);
            let cut_nodes: Vec<&GraphNode> = answered
                .cut
                .iter()
                .map(|cut| candidates[cut.index])
                .collect();
            let mut next =
                crate::next::retries("query_graph_tool", arguments, &kept_args, &cut_nodes);
            if let Some(calls) = next.as_array_mut() {
                for (call, cut) in calls.iter_mut().zip(&answered.cut) {
                    let why = call["why"].as_str().unwrap_or("").to_string();
                    call["why"] = json!(format!(
                        "all {} rows of {why}; the answer shows {}",
                        cut.rows, cut.shown
                    ));
                }
            }
            (summary, next)
        }
    };
    let mut payload = Ordered::default()
        .put("status", "ambiguous")
        .put("pattern", pattern)
        .put("target", target)
        .put("summary", summary)
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
        .put("candidates_truncated", ambiguity.truncated());
    if let Some(answered) = answered {
        payload = payload.put("answers", json!(answered.answers));
    }
    payload
        .put("next", next)
        .put("answerability", answerability.full())
        .put("missingness", json!(missingness))
        .put("_repo", graph.repo_context())
        .into_payload()
}

/// `resolve_query_target`: the node `target` names and how it was found, or
/// why it names no one node; `None` where Python answers.
pub(super) fn resolve(
    graph: &OpenGraph,
    root: &Path,
    pattern: &str,
    target: &str,
    answerability: &Answerability,
) -> Option<Result<(Option<GraphNode>, Resolution), Unresolved>> {
    let store = &graph.store;
    let exact = |qualified: &str| -> Option<Option<GraphNode>> {
        match store.get_node(qualified).ok()? {
            Some(node) => Some(Some(node)),
            None => store.get_node(&joined(root, qualified)?).ok(),
        }
    };
    let exact_node = exact(target)?;
    let mut respelled = match exact_node {
        Some(_) => Vec::new(),
        None => crate::target::respelled(store, target, exact)?,
    };
    if respelled.len() > 1 {
        return Some(Err(Unresolved::Ambiguous(Ambiguity::new(
            respelled.iter(),
            respelled.len(),
            true,
        ))));
    }
    let (node, resolution) = match (exact_node, respelled.pop()) {
        (Some(node), _) => (Some(node), Resolution::Exact),
        (None, Some(node)) => (Some(node), Resolution::ExactName),
        (None, None) => {
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
            } else if pattern == "importers_of"
                && !named_exists()?
                && !store.files_in_directory(target).ok()?.is_empty()
            {
                // A directory (a Go package): `run_pattern` reads its files.
                (None, Resolution::Exact)
            } else if pattern == "file_summary" && looks_like_file_target(target) {
                let candidates = file_path_candidates(root, target)?;
                if !file_is_indexed(store, &candidates)? {
                    return Some(Err(Unresolved::Answer(not_found(
                        pattern,
                        target,
                        true,
                        answerability,
                        graph,
                    ))));
                }
                (None, Resolution::Exact)
            } else if !looks_like_file_target(target) {
                let hits = store.search_nodes(target, NAME_SEARCH_LIMIT).ok()?;
                let named: Vec<&GraphNode> = hits.iter().filter(|hit| hit.name == target).collect();
                // Where no node has the name itself, the search hits (at
                // most `NAME_SEARCH_LIMIT`): the reply lists the first five.
                let ambiguity = if named.is_empty() {
                    Ambiguity::new(hits.iter().take(SHOWN_CANDIDATES), hits.len(), false)
                } else {
                    Ambiguity::new(named.iter().copied(), named.len(), true)
                };
                if named.len() == 1 {
                    (Some(named[0].clone()), Resolution::ExactName)
                } else if ambiguity.candidates.len() == 1 {
                    (Some(ambiguity.candidates[0].clone()), Resolution::Fuzzy)
                } else if ambiguity.candidates.len() > 1 {
                    return Some(Err(Unresolved::Ambiguous(ambiguity)));
                } else if pattern == "file_summary" {
                    (None, Resolution::Exact)
                } else {
                    return Some(Err(Unresolved::Answer(not_found(
                        pattern,
                        target,
                        false,
                        answerability,
                        graph,
                    ))));
                }
            } else {
                return Some(Err(Unresolved::Answer(not_found(
                    pattern,
                    target,
                    false,
                    answerability,
                    graph,
                ))));
            }
        }
    };
    Some(Ok((node, resolution)))
}
