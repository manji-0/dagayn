use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{params, OptionalExtension};
use serde_json::json;

use crate::embeddings::{embedding_row_shape_hint, embedding_search};
use crate::{GraphError, GraphStore, Result};

const PARTIAL_EMBEDDING_COVERAGE_THRESHOLD: f64 = 0.9;

#[derive(Clone, Debug)]
pub struct EmbeddingSearchResult {
    pub hits: Vec<(i64, f64)>,
    pub health: serde_json::Value,
}

impl GraphStore {
    pub fn get_embedding_provider_counts(&self) -> Result<HashMap<String, i64>> {
        let has_embeddings = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type IN ('table', 'view') AND name = 'embeddings'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !has_embeddings {
            return Ok(HashMap::new());
        }

        let mut stmt = self
            .conn
            .prepare("SELECT provider, COUNT(*) FROM embeddings GROUP BY provider")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (provider, count) = row?;
            out.insert(provider, count);
        }
        Ok(out)
    }

    pub fn embedding_search_json(
        &self,
        provider_key: &str,
        query_vec: &[f32],
        limit: i64,
    ) -> Result<String> {
        let payload = self.embedding_search(provider_key, query_vec, limit)?;
        serde_json::to_string(&json!({
            "hits": payload.hits,
            "health": payload.health,
        }))
        .map_err(Into::into)
    }

    pub fn embedding_search(
        &self,
        provider_key: &str,
        query_vec: &[f32],
        limit: i64,
    ) -> Result<EmbeddingSearchResult> {
        let limit = limit.max(0) as usize;
        let provider_counts = self.get_embedding_provider_counts()?;
        let embeddable_node_count = self.count_embeddable_nodes()?;
        let query_dimension = query_vec.len() as i64;

        let mut health = json!({
            "status": "unknown",
            "resolved_provider_key": provider_key,
            "matching_vector_count": 0,
            "query_dimension": query_dimension,
            "provider_counts": provider_counts,
            "embeddable_node_count": embeddable_node_count,
        });

        if provider_key.is_empty() {
            health["status"] = json!("provider_unavailable");
            return Ok(EmbeddingSearchResult {
                hits: Vec::new(),
                health,
            });
        }

        let total_count = provider_counts.get(provider_key).copied().unwrap_or(0);
        let matching_count = self.count_provider_vectors(provider_key, query_vec.len())?;
        health["matching_vector_count"] = json!(matching_count);

        if matching_count == 0 {
            if total_count > 0 {
                health["status"] = json!("dimension_mismatch");
                health["stored_dimension"] = json!(self.stored_vector_dimension(provider_key)?);
            } else if provider_counts.is_empty() {
                health["status"] = json!("missing_vectors");
            } else {
                health["status"] = json!("provider_mismatch");
            }
            return Ok(EmbeddingSearchResult {
                hits: Vec::new(),
                health,
            });
        }

        if limit == 0 || query_vec.is_empty() {
            health["status"] = json!("available");
            attach_embedding_coverage(&mut health, embeddable_node_count, matching_count);
            return Ok(EmbeddingSearchResult {
                hits: Vec::new(),
                health,
            });
        }

        let db_path = embedding_db_path(&self.conn)?;
        let qn_hits = embedding_search(&db_path, provider_key, query_vec, limit)?;
        let qualified_names: Vec<String> = qn_hits.iter().map(|(name, _)| name.clone()).collect();
        let nodes_by_qn = self.get_nodes_by_qualified_names(&qualified_names)?;
        let mut hits = Vec::with_capacity(qn_hits.len());
        for (qualified_name, score) in qn_hits {
            if let Some(node) = nodes_by_qn.get(&qualified_name) {
                hits.push((node.id, score as f64));
            }
        }

        health["status"] = json!("available");
        attach_embedding_coverage(&mut health, embeddable_node_count, matching_count);
        Ok(EmbeddingSearchResult { hits, health })
    }

    fn count_embeddable_nodes(&self) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE kind != 'File'",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    fn count_provider_vectors(&self, provider_key: &str, query_dim: usize) -> Result<i64> {
        if query_dim == 0 {
            return Ok(0);
        }
        let vector_bytes = query_dim * std::mem::size_of::<f32>();
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE provider = ? AND length(vector) = ?",
            params![provider_key, vector_bytes as i64],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    fn stored_vector_dimension(&self, provider_key: &str) -> Result<Option<i64>> {
        let (_, uniform_dim) = embedding_row_shape_hint(&self.conn, provider_key)?;
        Ok(uniform_dim.map(|dim| dim as i64))
    }
}

fn embedding_db_path(conn: &rusqlite::Connection) -> Result<PathBuf> {
    conn.path()
        .map(PathBuf::from)
        .ok_or_else(|| {
            GraphError::InvalidEmbedding(
                "native embedding search requires a file-backed graph database".into(),
            )
        })
}

fn attach_embedding_coverage(
    health: &mut serde_json::Value,
    embeddable_node_count: i64,
    matching_vector_count: i64,
) {
    if embeddable_node_count <= 0 {
        return;
    }
    let indexed = matching_vector_count.max(0) as f64;
    let embeddable = embeddable_node_count as f64;
    let coverage = (indexed / embeddable).min(1.0);
    health["missing_embedding_count"] = json!((embeddable_node_count - matching_vector_count).max(0));
    health["embedding_coverage"] =
        json!((coverage * 10_000.0).round() / 10_000.0);
    if coverage < PARTIAL_EMBEDDING_COVERAGE_THRESHOLD {
        health["partial_coverage"] = json!(true);
        if health.get("status").and_then(|value| value.as_str()) == Some("available") {
            health["status"] = json!("degraded");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeInput};
    use rusqlite::Connection;
    use serde_json::json;
    use std::path::PathBuf;

    fn temp_db_path(label: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "dagayn-embedding-query-{}-{}-{}.db",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn seed_store_with_embedding(
        provider: &str,
        vector: &[f32],
    ) -> (GraphStore, PathBuf) {
        let db_path = temp_db_path("store");
        let mut store = GraphStore::open(&db_path).expect("open store");
        store
            .store_file_batch(&[(
                "file.py".to_string(),
                vec![NodeInput {
                    kind: "Function".to_string(),
                    name: "alpha".to_string(),
                    file_path: "file.py".to_string(),
                    line_start: 1,
                    line_end: 1,
                    language: "python".to_string(),
                    parent_name: None,
                    params: None,
                    return_type: None,
                    modifiers: None,
                    is_test: false,
                    extra: json!({}),
                }],
                vec![],
                "hash".to_string(),
                0,
            )])
            .expect("store node");
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS embeddings (
                qualified_name TEXT NOT NULL,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL,
                PRIMARY KEY (qualified_name, provider)
            );",
        )
        .expect("embeddings table");
        let blob: Vec<u8> = vector
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect();
        conn.execute(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) VALUES (?, ?, 'h', ?)",
            params!["file.py::alpha", blob, provider],
        )
        .expect("insert embedding");
        (store, db_path)
    }

    #[test]
    fn embedding_search_json_returns_node_ids_and_health() {
        let (store, db_path) = seed_store_with_embedding("fake#dim=4", &[1.0, 0.0, 0.0, 0.0]);
        let payload = store
            .embedding_search("fake#dim=4", &[1.0, 0.0, 0.0, 0.0], 5)
            .expect("embedding search");
        assert_eq!(payload.hits.len(), 1);
        assert!(payload.hits[0].1 > 0.9);
        assert_eq!(payload.health["status"], "available");
        assert_eq!(payload.health["matching_vector_count"], 1);
        assert_eq!(payload.health["embeddable_node_count"], 1);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn embedding_search_json_reports_dimension_mismatch() {
        let (store, db_path) =
            seed_store_with_embedding("fake#dim=8", &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let payload = store
            .embedding_search("fake#dim=8", &[1.0, 0.0, 0.0, 0.0], 5)
            .expect("embedding search");
        assert!(payload.hits.is_empty());
        assert_eq!(payload.health["status"], "dimension_mismatch");
        assert_eq!(payload.health["stored_dimension"], 8);
        let _ = std::fs::remove_file(db_path);
    }
}
