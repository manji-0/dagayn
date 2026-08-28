//! FTS5 and LIKE-fallback node search.
//!
//! Port of `GraphStoreSearchMixin.fts_query` / `keyword_query` / `search_nodes`.

use crate::helpers::*;
use crate::*;

/// Segments that match too much to anchor an OR fallback on.
const COMMON_FTS_SEGMENTS: &[&str] = &[
    "py", "rs", "ts", "js", "go", "src", "test", "tests", "index", "main", "lib", "mod", "api",
    "app", "util", "utils", "common", "core",
];

const FTS_SQL: &str = "SELECT rowid, bm25(nodes_fts, 8.0, 6.0, 3.0, 4.0, 5.0, 1.0) AS score \
     FROM nodes_fts WHERE nodes_fts MATCH ? ORDER BY score LIMIT ?";

/// `(node_id, score)` hits plus how the query matched: `and`, `or`, `phrase`, `none`.
pub type FtsQueryResult = (Vec<(i64, f64)>, &'static str);

fn split_segments(query: &str) -> Vec<&str> {
    query
        .split(|ch: char| matches!(ch, '.' | '/' | ':') || ch.is_whitespace())
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// Least likely to match spuriously: uncommon first, then longest, then last
/// alphabetically -- the same key Python's `_most_selective_segment` maximizes.
fn most_selective_segment<'a>(segments: &[&'a str]) -> &'a str {
    segments
        .iter()
        .copied()
        .max_by_key(|segment| {
            (
                !COMMON_FTS_SEGMENTS.contains(segment),
                segment.len(),
                *segment,
            )
        })
        .unwrap_or("")
}

fn quote_fts(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// `(primary_query, or_fallback_query, fallback_match_mode)`.
fn build_fts_match_queries(fts_query: &str) -> (String, String, &'static str) {
    let segments = split_segments(fts_query);
    if segments.len() > 1 {
        let quoted = segments.iter().map(|s| quote_fts(s)).collect::<Vec<_>>();
        let safe_query = quoted.join(" AND ");
        let anchor = quote_fts(most_selective_segment(&segments));
        let fallback = format!("({}) AND {anchor}", quoted.join(" OR "));
        (safe_query, fallback, "or")
    } else {
        let safe_query = quote_fts(fts_query);
        (safe_query.clone(), safe_query, "phrase")
    }
}

impl GraphStore {
    /// FTS5 BM25 search over `nodes_fts`.
    ///
    /// AND-of-quoted-segments when the input contains separators, so
    /// `api.get_users` matches `api.py::get_users` even though the tokens are
    /// not adjacent; a single phrase otherwise. Quotes prevent FTS5 operator
    /// injection. When the AND arm misses, an OR arm still requiring the most
    /// selective segment runs, so path-shaped junk does not match on shared
    /// tokens like `py` or `src`.
    ///
    /// Returns no hits when the FTS index is unavailable.
    pub fn fts_query(&self, query: &str, limit: i64) -> Result<FtsQueryResult> {
        let fts_query = segment_japanese_fts_text(query);
        let (safe_query, fallback_query, fallback_mode) = build_fts_match_queries(&fts_query);

        let run = |match_query: &str| -> rusqlite::Result<Vec<(i64, f64)>> {
            let mut stmt = self.conn.prepare(FTS_SQL)?;
            let rows = stmt.query_map(params![match_query, limit], |row| {
                // FTS5 rank is negative BM25 (lower = better); negate it.
                Ok((row.get::<_, i64>(0)?, -row.get::<_, f64>(1)?))
            })?;
            rows.collect()
        };

        let primary_mode = if split_segments(&fts_query).len() > 1 {
            "and"
        } else {
            "phrase"
        };
        match run(&safe_query) {
            Ok(hits) if !hits.is_empty() => Ok((hits, primary_mode)),
            Ok(_) if fallback_query != safe_query => match run(&fallback_query) {
                Ok(hits) if !hits.is_empty() => Ok((hits, fallback_mode)),
                Ok(_) => Ok((Vec::new(), "none")),
                // A malformed MATCH expression or a missing index is a query
                // failure, not a crash: callers fall back to keyword search.
                Err(_) => Ok((Vec::new(), "none")),
            },
            Ok(_) => Ok((Vec::new(), "none")),
            Err(_) => Ok((Vec::new(), "none")),
        }
    }

    /// AND-of-words LIKE fallback returning `(node_id, score)` with 3/2/1 scoring.
    ///
    /// Only used when FTS5 is unavailable. SQLite's `LIKE` folds ASCII only, so
    /// a query with non-ASCII words scans names in Rust instead of pre-filtering
    /// in SQL -- otherwise case/accent variants would never match.
    pub fn keyword_query(&self, query: &str, limit: i64) -> Result<Vec<(i64, f64)>> {
        let lowered = query.to_lowercase();
        let words = lowered.split_whitespace().collect::<Vec<_>>();
        if words.is_empty() {
            return Ok(Vec::new());
        }

        let all_ascii = words.iter().all(|word| word.is_ascii());
        let rows: Vec<(i64, String)> = if all_ascii {
            let where_clause =
                std::iter::repeat_n("(name LIKE ? OR qualified_name LIKE ?)", words.len())
                    .collect::<Vec<_>>()
                    .join(" AND ");
            let sql = format!("SELECT id, name FROM nodes WHERE {where_clause} LIMIT ?");
            let mut params: Vec<SqlValue> = Vec::with_capacity(words.len() * 2 + 1);
            for word in &words {
                params.push(SqlValue::Text(format!("%{word}%")));
                params.push(SqlValue::Text(format!("%{word}%")));
            }
            params.push(SqlValue::Integer(limit.saturating_mul(4)));
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

        let mut results = Vec::new();
        for (node_id, name) in rows {
            let name_lower = name.to_lowercase();
            if !words.iter().all(|word| name_lower.contains(word)) {
                continue;
            }
            let score = if name_lower == lowered {
                3.0
            } else if name_lower.starts_with(&lowered) {
                2.0
            } else {
                1.0
            };
            results.push((node_id, score));
        }

        // Stable sort keeps SQL row order within a score band, matching the
        // Python fallback's `list.sort` on score alone.
        results.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit.max(0) as usize);
        Ok(results)
    }

    /// Keyword search across node names: FTS5 first, LIKE fallback second.
    pub fn search_nodes(&self, query: &str, limit: i64) -> Result<Vec<GraphNode>> {
        let (hits, _) = self.fts_query(query, limit)?;
        if !hits.is_empty() {
            return self.nodes_in_hit_order(&hits);
        }
        let keyword_hits = self.keyword_query(query, limit)?;
        if !keyword_hits.is_empty() {
            return self.nodes_in_hit_order(&keyword_hits);
        }
        Ok(Vec::new())
    }

    fn nodes_in_hit_order(&self, hits: &[(i64, f64)]) -> Result<Vec<GraphNode>> {
        let node_ids = hits.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let by_id = self.get_nodes_by_ids(&node_ids)?;
        Ok(node_ids
            .iter()
            .filter_map(|node_id| by_id.get(node_id).cloned())
            .collect())
    }
}
