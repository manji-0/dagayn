use crate::helpers::*;
use crate::*;

const FTS_SEGMENTER_METADATA_KEY: &str = "fts_segmenter";

const COMMON_FTS_SEGMENTS: &[&str] = &[
    "py", "rs", "ts", "js", "go", "src", "test", "tests", "index", "main", "lib", "mod", "api",
    "app", "util", "utils", "common", "core",
];

#[derive(Clone, Debug)]
pub struct FtsQueryResult {
    pub hits: Vec<(i64, f64)>,
    pub match_mode: String,
}

impl GraphStore {
    pub fn fts_query(&self, query: &str, limit: i64) -> Result<FtsQueryResult> {
        let segmenter = self.get_metadata(FTS_SEGMENTER_METADATA_KEY)?;
        let fts_query = segment_query_text(query, segmenter.as_deref());
        let (safe_query, fallback_query, fallback_mode) = build_fts_match_queries(&fts_query);
        let sql = "SELECT rowid, bm25(nodes_fts, 8.0, 6.0, 3.0, 4.0, 5.0, 1.0) AS score \
                   FROM nodes_fts WHERE nodes_fts MATCH ? ORDER BY score LIMIT ?";

        let mut stmt = match self.conn.prepare(sql) {
            Ok(stmt) => stmt,
            Err(_) => {
                return Ok(FtsQueryResult {
                    hits: Vec::new(),
                    match_mode: "none".to_string(),
                })
            }
        };

        let mut match_mode = if fts_query.split(['.', '/', ':', ' ']).filter(|s| !s.is_empty()).count() > 1 {
            "and".to_string()
        } else {
            "phrase".to_string()
        };

        let mut hits = {
            let mut rows = stmt.query(rusqlite::params![safe_query, limit])?;
            collect_fts_hits(&mut rows)?
        };
        if hits.is_empty() && fallback_query != safe_query {
            let mut fallback_rows = stmt.query(rusqlite::params![fallback_query, limit])?;
            hits = collect_fts_hits(&mut fallback_rows)?;
            match_mode = if hits.is_empty() {
                "none".to_string()
            } else {
                fallback_mode.to_string()
            }
        } else if hits.is_empty() {
            match_mode = "none".to_string();
        }

        Ok(FtsQueryResult { hits, match_mode })
    }

    pub fn keyword_query(&self, query: &str, limit: i64) -> Result<Vec<(i64, f64)>> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(|word| word.to_lowercase())
            .filter(|word| !word.is_empty())
            .collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }

        let rows = if words.iter().all(|word| word.is_ascii()) {
            let mut conditions = Vec::new();
            let mut params: Vec<rusqlite::types::Value> = Vec::new();
            for word in &words {
                conditions.push("(name LIKE ? OR qualified_name LIKE ?)".to_string());
                let pattern = format!("%{word}%");
                params.push(rusqlite::types::Value::Text(pattern.clone()));
                params.push(rusqlite::types::Value::Text(pattern));
            }
            let where_clause = conditions.join(" AND ");
            let sql = format!("SELECT id, name FROM nodes WHERE {where_clause} LIMIT ?");
            params.push(rusqlite::types::Value::Integer(limit * 4));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            let mut stmt = self.conn.prepare("SELECT id, name FROM nodes")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let q_lower = query.to_lowercase();
        let mut results = Vec::new();
        for (node_id, name) in rows {
            let name_lower = name.to_lowercase();
            if !words.iter().all(|word| name_lower.contains(word)) {
                continue;
            }
            let score: f64 = if name_lower == q_lower {
                3.0
            } else if name_lower.starts_with(&q_lower) {
                2.0
            } else {
                1.0
            };
            results.push((node_id, score));
        }
        results.sort_by(|left, right| right.1.total_cmp(&left.1));
        results.truncate(limit as usize);
        Ok(results)
    }

    pub fn fts_index_health_json(&self) -> Result<String> {
        let nodes_count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM nodes", [], |row| row.get(0))?;
        let fts_count = if table_exists(&self.conn, "nodes_fts")? {
            self.conn
                .query_row("SELECT count(*) FROM nodes_fts", [], |row| row.get(0))?
        } else {
            0
        };
        let status = if fts_count == 0 {
            "missing"
        } else if fts_count < nodes_count {
            "partial"
        } else {
            "ready"
        };
        let built_at = self.get_metadata(crate::fts_sync::FTS_BUILT_AT_KEY)?;
        let payload = serde_json::json!({
            "status": status,
            "nodes_count": nodes_count,
            "fts_count": fts_count,
            "built_at": built_at,
        });
        serde_json::to_string(&payload).map_err(Into::into)
    }
}

fn segment_query_text(query: &str, _segmenter: Option<&str>) -> String {
    segment_japanese_fts_text(query)
}

fn build_fts_match_queries(fts_query: &str) -> (String, String, &'static str) {
    let segments: Vec<&str> = fts_query
        .split(['.', '/', ':', ' '])
        .filter(|segment| !segment.is_empty())
        .collect();
    let quoted_segments: Vec<String> = segments
        .iter()
        .map(|segment| format!("\"{}\"", segment.replace('"', "\"\"")))
        .collect();

    if quoted_segments.len() > 1 {
        let safe_query = quoted_segments.join(" AND ");
        let anchor = most_selective_segment(&segments);
        let quoted_anchor = format!("\"{}\"", anchor.replace('"', "\"\""));
        let fallback_query = format!("({}) AND {}", quoted_segments.join(" OR "), quoted_anchor);
        (safe_query, fallback_query, "or")
    } else {
        let safe_query = format!("\"{}\"", fts_query.replace('"', "\"\""));
        (safe_query.clone(), safe_query, "phrase")
    }
}

fn most_selective_segment(segments: &[&str]) -> String {
    segments
        .iter()
        .max_by_key(|segment| {
            (
                !COMMON_FTS_SEGMENTS.contains(segment),
                segment.len(),
                *segment,
            )
        })
        .map(|segment| segment.to_string())
        .unwrap_or_default()
}

fn collect_fts_hits(
    rows: &mut rusqlite::Rows<'_>,
) -> std::result::Result<Vec<(i64, f64)>, rusqlite::Error> {
    let mut hits = Vec::new();
    while let Some(row) = rows.next()? {
        let node_id: i64 = row.get(0)?;
        let score: f64 = row.get(1)?;
        hits.push((node_id, -score));
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_store_with_fts() -> GraphStore {
        let mut path = std::env::temp_dir();
        path.push(format!("dagayn-search-{}-{}.db", "fts", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut store = GraphStore::open(&path).expect("open graph store");
        store
            .store_file_batch(&[(
                "api.py".to_string(),
                vec![NodeInput {
                    kind: "Function".to_string(),
                    name: "get_users".to_string(),
                    file_path: "api.py".to_string(),
                    line_start: 1,
                    line_end: 10,
                    language: "python".to_string(),
                    parent_name: None,
                    params: Some("(db)".to_string()),
                    return_type: Some("list".to_string()),
                    modifiers: None,
                    is_test: false,
                    extra: json!({}),
                }],
                vec![],
                "hash".to_string(),
                0,
            )])
            .expect("store node");
        store.rebuild_fts_index().expect("rebuild fts");
        store
    }

    #[test]
    fn fts_query_finds_indexed_symbol() {
        let store = sample_store_with_fts();
        let result = store.fts_query("get_users", 10).expect("fts query");
        assert!(!result.hits.is_empty());
        assert!(result.hits[0].1 > 0.0);
    }
}
