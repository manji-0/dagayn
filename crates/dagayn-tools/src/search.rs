//! `semantic_search_nodes_tool` (`dagayn.tools.query.semantic_search_nodes`
//! over `dagayn.search.hybrid_search`) when no embedding provider takes part:
//! nothing in the arguments, the server defaults, or the environment selects
//! one, and the graph stores no vectors a persisted provider name could
//! revive. The embedding arm then reports `provider_unavailable` and the
//! answer is the FTS (or keyword) arm alone, which is reproduced here.
//! Anything that would embed the query is Python's.

use std::collections::{HashMap, HashSet};

use dagayn_build::{Vcs, detect_vcs};
use dagayn_graph::{GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::{Args, Context, Ordered, Payload, explicit_repo, open_graph};

/// `_MAX_SEARCH_LIMIT`.
const MAX_LIMIT: i64 = 200;
const TEST_DEBOOST: f64 = 0.6;

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "for", "of", "to", "in", "on", "by", "with", "from", "and",
    "or", "not", "where", "how", "what", "which", "find", "show", "list", "all", "any", "this",
    "that", "these", "those", "it", "we", "do", "does", "did",
];
const DOC_INTENT: &[&str] = &[
    "documentation",
    "readme",
    "usage",
    "guide",
    "section",
    "instructions",
];
const PROCESS_PATTERN: &[&str] = &[
    "assigns",
    "branches",
    "builds",
    "calls",
    "computes",
    "converts",
    "creates",
    "deletes",
    "detects",
    "embedding",
    "embeddings",
    "embeds",
    "fetches",
    "filters",
    "inserts",
    "iterates",
    "loads",
    "loops",
    "merges",
    "opens",
    "parses",
    "queries",
    "ranks",
    "reads",
    "rebuilds",
    "renders",
    "returns",
    "searches",
    "stores",
    "tested",
    "updates",
    "uses",
    "validates",
    "writes",
];

/// `_TOKEN_RE.findall`: runs of `[A-Za-z0-9_]`.
fn tokens(text: &str) -> Vec<&str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
        .collect()
}

/// `_IDENT_RE.findall`: `[A-Za-z_][A-Za-z0-9_]+`, scanned left to right.
fn identifier_matches(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if i - start >= 2 {
                out.push(&text[start..i]);
            }
        } else {
            i += 1;
        }
    }
    out
}

/// `_extract_identifiers`: snake_case or mixed-case tokens, in order, once.
fn extract_identifiers(query: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for candidate in identifier_matches(query) {
        if STOPWORDS.contains(&candidate.to_lowercase().as_str()) {
            continue;
        }
        let snake = candidate.contains('_');
        let camelish = candidate.chars().skip(1).any(|c| c.is_uppercase());
        if (snake || camelish) && !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// `_query_rerank_intent` followed by `_embedding_text_mode_for_intent`:
/// only `process_pattern` selects the narrative partition.
fn embedding_text_mode(query: &str) -> &'static str {
    let stripped = query.trim();
    let query_tokens: HashSet<String> = tokens(query).iter().map(|t| t.to_lowercase()).collect();
    let intersects = |terms: &[&str]| terms.iter().any(|term| query_tokens.contains(*term));
    if stripped.is_empty() {
        return "material"; // `empty`
    }
    if stripped.contains('.')
        || stripped.contains("::")
        || !extract_identifiers(stripped).is_empty()
    {
        return "material"; // `exact`
    }
    if intersects(DOC_INTENT) {
        return "material"; // `documentation`
    }
    if intersects(PROCESS_PATTERN) {
        return "narrative"; // `process_pattern`
    }
    "material" // `purpose` or `exact`
}

/// `detect_query_kind_boost`.
fn kind_boosts(query: &str) -> Vec<(&'static str, f64)> {
    let mut boosts = Vec::new();
    let q = query.trim();
    if q.is_empty() {
        return boosts;
    }
    let mut chars = q.chars();
    let pascal = matches!((chars.next(), chars.next()), (Some(a), Some(b)) if a.is_ascii_uppercase() && b.is_ascii_lowercase());
    // `str.isupper()`: some cased character, none lowercase.
    let is_upper = q.chars().any(char::is_uppercase) && !q.chars().any(char::is_lowercase);
    if pascal && !is_upper {
        boosts.push(("Class", 1.5));
        boosts.push(("Type", 1.5));
    }
    if q.contains('_') && q.chars().any(|c| c.is_ascii_alphabetic()) {
        boosts.push(("Function", 1.5));
    }
    if q.contains('.') {
        boosts.push(("_qualified", 2.0));
    }
    boosts
}

/// `_qualified_name_matches`.
fn qualified_name_matches(query: &str, qualified_name: &str) -> bool {
    let q = query.to_lowercase();
    let qn = qualified_name.to_lowercase();
    if qn.contains(&q) {
        return true;
    }
    let split = |s: &str| -> Vec<String> {
        s.split(['.', '/', ':'])
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    };
    let q_tokens = split(&q);
    if q_tokens.is_empty() {
        return false;
    }
    let mut i = 0;
    for token in split(&qn) {
        if i < q_tokens.len() && token == q_tokens[i] {
            i += 1;
        }
    }
    i == q_tokens.len()
}

/// `rrf_merge` with `k = 10`: insertion order, then a stable sort by score.
fn rrf_merge(lists: &[&[(i64, f64)]]) -> Vec<(i64, f64)> {
    let mut order: Vec<i64> = Vec::new();
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for list in lists {
        for (rank, (id, _)) in list.iter().enumerate() {
            let entry = scores.entry(*id).or_insert_with(|| {
                order.push(*id);
                0.0
            });
            *entry += 1.0 / (10.0 + rank as f64 + 1.0);
        }
    }
    let mut merged: Vec<(i64, f64)> = order.into_iter().map(|id| (id, scores[&id])).collect();
    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged
}

/// Python's `round(value, 6)`.
fn round6(value: f64) -> f64 {
    format!("{value:.6}").parse().unwrap_or(value)
}

fn sanitize(name: &str) -> String {
    name.chars()
        .filter(|c| *c == '\t' || *c == '\n' || (*c as u32) >= 0x20)
        .take(256)
        .collect()
}

struct Hits {
    mode: &'static str,
    results: Vec<Map<String, Value>>,
    truncated: bool,
    total: usize,
}

/// `hybrid_search` with an empty embedding arm; `None` when a query would
/// fail in a way Python only logs.
fn fts_search(store: &GraphStore, query: &str, kind: Option<&str>, limit: i64) -> Option<Hits> {
    let empty = Hits {
        mode: "empty",
        results: Vec::new(),
        truncated: false,
        total: 0,
    };
    let (mut multiplier, max_multiplier) = if kind.is_some() { (12, 48) } else { (3, 9) };
    let mut merged: Vec<(i64, f64)>;
    let mut fts_results: Vec<(i64, f64)>;
    let mut keyword_results: Vec<(i64, f64)>;
    let mut keyword_mode;
    let mut mode;
    loop {
        let fetch_limit = limit * multiplier;
        fts_results = Vec::new();
        keyword_results = Vec::new();
        let mut match_modes: Vec<&str> = Vec::new();
        let (hits, match_mode) = store.fts_query(query, fetch_limit).ok()?;
        fts_results.extend(hits);
        if match_mode != "none" {
            match_modes.push(match_mode);
        }
        for ident in extract_identifiers(query) {
            let Ok((hits, match_mode)) = store.fts_query(ident, fetch_limit) else {
                continue;
            };
            if !hits.is_empty() {
                fts_results.extend(hits);
                if match_mode != "none" {
                    match_modes.push(match_mode);
                }
            }
        }
        let any_match = !match_modes.contains(&"and") && match_modes.contains(&"or");
        if !fts_results.is_empty() {
            let ids: Vec<i64> = fts_results.iter().map(|(id, _)| *id).collect();
            let valid = store.get_nodes_by_ids(&ids).ok()?;
            fts_results.retain(|(id, _)| valid.contains_key(id));
        }
        keyword_mode = false;
        if !fts_results.is_empty() {
            merged = rrf_merge(&[fts_results.as_slice()]);
            if any_match {
                keyword_results = store.keyword_query(query, fetch_limit).ok()?;
                if !keyword_results.is_empty() {
                    merged = rrf_merge(&[merged.as_slice(), keyword_results.as_slice()]);
                }
            }
        } else {
            keyword_results = store.keyword_query(query, fetch_limit).ok()?;
            if keyword_results.is_empty() {
                return Some(empty);
            }
            merged = keyword_results.clone();
            keyword_mode = true;
        }
        mode = if keyword_mode {
            "keyword_fallback"
        } else {
            "fts_only"
        };
        let Some(kind) = kind else { break };
        let ids: Vec<i64> = merged.iter().map(|(id, _)| *id).collect();
        let nodes = store.get_nodes_by_ids(&ids).ok()?;
        let kind_hits = ids
            .iter()
            .filter(|id| nodes.get(id).is_some_and(|node| node.kind == kind))
            .count() as i64;
        if kind_hits >= limit || multiplier >= max_multiplier {
            break;
        }
        multiplier *= 2;
    }
    if merged.is_empty() {
        return Some(empty);
    }
    let fts_ids: HashSet<i64> = fts_results.iter().map(|(id, _)| *id).collect();
    let keyword_ids: HashSet<i64> = keyword_results.iter().map(|(id, _)| *id).collect();
    let boosts = kind_boosts(query);
    let boost_for = |key: &str| boosts.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    let query_tokens: HashSet<String> = tokens(query).iter().map(|t| t.to_lowercase()).collect();
    let test_intent = ["test", "tests", "coverage", "proves"]
        .iter()
        .any(|term| query_tokens.contains(*term));
    let ids: Vec<i64> = merged.iter().map(|(id, _)| *id).collect();
    let nodes: HashMap<i64, GraphNode> = store.get_nodes_by_ids(&ids).ok()?;
    let mut boosted: Vec<(i64, f64)> = Vec::new();
    for (id, score) in &merged {
        let Some(node) = nodes.get(id) else { continue };
        let mut boost = 1.0_f64;
        if let Some(kind_boost) = boost_for(&node.kind) {
            boost *= kind_boost;
        }
        if let Some(qualified_boost) = boost_for("_qualified")
            && qualified_name_matches(query, &node.qualified_name)
        {
            boost *= qualified_boost;
        }
        // `_intent_boost` is 1.0 outside hybrid mode.
        if node.is_test && !test_intent {
            boost *= TEST_DEBOOST;
        }
        boosted.push((*id, score * boost));
    }
    boosted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let eligible: Vec<(i64, f64)> = boosted
        .into_iter()
        .filter(|(id, _)| {
            nodes
                .get(id)
                .is_some_and(|node| kind.is_none_or(|kind| node.kind == kind))
        })
        .collect();
    let mut results = Vec::new();
    for (id, score) in eligible.iter().take(limit as usize) {
        let node = &nodes[id];
        let source = if node.kind == "DocSection" {
            "doc"
        } else if keyword_mode || keyword_ids.contains(id) {
            "keyword"
        } else if fts_ids.contains(id) {
            "fts"
        } else {
            "embedding"
        };
        let mut result = Map::new();
        result.insert("name".into(), json!(sanitize(&node.name)));
        result.insert(
            "qualified_name".into(),
            json!(sanitize(&node.qualified_name)),
        );
        result.insert("kind".into(), json!(node.kind));
        result.insert("file_path".into(), json!(node.file_path));
        result.insert("line_start".into(), json!(node.line_start));
        result.insert("line_end".into(), json!(node.line_end));
        result.insert("language".into(), json!(node.language));
        result.insert("params".into(), json!(node.params));
        result.insert("return_type".into(), json!(node.return_type));
        result.insert("signature".into(), json!(node.signature));
        result.insert("score".into(), json!(round6(*score)));
        result.insert("rank".into(), json!(results.len() + 1));
        result.insert("source".into(), json!(source));
        result.insert("is_test".into(), json!(node.is_test));
        results.push(result);
    }
    Some(Hits {
        mode,
        results,
        truncated: eligible.len() > limit as usize,
        total: eligible.len(),
    })
}

/// `guidance_actions_to_hints` for one guidance item.
fn hints(action: &str, warnings: &[&str]) -> Value {
    let head = action.split(" -- ").next().unwrap_or(action);
    let tool = head.split(' ').next().unwrap_or(head);
    let tool = tool.split('(').next().unwrap_or(tool);
    json!({
        "next_steps": [{"tool": tool, "suggestion": action}],
        "related": [],
        "warnings": warnings,
    })
}

pub(crate) fn semantic_search(
    context: &Context,
    arguments: &Map<String, Value>,
) -> Option<Payload> {
    let args = Args::new(
        arguments,
        &[
            "query",
            "kind",
            "limit",
            "repo_root",
            "model",
            "provider",
            "detail_level",
        ],
    )?;
    let query = args.string("query")?;
    if query.trim().is_empty() {
        return None;
    }
    let kind = args.optional_string("kind")?;
    let limit = args.integer("limit", 20)?;
    if limit < 1 {
        return None;
    }
    let limit = limit.min(MAX_LIMIT);
    let minimal = match arguments.get("detail_level") {
        None => false,
        Some(Value::String(level)) if level == "standard" => false,
        Some(Value::String(level)) if level == "minimal" => true,
        Some(_) => return None,
    };
    if !keyword_only(context, &args)? {
        return None;
    }
    let root = explicit_repo(context, args.optional_string("repo_root")?)?;
    if !matches!(detect_vcs(&root), Vcs::Git | Vcs::None) {
        return None;
    }
    let graph = open_graph(&root)?;
    let store = &graph.store;
    if !stores_no_vectors(store)? {
        return None;
    }

    let stats = store.get_stats().ok()?;
    let answerability = Answerability::recorded(store, &stats)?;
    let hits = fts_search(store, query, kind, limit)?;

    let health = json!({
        "status": "provider_unavailable",
        "requested_provider": null,
        "requested_model": null,
        "requested_text_mode": embedding_text_mode(query),
        "resolved_provider": null,
        "resolved_provider_key": null,
        "auto_resolved_provider": null,
        "matching_vector_count": 0,
        "provider_counts": {},
    });
    let missing_embeddings = json!({
        "reason_code": "missing_embeddings",
        "severity": "medium",
        "claim_effect": "semantic ranking may be keyword-only",
    });
    let mut missingness = answerability.missingness();
    missingness.push(missing_embeddings.clone());

    let result_count = hits.results.len();
    let summary = match kind {
        Some(kind) => format!("Found {result_count} node(s) matching '{query}' (kind={kind})"),
        None => format!("Found {result_count} node(s) matching '{query}'"),
    };
    let exact_count = hits
        .results
        .iter()
        .filter(|r| {
            r.get("name").and_then(Value::as_str) == Some(query)
                || r.get("qualified_name").and_then(Value::as_str) == Some(query)
        })
        .count();
    let next_action = if exact_count == 1 {
        json!({"tool": "query_graph_tool", "suggestion": "fetch live source with pattern=\"source_of\", then callers_of/callees_of"})
    } else if exact_count > 1 {
        json!({"tool": "semantic_search_nodes_tool", "suggestion": format!("choose one qualified name before querying relationships for '{query}'")})
    } else if result_count > 0 {
        json!({"tool": "query_graph_tool", "suggestion": "fetch live source with pattern=\"source_of\" for the chosen qualified_name"})
    } else {
        json!({"tool": "semantic_search_nodes_tool", "suggestion": "broaden the query or verify the graph is up to date"})
    };
    let (guidance, hint) = if result_count > 0 {
        let action = "query_graph_tool pattern=\"source_of\" -- fetch the chosen node's live span";
        (
            json!([{
                "claim": format!("Hybrid search returned {result_count} candidate(s) for '{query}'."),
                "evidence": [{"type": "computed", "query": query, "result_count": result_count, "search_mode": hits.mode}],
                "confidence": "medium",
                "missingness": [missing_embeddings],
                "action": action,
                "reason_codes": ["hybrid_search"],
                "counts": {"result_count": result_count},
            }]),
            hints(action, &["missing_embeddings"]),
        )
    } else {
        let action = "dagayn update -- refresh graph coverage before concluding absence";
        (
            json!([{
                "claim": format!("No nodes matched '{query}' in the current graph."),
                "evidence": [{"type": "computed", "query": query, "search_mode": hits.mode}],
                "confidence": "low",
                "missingness": [
                    missing_embeddings,
                    {
                        "reason_code": "not_found_in_current_graph",
                        "severity": "medium",
                        "claim_effect": "absence is graph-limited, not proof the symbol does not exist",
                    },
                ],
                "action": action,
                "reason_codes": ["zero_result"],
                "counts": {"result_count": 0},
            }]),
            hints(
                action,
                &["missing_embeddings", "not_found_in_current_graph"],
            ),
        )
    };
    let results: Vec<Value> = if minimal {
        hits.results
            .iter()
            .take(5)
            .map(|r| {
                let mut out = Map::new();
                for key in [
                    "name",
                    "kind",
                    "file_path",
                    "qualified_name",
                    "line_start",
                    "line_end",
                    "score",
                ] {
                    if let Some(value) = r.get(key) {
                        out.insert(key.into(), value.clone());
                    }
                }
                let kind = r.get("kind").and_then(Value::as_str).unwrap_or("");
                let path = r
                    .get("file_path")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                let authored = kind.starts_with("Doc")
                    || [".md", ".markdown", ".mdx"]
                        .iter()
                        .any(|ext| path.ends_with(ext));
                out.insert(
                    "evidence_type".into(),
                    json!(if authored { "authored" } else { "extracted" }),
                );
                Value::Object(out)
            })
            .collect()
    } else {
        hits.results.into_iter().map(Value::Object).collect()
    };
    let confidence = if result_count > 0 { "medium" } else { "low" };
    let zero_result_reason = if result_count > 0 {
        Value::Null
    } else {
        json!("not_found_in_current_graph")
    };
    Some(
        Ordered::default()
            .put("status", "ok")
            .put("query", query)
            .put("search_mode", hits.mode)
            .put("embedding_health", health)
            .put("answerability", answerability.full())
            .put("missingness", json!(missingness))
            .put("result_count", result_count)
            .put("truncated", hits.truncated)
            .put("total", hits.total)
            .put("confidence", confidence)
            .put("zero_result_reason", zero_result_reason)
            .put("next_action", next_action.clone())
            .put(
                "exactness",
                json!({
                    "exact_match_count": exact_count,
                    "ambiguity": if exact_count > 1 { json!("multiple_exact_matches") } else { Value::Null },
                    "source_arm": hits.mode,
                    "next_action": next_action,
                }),
            )
            .put("summary", summary)
            .put("results", json!(results))
            .put("guidance", guidance)
            .put("_hints", hint)
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}

/// Whether no provider or model is in play for a search with these
/// arguments (`model`, `provider`), the server defaults, and the
/// environment; any of them embeds the query, which is Python's. `None` for
/// arguments fastmcp would coerce.
pub(crate) fn keyword_only(context: &Context, args: &Args) -> Option<bool> {
    let named = |key: &str| -> Option<bool> {
        Some(args.optional_string(key)?.is_some_and(|v| !v.is_empty()))
    };
    Some(
        !(named("model")?
            || named("provider")?
            || context.embedding_provider.is_some()
            || context.embedding_model.is_some()
            || provider_in_environment()),
    )
}

/// Whether the graph stores no vectors: stored ones can name a provider that
/// `provider_from_persisted_name` revives without any configuration.
pub(crate) fn stores_no_vectors(store: &GraphStore) -> Option<bool> {
    Some(
        store
            .embedding_provider_counts()
            .ok()?
            .is_none_or(|counts| counts.is_empty()),
    )
}

/// `hybrid_search(store, query, limit=1)["results"][0]["qualified_name"]`
/// with an empty embedding arm; `Some(None)` when nothing matched.
pub(crate) fn top_qualified_name(store: &GraphStore, query: &str) -> Option<Option<String>> {
    let hits = fts_search(store, query, None, 1)?;
    Some(hits.results.first().and_then(|hit| {
        hit.get("qualified_name")
            .and_then(Value::as_str)
            .map(str::to_string)
    }))
}

/// `get_provider(None)` / `_infer_remote_embedding_provider_from_env`: any
/// provider configured in the environment.
fn provider_in_environment() -> bool {
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    (set("CRG_OPENAI_API_KEY") && set("CRG_OPENAI_BASE_URL") && set("CRG_OPENAI_MODEL"))
        || set("GOOGLE_API_KEY")
        || set("MINIMAX_API_KEY")
}

#[cfg(test)]
mod tests {
    use super::{
        embedding_text_mode, extract_identifiers, kind_boosts, qualified_name_matches, rrf_merge,
    };

    #[test]
    fn identifiers_are_snake_or_mixed_case() {
        assert_eq!(
            extract_identifiers("tests for embed_graph and GraphStore the a"),
            ["embed_graph", "GraphStore"]
        );
        assert!(extract_identifiers("find all functions").is_empty());
    }

    #[test]
    fn process_queries_select_the_narrative_partition() {
        assert_eq!(
            embedding_text_mode("how the parser builds nodes"),
            "narrative"
        );
        assert_eq!(embedding_text_mode("parse_file builds nodes"), "material");
        assert_eq!(embedding_text_mode("usage guide builds"), "material");
    }

    #[test]
    fn kind_boosts_follow_the_query_shape() {
        assert_eq!(kind_boosts("GraphStore"), [("Class", 1.5), ("Type", 1.5)]);
        assert_eq!(kind_boosts("get_users"), [("Function", 1.5)]);
        assert_eq!(kind_boosts("HTTP"), []);
        assert_eq!(
            kind_boosts("api.get_users"),
            [("Function", 1.5), ("_qualified", 2.0)]
        );
        assert!(qualified_name_matches(
            "api.get_users",
            "src/api.py::get_users"
        ));
        assert!(!qualified_name_matches(
            "api.other",
            "src/api.py::get_users"
        ));
    }

    #[test]
    fn rrf_keeps_first_seen_order_on_ties() {
        let merged = rrf_merge(&[&[(1, 0.0), (2, 0.0)], &[(2, 0.0), (1, 0.0)]]);
        assert_eq!(merged.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [1, 2]);
    }
}
