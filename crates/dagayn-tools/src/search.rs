//! `semantic_search_nodes_tool` (`dagayn.tools.query.semantic_search_nodes`
//! over `dagayn.search.hybrid_search`) when no embedding provider takes part:
//! nothing in the arguments, the server defaults, or the environment selects
//! one, and the graph stores no vectors a persisted provider name could
//! revive. The embedding arm then reports `provider_unavailable` and the
//! answer is the FTS (or keyword) arm alone, which is reproduced here.
//! Anything that would embed the query is Python's.

use std::collections::{HashMap, HashSet};

use dagayn_graph::{GraphNode, GraphStore};
use serde_json::{Map, Value, json};

use crate::query::sanitize;
use crate::suggestions::round_to;
use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo};

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
const PURPOSE_TERMS: &[&str] = &[
    "behavior",
    "feature",
    "goal",
    "handles",
    "logic",
    "purpose",
    "responsible",
    "supports",
    "workflow",
];
const CODE_INTENT: &[&str] = &[
    "code",
    "function",
    "implementation",
    "implements",
    "logic",
    "helper",
    "wrapper",
    "path",
    "handler",
    "method",
    "class",
    "rust",
    "python",
    "typescript",
    "test",
    "tests",
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

/// `_query_rerank_intent`.
fn rerank_intent(query: &str) -> &'static str {
    let stripped = query.trim();
    let query_tokens: HashSet<String> = tokens(query).iter().map(|t| t.to_lowercase()).collect();
    let intersects = |terms: &[&str]| terms.iter().any(|term| query_tokens.contains(*term));
    if stripped.is_empty() {
        return "empty";
    }
    if stripped.contains('.')
        || stripped.contains("::")
        || !extract_identifiers(stripped).is_empty()
    {
        return "exact";
    }
    if intersects(DOC_INTENT) {
        return "documentation";
    }
    if intersects(PROCESS_PATTERN) {
        return "process_pattern";
    }
    if tokens(stripped).len() >= 2 || intersects(PURPOSE_TERMS) {
        return "purpose";
    }
    "exact"
}

/// `_embedding_text_mode_for_intent`: only `process_pattern` selects the
/// narrative partition.
fn embedding_text_mode(query: &str) -> &'static str {
    if rerank_intent(query) == "process_pattern" {
        "narrative"
    } else {
        "material"
    }
}

/// `_split_identifier_terms`: `snake_case`, `camelCase`, and `HTTPServer`
/// boundaries, lowercased.
fn identifier_terms(name: &str) -> HashSet<String> {
    let chars: Vec<char> = name.replace('_', " ").chars().collect();
    let mut spaced = String::new();
    for (index, c) in chars.iter().enumerate() {
        if index > 0 && c.is_ascii_uppercase() {
            let prev = chars[index - 1];
            let lower_or_digit = prev.is_ascii_lowercase() || prev.is_ascii_digit();
            let acronym_end = prev.is_ascii_uppercase()
                && chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
            if lower_or_digit || acronym_end {
                spaced.push(' ');
            }
        }
        spaced.push(*c);
    }
    spaced.split_whitespace().map(str::to_lowercase).collect()
}

/// `_intent_boost` in hybrid mode (1.0 otherwise), multiplied in Python's
/// order so the product rounds the same.
fn intent_boost(
    query_tokens: &HashSet<String>,
    node: &GraphNode,
    fts_rank: Option<usize>,
    emb_rank: Option<usize>,
    intent: &str,
) -> f64 {
    let has = |terms: &[&str]| terms.iter().any(|term| query_tokens.contains(*term));
    let code_intent = has(CODE_INTENT);
    let doc_intent = has(DOC_INTENT);
    let test_intent = has(&["test", "tests", "coverage", "proves"]);
    let markdown_node = node.kind == "DocSection" || node.file_path.to_lowercase().ends_with(".md");
    let code_node = matches!(node.kind.as_str(), "Function" | "Class" | "Type" | "Test");
    let mut boost = 1.0_f64;
    if fts_rank.is_some_and(|rank| rank <= 3) {
        boost *= 1.25;
    }
    if fts_rank == Some(1) {
        boost *= 1.15;
    }
    if emb_rank == Some(1) {
        boost *= 1.30;
    }
    if fts_rank.is_some() && emb_rank.is_some() {
        boost *= 1.15;
    }
    if intent == "process_pattern" {
        if let Some(rank) = emb_rank {
            boost *= 1.55;
            if rank <= 5 {
                boost *= 1.35;
            } else if rank <= 20 {
                boost *= 1.15;
            }
        }
        if code_node {
            boost *= 1.60;
        }
        if node.kind == "Function" {
            boost *= 1.25;
        }
        if markdown_node {
            boost *= 0.18;
        }
        if node.is_test && !test_intent {
            boost *= 0.55;
        }
    } else if intent == "purpose" {
        if fts_rank.is_some() && emb_rank.is_some() {
            boost *= 1.40;
        } else if emb_rank.is_some_and(|rank| rank <= 5) {
            boost *= 1.15;
        }
        if code_node {
            boost *= 1.10;
        }
        if markdown_node && !doc_intent {
            boost *= 0.75;
            if code_intent {
                boost *= 0.55;
            }
        }
    }
    if code_intent && !doc_intent {
        if markdown_node {
            boost *= 0.45;
        } else if code_node {
            boost *= 1.18;
        }
    }
    if doc_intent {
        if node.kind == "DocSection" {
            boost *= 1.35;
        } else if markdown_node {
            boost *= 1.15;
        }
    }
    if test_intent && (node.is_test || node.file_path.starts_with("tests/")) {
        boost *= 1.55;
    }
    let name_terms = identifier_terms(&node.name);
    if !name_terms.is_empty() && name_terms.iter().all(|term| query_tokens.contains(term)) {
        boost *= if code_node { 1.70 } else { 1.30 };
    }
    boost
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

struct Hits {
    mode: &'static str,
    results: Vec<Map<String, Value>>,
    truncated: bool,
    total: usize,
}

/// `hybrid_search` given the embedding arm's hits (`all_emb`, empty when the
/// arm had none); `None` when a query would fail in a way Python only logs.
fn fts_search(
    store: &GraphStore,
    query: &str,
    kind: Option<&str>,
    limit: i64,
    all_emb: &[(i64, f64)],
) -> Option<Hits> {
    let empty = Hits {
        mode: "empty",
        results: Vec::new(),
        truncated: false,
        total: 0,
    };
    let (mut multiplier, max_multiplier) = if kind.is_some() { (12, 48) } else { (3, 9) };
    let mut merged: Vec<(i64, f64)>;
    let mut fts_results: Vec<(i64, f64)>;
    let mut emb_results: &[(i64, f64)];
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
        emb_results = &all_emb[..all_emb.len().min(fetch_limit.max(0) as usize)];
        keyword_mode = false;
        if !fts_results.is_empty() || !emb_results.is_empty() {
            let mut lists: Vec<&[(i64, f64)]> = Vec::new();
            if !fts_results.is_empty() {
                lists.push(fts_results.as_slice());
            }
            if !emb_results.is_empty() {
                lists.push(emb_results);
            }
            merged = rrf_merge(&lists);
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
        } else if !fts_results.is_empty() && !emb_results.is_empty() {
            "hybrid"
        } else if !fts_results.is_empty() {
            "fts_only"
        } else {
            "embedding_only"
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
    let emb_ids: HashSet<i64> = emb_results.iter().map(|(id, _)| *id).collect();
    let keyword_ids: HashSet<i64> = keyword_results.iter().map(|(id, _)| *id).collect();
    let first_ranks = |hits: &[(i64, f64)]| {
        let mut ranks: HashMap<i64, usize> = HashMap::new();
        for (rank, (id, _)) in hits.iter().enumerate() {
            ranks.entry(*id).or_insert(rank + 1);
        }
        ranks
    };
    let fts_rank = first_ranks(&fts_results);
    let emb_rank = first_ranks(emb_results);
    let hybrid_mode = !fts_results.is_empty() && !emb_results.is_empty();
    let intent = rerank_intent(query);
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
        if hybrid_mode {
            boost *= intent_boost(
                &query_tokens,
                node,
                fts_rank.get(id).copied(),
                emb_rank.get(id).copied(),
                intent,
            );
        }
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
        } else if fts_ids.contains(id) && emb_ids.contains(id) {
            "both"
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
        result.insert("score".into(), json!(round_to(*score, 6)));
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
    let limit = args.integer("limit", 20)?;
    // Checked first, before the store is opened: no `_repo`.
    if limit < 1 {
        let message = format!("limit must be an integer >= 1 (got {limit})");
        return Some(
            Ordered::default()
                .put("status", "error")
                .put("error", message.clone())
                .put("summary", message)
                .put("limit", limit)
                .into_payload(),
        );
    }
    if query.trim().is_empty() {
        return None;
    }
    let kind = args.optional_string("kind")?;
    let limit = limit.min(MAX_LIMIT);
    let minimal = match arguments.get("detail_level") {
        None => false,
        Some(Value::String(level)) if level == "standard" => false,
        Some(Value::String(level)) if level == "minimal" => true,
        Some(_) => return None,
    };
    let request = embedding_request(context, &args)?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let graph = open_graph(&root)?;
    let store = &graph.store;

    let answerability = graph.answerability()?;
    let fetch = limit * if kind.is_some() { 48 } else { 3 };
    let (emb, health) = embedding_arm_for(store, &graph.db_path, query, fetch, &request)?;
    let hits = fts_search(store, query, kind, limit, &emb)?;

    // `embedding_health_available` and `_partial_coverage_missingness`.
    let mut arm_missing: Vec<Value> = Vec::new();
    if !matches!(
        health.get("status").and_then(Value::as_str),
        Some("available" | "degraded")
    ) {
        arm_missing.push(json!({
            "reason_code": "missing_embeddings",
            "severity": "medium",
            "claim_effect": "semantic ranking may be keyword-only",
        }));
    }
    if health.get("partial_coverage") == Some(&Value::Bool(true)) {
        arm_missing.push(json!({
            "reason_code": "partial_embeddings",
            "severity": "medium",
            "claim_effect": "semantic ranking covers only part of the graph, so a node's absence from these results is not evidence it is irrelevant",
            "details": {
                "embedding_coverage": health.get("embedding_coverage"),
                "missing_embedding_count": health.get("missing_embedding_count"),
            },
        }));
    }
    let mut missingness = answerability.missingness();
    missingness.extend(arm_missing.iter().cloned());
    let arm_codes: Vec<&str> = arm_missing
        .iter()
        .filter_map(|item| item.get("reason_code").and_then(Value::as_str))
        .collect();

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
        let missing = if arm_missing.is_empty() {
            json!([{
                "reason_code": "ranking_is_evidence_not_verdict",
                "severity": "low",
                "claim_effect": "scores rank leads; fetch source_of for the chosen qualified_name, then callers_of if needed",
            }])
        } else {
            json!(arm_missing)
        };
        (
            json!([{
                "claim": format!("Hybrid search returned {result_count} candidate(s) for '{query}'."),
                "evidence": [{"type": "computed", "query": query, "result_count": result_count, "search_mode": hits.mode}],
                "confidence": "medium",
                "missingness": missing,
                "action": action,
                "reason_codes": ["hybrid_search"],
                "counts": {"result_count": result_count},
            }]),
            hints(action, &arm_codes),
        )
    } else {
        let action = "dagayn update -- refresh graph coverage before concluding absence";
        let mut missing = arm_missing.clone();
        missing.push(json!({
            "reason_code": "not_found_in_current_graph",
            "severity": "medium",
            "claim_effect": "absence is graph-limited, not proof the symbol does not exist",
        }));
        let mut codes = arm_codes.clone();
        codes.push("not_found_in_current_graph");
        (
            json!([{
                "claim": format!("No nodes matched '{query}' in the current graph."),
                "evidence": [{"type": "computed", "query": query, "search_mode": hits.mode}],
                "confidence": "low",
                "missingness": missing,
                "action": action,
                "reason_codes": ["zero_result"],
                "counts": {"result_count": 0},
            }]),
            hints(action, &codes),
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
            .put("next", crate::next::read_hits(&results))
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

/// Which embedding provider a search with these arguments would use: a
/// provider or model named in the call is Python's, and of the server
/// defaults only the `--local-embedding` sidecar's `openai` is answered.
pub(crate) fn embedding_request<'a>(
    context: &'a Context,
    args: &Args,
) -> Option<crate::embedding_arm::Request<'a>> {
    let named = |key: &str| -> Option<bool> {
        Some(args.optional_string(key)?.is_some_and(|v| !v.is_empty()))
    };
    if named("model")? || named("provider")? {
        return None;
    }
    match (
        context.embedding_provider.as_deref(),
        context.embedding_model.as_deref(),
    ) {
        (None, None) => Some(crate::embedding_arm::Request::Persisted),
        (Some(provider), model) if provider.trim().eq_ignore_ascii_case("openai") => {
            Some(crate::embedding_arm::Request::Openai { provider, model })
        }
        _ => None,
    }
}

/// `_embedding_search_with_health` for `hybrid_search(query)` fetching
/// `fetch` vectors: the hits and the health record; `None` for Python.
pub(crate) fn embedding_arm_for(
    store: &GraphStore,
    db_path: &std::path::Path,
    query: &str,
    fetch: i64,
    request: &crate::embedding_arm::Request,
) -> Option<(Vec<(i64, f64)>, Value)> {
    let text_mode = embedding_text_mode(query);
    let counts = store.embedding_provider_counts().ok()?.unwrap_or_default();
    if !counts.is_empty() {
        return crate::embedding_arm::search(
            store, db_path, query, fetch, text_mode, &counts, request,
        );
    }
    // With no vectors stored, only the provider-less answer is here.
    if !matches!(request, crate::embedding_arm::Request::Persisted) || provider_in_environment() {
        return None;
    }
    Some((
        Vec::new(),
        json!({
            "status": "provider_unavailable",
            "requested_provider": null,
            "requested_model": null,
            "requested_text_mode": text_mode,
            "resolved_provider": null,
            "resolved_provider_key": null,
            "auto_resolved_provider": null,
            "matching_vector_count": 0,
            "provider_counts": {},
        }),
    ))
}

/// `hybrid_search(store, query, limit=1)["results"][0]["qualified_name"]`;
/// `Some(None)` when nothing matched, `None` for Python.
pub(crate) fn top_qualified_name(
    store: &GraphStore,
    db_path: &std::path::Path,
    query: &str,
    request: &crate::embedding_arm::Request,
) -> Option<Option<String>> {
    let (emb, _) = embedding_arm_for(store, db_path, query, 3, request)?;
    let hits = fts_search(store, query, None, 1, &emb)?;
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
