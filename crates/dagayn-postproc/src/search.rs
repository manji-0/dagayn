use std::collections::{HashMap, HashSet};

use dagayn_graph::{GraphError, GraphNode, GraphStore, Result};
use regex::Regex;
use serde_json::{json, Value};

const TEST_DEBOOST: f64 = 0.6;

pub fn rrf_merge(result_lists: &[Vec<(i64, f64)>], k: i64) -> Vec<(i64, f64)> {
    let k = k.max(1) as f64;
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for result_list in result_lists {
        for (rank, (item_id, _score)) in result_list.iter().enumerate() {
            *scores.entry(*item_id).or_default() += 1.0 / (k + rank as f64 + 1.0);
        }
    }
    let mut merged: Vec<(i64, f64)> = scores.into_iter().collect();
    merged.sort_by(|left, right| right.1.total_cmp(&left.1));
    merged
}

pub fn hybrid_search_json(
    store: &GraphStore,
    query: &str,
    emb_hits_json: &str,
    embedding_health_json: &str,
    kind: &str,
    limit: i64,
    context_files_json: &str,
    _provider: &str,
    _model: &str,
) -> Result<String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(json!({
            "mode": "empty",
            "results": [],
            "embedding_health": parse_json_value(embedding_health_json, json!({"status": "not_requested"})),
            "truncated": false,
            "total": 0,
        })
        .to_string());
    }

    let emb_results = parse_hit_pairs(emb_hits_json)?;
    let embedding_health = parse_json_value(
        embedding_health_json,
        json!({"status": "not_requested"}),
    );
    let context_files: Vec<String> = serde_json::from_str(context_files_json).unwrap_or_default();
    let kind_filter = if kind.is_empty() { None } else { Some(kind.to_string()) };
    let limit = limit.max(1);

    let fts_health: Value =
        serde_json::from_str(&store.fts_index_health_json()?).unwrap_or_else(|_| json!({}));
    let query_tokens = query_tokens(query);
    let rerank_intent = query_rerank_intent(query, &query_tokens);

    let mut fetch_multiplier = if kind_filter.is_some() { 12 } else { 3 };
    let max_fetch_multiplier = if kind_filter.is_some() { 48 } else { 9 };

    let mut fts_results = Vec::new();
    let mut keyword_results = Vec::new();
    let mut fts_match_mode: Option<String> = None;
    let mut mode = "empty".to_string();
    let mut keyword_mode = false;
    let mut merged = Vec::new();

    while fetch_multiplier <= max_fetch_multiplier {
        let fetch_limit = limit * fetch_multiplier as i64;
        fts_results.clear();
        keyword_results.clear();
        let mut fts_match_modes = Vec::new();

        if let Ok(primary) = store.fts_query(query, fetch_limit) {
            fts_results.extend(primary.hits);
            if primary.match_mode != "none" {
                fts_match_modes.push(primary.match_mode);
            }
            for ident in extract_identifiers(query) {
                if let Ok(extra) = store.fts_query(&ident, fetch_limit) {
                    if !extra.hits.is_empty() {
                        fts_results.extend(extra.hits);
                        if extra.match_mode != "none" {
                            fts_match_modes.push(extra.match_mode);
                        }
                    }
                }
            }
        }

        fts_match_mode = if fts_match_modes.iter().any(|mode| mode == "and") {
            Some("and".to_string())
        } else if fts_match_modes.iter().any(|mode| mode == "or") {
            Some("any".to_string())
        } else if fts_match_modes.iter().any(|mode| mode == "phrase") {
            Some("phrase".to_string())
        } else {
            None
        };

        if !fts_results.is_empty() {
            let ids: Vec<i64> = fts_results.iter().map(|(id, _)| *id).collect();
            let valid_ids: HashSet<i64> = store.get_nodes_by_ids(&ids)?.into_keys().collect();
            fts_results.retain(|(id, _)| valid_ids.contains(id));
        }

        keyword_mode = false;
        if fts_results.is_empty() && emb_results.is_empty() {
            keyword_results = store.keyword_query(query, fetch_limit)?;
            if keyword_results.is_empty() {
                return Ok(json!({
                    "mode": "empty",
                    "results": [],
                    "embedding_health": embedding_health,
                    "fts_health": fts_health,
                    "rerank_intent": rerank_intent,
                    "truncated": false,
                    "total": 0,
                })
                .to_string());
            }
            merged = keyword_results.clone();
            mode = "keyword_fallback".to_string();
            keyword_mode = true;
        } else {
            let mut lists = Vec::new();
            if !fts_results.is_empty() {
                lists.push(fts_results.clone());
            }
            if !emb_results.is_empty() {
                lists.push(emb_results.clone());
            }
            merged = rrf_merge(&lists, 10);
            if fts_match_mode.as_deref() == Some("any") {
                keyword_results = store.keyword_query(query, fetch_limit)?;
                if !keyword_results.is_empty() {
                    merged = rrf_merge(&[merged, keyword_results.clone()], 10);
                }
            }
            mode = if keyword_mode {
                "keyword_fallback".to_string()
            } else if !fts_results.is_empty() && !emb_results.is_empty() {
                "hybrid".to_string()
            } else if !fts_results.is_empty() {
                "fts_only".to_string()
            } else {
                "embedding_only".to_string()
            };
        }

        if kind_filter.is_none() {
            break;
        }
        let candidate_ids: Vec<i64> = merged.iter().map(|(id, _)| *id).collect();
        let node_map = store.get_nodes_by_ids(&candidate_ids)?;
        let kind_hits = candidate_ids
            .iter()
            .filter(|node_id| {
                node_map.get(node_id).is_some_and(|node| {
                    kind_filter
                        .as_deref()
                        .is_some_and(|kind| node.kind == kind)
                })
            })
            .count();
        if kind_hits as i64 >= limit || fetch_multiplier >= max_fetch_multiplier {
            break;
        }
        fetch_multiplier *= 2;
    }

    if merged.is_empty() {
        return Ok(json!({
            "mode": "empty",
            "results": [],
            "embedding_health": embedding_health,
            "fts_health": fts_health,
            "rerank_intent": rerank_intent,
            "truncated": false,
            "total": 0,
        })
        .to_string());
    }

    let fts_ids: HashSet<i64> = fts_results.iter().map(|(id, _)| *id).collect();
    let emb_ids: HashSet<i64> = emb_results.iter().map(|(id, _)| *id).collect();
    let keyword_ids: HashSet<i64> = keyword_results.iter().map(|(id, _)| *id).collect();
    let fts_rank_by_id = rank_map(&fts_results);
    let emb_rank_by_id = rank_map(&emb_results);
    let kind_boosts = detect_query_kind_boost(query);
    let context_set: HashSet<String> = context_files.into_iter().collect();
    let hybrid_mode = !fts_results.is_empty() && !emb_results.is_empty();

    let candidate_ids: Vec<i64> = merged.iter().map(|(id, _)| *id).collect();
    let node_map = store.get_nodes_by_ids(&candidate_ids)?;

    let mut boosted = Vec::new();
    for (node_id, score) in merged {
        let Some(node) = node_map.get(&node_id) else {
            continue;
        };
        let mut boost = 1.0;
        if let Some(kind_boost) = kind_boosts.get(node.kind.as_str()) {
            boost *= kind_boost;
        }
        if kind_boosts.contains_key("_qualified")
            && qualified_name_matches(query, &node.qualified_name)
        {
            boost *= kind_boosts["_qualified"];
        }
        if context_set.contains(&node.file_path) {
            boost *= 1.5;
        }
        boost *= intent_boost(
            &query_tokens,
            node,
            fts_rank_by_id.get(&node_id).copied(),
            emb_rank_by_id.get(&node_id).copied(),
            hybrid_mode,
            &rerank_intent,
        );
        if node.is_test && !query_tokens.contains("test") && !query_tokens.contains("tests") {
            if !query_tokens.contains("coverage") && !query_tokens.contains("proves") {
                boost *= TEST_DEBOOST;
            }
        }
        boosted.push((node_id, score * boost));
    }
    boosted.sort_by(|left, right| right.1.total_cmp(&left.1));

    let mut eligible = Vec::new();
    for (node_id, score) in boosted {
        let Some(node) = node_map.get(&node_id) else {
            continue;
        };
        if let Some(kind) = &kind_filter {
            if node.kind != *kind {
                continue;
            }
        }
        eligible.push((node_id, score));
    }

    let signatures = load_signatures(store, &eligible.iter().map(|(id, _)| *id).collect::<Vec<_>>())?;
    let mut results = Vec::new();
    for (node_id, final_score) in eligible.iter().take(limit as usize) {
        let Some(node) = node_map.get(node_id) else {
            continue;
        };
        let source = if node.kind == "DocSection" {
            "doc"
        } else if keyword_mode || keyword_ids.contains(node_id) {
            "keyword"
        } else if fts_ids.contains(node_id) && emb_ids.contains(node_id) {
            "both"
        } else if fts_ids.contains(node_id) {
            "fts"
        } else {
            "embedding"
        };
        results.push(json!({
            "name": sanitize_name(&node.name),
            "qualified_name": sanitize_name(&node.qualified_name),
            "kind": node.kind,
            "file_path": node.file_path,
            "line_start": node.line_start,
            "line_end": node.line_end,
            "language": node.language,
            "params": node.params,
            "return_type": node.return_type,
            "signature": signatures.get(node_id),
            "score": (final_score * 1_000_000.0).round() / 1_000_000.0,
            "rank": results.len() as i64 + 1,
            "source": source,
            "is_test": node.is_test,
        }));
    }

    let total = eligible.len() as i64;
    let mut response = json!({
        "mode": mode,
        "results": results,
        "embedding_health": embedding_health,
        "fts_health": fts_health,
        "rerank_intent": rerank_intent,
        "truncated": total > limit,
        "total": total,
    });
    if let Some(match_mode) = fts_match_mode {
        response["fts_match_mode"] = json!(match_mode);
    }
    serde_json::to_string(&response).map_err(GraphError::from)
}

fn parse_hit_pairs(raw: &str) -> Result<Vec<(i64, f64)>> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(raw).map_err(GraphError::from)?;
    let Some(items) = value.as_array() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for item in items {
        if let Some(pair) = item.as_array() {
            if pair.len() >= 2 {
                if let (Some(id), Some(score)) = (pair[0].as_i64(), pair[1].as_f64()) {
                    out.push((id, score));
                }
            }
        }
    }
    Ok(out)
}

fn parse_json_value(raw: &str, default: Value) -> Value {
    if raw.trim().is_empty() {
        return default;
    }
    serde_json::from_str(raw).unwrap_or(default)
}

fn rank_map(hits: &[(i64, f64)]) -> HashMap<i64, i64> {
    let mut out = HashMap::new();
    for (rank, (node_id, _)) in hits.iter().enumerate() {
        out.entry(*node_id).or_insert((rank + 1) as i64);
    }
    out
}

fn load_signatures(store: &GraphStore, node_ids: &[i64]) -> Result<HashMap<i64, Option<String>>> {
    store.get_node_signatures_by_ids(node_ids)
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| *ch == '\t' || *ch == '\n' || (*ch as u32) >= 0x20)
        .take(256)
        .collect()
}

fn query_tokens(query: &str) -> HashSet<String> {
    regex_tokens(query)
        .into_iter()
        .map(|token| token.to_lowercase())
        .collect()
}

fn regex_tokens(query: &str) -> Vec<String> {
    static TOKEN_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"[A-Za-z0-9_]+").expect("valid token regex"));
    TOKEN_RE
        .find_iter(query)
        .map(|m| m.as_str().to_string())
        .collect()
}

fn extract_identifiers(query: &str) -> Vec<String> {
    static IDENT_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"[A-Za-z_][A-Za-z0-9_]+").expect("valid identifier regex")
    });
    let stopwords = query_stopwords();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for candidate in IDENT_RE.find_iter(query) {
        let c = candidate.as_str();
        if stopwords.contains(&c.to_lowercase()) {
            continue;
        }
        let is_snake = c.contains('_');
        let is_camelish = c.chars().skip(1).any(|ch| ch.is_uppercase());
        if !(is_snake || is_camelish) {
            continue;
        }
        if seen.insert(c.to_string()) {
            out.push(c.to_string());
        }
    }
    out
}

fn query_rerank_intent(query: &str, query_tokens: &HashSet<String>) -> String {
    let stripped = query.trim();
    if stripped.is_empty() {
        return "empty".to_string();
    }
    if stripped.contains('.') || stripped.contains("::") || !extract_identifiers(stripped).is_empty() {
        return "exact".to_string();
    }
    if query_tokens.intersection(&doc_intent_terms()).next().is_some() {
        return "documentation".to_string();
    }
    if query_tokens.intersection(&process_pattern_terms()).next().is_some() {
        return "process_pattern".to_string();
    }
    if regex_tokens(stripped).len() >= 2 || query_tokens.intersection(&purpose_query_terms()).next().is_some() {
        return "purpose".to_string();
    }
    "exact".to_string()
}

fn detect_query_kind_boost(query: &str) -> HashMap<String, f64> {
    let mut boosts = HashMap::new();
    let q = query.trim();
    if q.is_empty() {
        return boosts;
    }
    if q.chars().next().is_some_and(|ch| ch.is_uppercase())
        && q.chars().skip(1).any(|ch| ch.is_lowercase())
        && !q.chars().all(|ch| ch.is_uppercase())
    {
        boosts.insert("Class".to_string(), 1.5);
        boosts.insert("Type".to_string(), 1.5);
    }
    if q.contains('_') && q.chars().any(|ch| ch.is_alphabetic()) {
        boosts.insert("Function".to_string(), 1.5);
    }
    if q.contains('.') {
        boosts.insert("_qualified".to_string(), 2.0);
    }
    boosts
}

fn qualified_name_matches(query: &str, qualified_name: &str) -> bool {
    let q = query.to_lowercase();
    let qn = qualified_name.to_lowercase();
    if qn.contains(&q) {
        return true;
    }
    let q_tokens: Vec<&str> = q
        .split(['.', '/', ':'])
        .filter(|token| !token.is_empty())
        .collect();
    if q_tokens.is_empty() {
        return false;
    }
    let qn_tokens: Vec<&str> = qn
        .split(['.', '/', ':'])
        .filter(|token| !token.is_empty())
        .collect();
    let mut i = 0usize;
    for tok in qn_tokens {
        if i < q_tokens.len() && tok == q_tokens[i] {
            i += 1;
        }
    }
    i == q_tokens.len()
}

fn intent_boost(
    query_tokens: &HashSet<String>,
    node: &GraphNode,
    fts_rank: Option<i64>,
    emb_rank: Option<i64>,
    hybrid_mode: bool,
    rerank_intent: &str,
) -> f64 {
    if !hybrid_mode {
        return 1.0;
    }
    let mut boost = 1.0;
    let code_intent = query_tokens.intersection(&code_intent_terms()).next().is_some();
    let doc_intent = query_tokens.intersection(&doc_intent_terms()).next().is_some();
    let test_intent = query_tokens.contains("test")
        || query_tokens.contains("tests")
        || query_tokens.contains("coverage")
        || query_tokens.contains("proves");
    let markdown_node =
        node.kind == "DocSection" || node.file_path.to_lowercase().ends_with(".md");
    let code_node = matches!(node.kind.as_str(), "Function" | "Class" | "Type" | "Test");

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

    match rerank_intent {
        "process_pattern" => {
            if emb_rank.is_some() {
                boost *= 1.55;
                if emb_rank.is_some_and(|rank| rank <= 5) {
                    boost *= 1.35;
                } else if emb_rank.is_some_and(|rank| rank <= 20) {
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
        }
        "purpose" => {
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
        _ => {}
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
    if test_intent
        && (node.is_test || node.file_path.starts_with("tests/"))
    {
        boost *= 1.55;
    }

    let name_terms = split_identifier_terms(&node.name);
    if !name_terms.is_empty() && name_terms.is_subset(query_tokens) {
        boost *= if matches!(node.kind.as_str(), "Function" | "Class" | "Type" | "Test") {
            1.70
        } else {
            1.30
        };
    }
    boost
}

fn split_identifier_terms(value: &str) -> HashSet<String> {
    static BOUNDARY_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])").expect("valid boundary regex")
    });
    BOUNDARY_RE
        .replace_all(value, " ")
        .replace('_', " ")
        .split_whitespace()
        .map(|part| part.to_lowercase())
        .filter(|part| !part.is_empty())
        .collect()
}

fn query_stopwords() -> HashSet<String> {
    [
        "the", "a", "an", "is", "are", "for", "of", "to", "in", "on", "by", "with", "from", "and",
        "or", "not", "where", "how", "what", "which", "find", "show", "list", "all", "any", "this",
        "that", "these", "those", "it", "we", "do", "does", "did",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn code_intent_terms() -> HashSet<String> {
    [
        "code", "function", "implementation", "implements", "logic", "helper", "wrapper", "path",
        "handler", "method", "class", "rust", "python", "typescript", "test", "tests",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn doc_intent_terms() -> HashSet<String> {
    ["documentation", "readme", "usage", "guide", "section", "instructions"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn process_pattern_terms() -> HashSet<String> {
    [
        "assigns", "branches", "builds", "calls", "computes", "converts", "creates", "deletes",
        "detects", "embedding", "embeddings", "embeds", "fetches", "filters", "inserts", "iterates",
        "loads", "loops", "merges", "opens", "parses", "queries", "ranks", "reads", "rebuilds",
        "renders", "returns", "searches", "stores", "tested", "updates", "uses", "validates",
        "writes",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn purpose_query_terms() -> HashSet<String> {
    [
        "behavior", "feature", "goal", "handles", "logic", "purpose", "responsible", "supports",
        "workflow",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_merge_combines_ranked_lists() {
        let merged = rrf_merge(
            &[
                vec![(1, 1.0), (2, 0.5)],
                vec![(2, 0.9), (3, 0.4)],
            ],
            10,
        );
        assert_eq!(merged[0].0, 2);
        assert!(merged[0].1 > 0.0);
    }

    #[test]
    fn extract_identifiers_finds_snake_and_camel() {
        let ids = extract_identifiers("tests for embed_graph and GraphStore");
        assert!(ids.contains(&"embed_graph".to_string()));
        assert!(ids.contains(&"GraphStore".to_string()));
    }
}
