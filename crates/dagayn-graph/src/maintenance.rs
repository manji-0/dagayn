//! Derived-table maintenance mirroring `GraphStoreMaintenanceMixin`.

use crate::*;

/// Derived tables that reference `nodes.id` / each other, in the order they must
/// be pruned: a parent only after the children that could keep it alive.
///
/// `communities` is absent because its sweep is `refresh_community_stats`, which
/// lives in `dagayn-postproc`; the orchestration is therefore
/// `dagayn_postproc::prune_orphaned_graph_structures`.
pub const ORPHAN_PRUNE_STEPS: &[(&str, &str)] = &[
    (
        "community_summaries",
        "NOT EXISTS (SELECT 1 FROM communities c \
         WHERE c.id = community_summaries.community_id)",
    ),
    (
        "risk_index",
        "NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = risk_index.node_id)",
    ),
];

impl GraphStore {
    /// Delete rows of one derived table whose nodes no longer exist.
    ///
    /// Returns the number of rows deleted, or `0` when the table is absent on an
    /// older schema. Callers should iterate [`ORPHAN_PRUNE_STEPS`] in order.
    pub fn prune_orphan_table(&self, table: &str, predicate: &str) -> Result<i64> {
        let sql = format!("DELETE FROM {table} WHERE {predicate}");
        match self.conn.execute(&sql, []) {
            Ok(rows) => Ok(rows as i64),
            Err(_) => Ok(0),
        }
    }
}

impl GraphStore {
    /// Delete vectors whose node is gone, across every provider partition.
    ///
    /// `EmbeddingStore.remove_orphans(live, all_providers=True)` in Python:
    /// without it, a deleted node's vector keeps winning top-k slots that
    /// search then drops, returning fewer results than asked for. A graph
    /// that never stored embeddings has no table and nothing to prune.
    pub fn prune_orphaned_embeddings(&mut self) -> Result<i64> {
        // Python's prune opens an `EmbeddingStore`, which creates (or
        // migrates) the embeddings schema first; a graph built by either
        // backend then has the same tables and triggers.
        self.ensure_embeddings_schema()?;
        let deleted = self.conn.execute(
            "DELETE FROM embeddings WHERE qualified_name NOT IN \
             (SELECT qualified_name FROM nodes WHERE kind != 'File')",
            [],
        )?;
        Ok(deleted as i64)
    }
}

/// `dagayn.embeddings_store._EMBEDDINGS_SCHEMA`, byte for byte: SQLite keeps
/// the statement text in `sqlite_master`.
const EMBEDDINGS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS embeddings (
    qualified_name TEXT NOT NULL,
    vector BLOB NOT NULL,
    text_hash TEXT NOT NULL,
    provider TEXT NOT NULL DEFAULT 'unknown',
    PRIMARY KEY (qualified_name, provider)
);
"#;

/// `dagayn.embeddings_store._EMBEDDINGS_GENERATION_SCHEMA`, byte for byte.
const EMBEDDINGS_GENERATION_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS embeddings_generation (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    epoch TEXT NOT NULL,
    generation INTEGER NOT NULL
);
INSERT OR IGNORE INTO embeddings_generation (id, epoch, generation)
    VALUES (1, lower(hex(randomblob(8))), 0);
CREATE TRIGGER IF NOT EXISTS embeddings_generation_insert AFTER INSERT ON embeddings
BEGIN
    UPDATE embeddings_generation SET generation = generation + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS embeddings_generation_update AFTER UPDATE ON embeddings
BEGIN
    UPDATE embeddings_generation SET generation = generation + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS embeddings_generation_delete AFTER DELETE ON embeddings
BEGIN
    UPDATE embeddings_generation SET generation = generation + 1 WHERE id = 1;
END;
"#;

impl GraphStore {
    /// `dagayn.embeddings_store._ensure_embeddings_schema`: create the
    /// embeddings table and its generation triggers, migrating a legacy
    /// single-provider table.
    pub fn ensure_embeddings_schema(&self) -> Result<()> {
        let columns: Vec<(String, i64)> = {
            let mut stmt = self.conn.prepare("PRAGMA table_info(embeddings)")?;
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })?
            .collect::<std::result::Result<_, _>>()?
        };
        if columns.is_empty() {
            self.conn.execute_batch(EMBEDDINGS_SCHEMA)?;
        } else {
            let mut columns = columns;
            if !columns.iter().any(|(name, _)| name == "provider") {
                self.conn.execute(
                    "ALTER TABLE embeddings ADD COLUMN provider TEXT NOT NULL DEFAULT 'unknown'",
                    [],
                )?;
                columns.push(("provider".to_string(), 0));
            }
            let primary_key: Vec<&str> = columns
                .iter()
                .filter(|(_, pk)| *pk > 0)
                .map(|(name, _)| name.as_str())
                .collect();
            if primary_key == ["qualified_name"] {
                self.conn.execute(
                    "ALTER TABLE embeddings RENAME TO embeddings_legacy_single_provider",
                    [],
                )?;
                self.conn.execute_batch(EMBEDDINGS_SCHEMA)?;
                self.conn.execute(
                    "INSERT OR REPLACE INTO embeddings (qualified_name, vector, text_hash, provider)\n            SELECT qualified_name, vector, text_hash, provider\n            FROM embeddings_legacy_single_provider",
                    [],
                )?;
                self.conn
                    .execute("DROP TABLE embeddings_legacy_single_provider", [])?;
            }
        }
        self.conn.execute_batch(EMBEDDINGS_GENERATION_SCHEMA)?;
        Ok(())
    }
}

/// Embedding coverage of the graph's non-`File` nodes for one provider
/// partition (all partitions when `None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingCoverage {
    pub embeddable_nodes: i64,
    pub indexed_embeddings: i64,
    pub missing_embeddings: i64,
    pub orphan_embeddings: i64,
}

impl GraphStore {
    /// Row count per provider partition, or `None` when the graph never
    /// stored embeddings (no table).
    pub fn embedding_provider_counts(&self) -> Result<Option<HashMap<String, i64>>> {
        let has_table: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type IN ('table', 'view') \
                 AND name = 'embeddings'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !has_table {
            return Ok(None);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT provider, COUNT(*) FROM embeddings GROUP BY provider")?;
        let counts = stmt
            .query_map([], |row| <(String, i64)>::try_from(row))?
            .collect::<std::result::Result<HashMap<_, _>, _>>()?;
        Ok(Some(counts))
    }

    /// The persisted spelling of `provider` when some row carries it, compared
    /// `COLLATE NOCASE` as `EmbeddingStore._provider_key_for_lookup` does.
    pub fn embedding_provider_spelling(&self, provider: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT provider FROM embeddings WHERE provider = ? COLLATE NOCASE LIMIT 1",
                [provider],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(Into::into)
    }

    /// The stored provider partitions, largest first, in the order
    /// `EmbeddingStore._persisted_key_for_same_identity` scans them.
    pub fn embedding_partitions_by_size(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider, COUNT(*) AS n FROM embeddings GROUP BY provider ORDER BY n DESC",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, Option<String>>(0))?;
        Ok(rows
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect())
    }

    /// `EmbeddingStore.count_provider`: rows stored under `provider`, only
    /// those whose vector is `byte_len` bytes when given.
    pub fn count_embeddings(&self, provider: &str, byte_len: Option<i64>) -> Result<i64> {
        Ok(match byte_len {
            None => self.conn.query_row(
                "SELECT COUNT(*) FROM embeddings WHERE provider = ?",
                [provider],
                |row| row.get(0),
            )?,
            Some(bytes) => self.conn.query_row(
                "SELECT COUNT(*) FROM embeddings WHERE provider = ? AND length(vector) = ?",
                rusqlite::params![provider, bytes],
                |row| row.get(0),
            )?,
        })
    }

    /// `get_embedding_status` coverage counts in Python.
    pub fn embedding_coverage(&self, provider: Option<&str>) -> Result<EmbeddingCoverage> {
        let clause = if provider.is_some() {
            "AND e.provider = ?1"
        } else {
            ""
        };
        let count = |sql: String| -> Result<i64> {
            Ok(match provider {
                Some(name) => self.conn.query_row(&sql, [name], |row| row.get(0))?,
                None => self.conn.query_row(&sql, [], |row| row.get(0))?,
            })
        };
        Ok(EmbeddingCoverage {
            embeddable_nodes: self.conn.query_row(
                "SELECT COUNT(*) FROM nodes WHERE kind != 'File'",
                [],
                |row| row.get(0),
            )?,
            missing_embeddings: count(format!(
                "SELECT COUNT(*) FROM nodes n WHERE n.kind != 'File' AND NOT EXISTS \
                 (SELECT 1 FROM embeddings e WHERE e.qualified_name = n.qualified_name {clause})"
            ))?,
            indexed_embeddings: count(format!(
                "SELECT COUNT(DISTINCT e.qualified_name) FROM embeddings e \
                 JOIN nodes n ON n.qualified_name = e.qualified_name \
                 WHERE n.kind != 'File' {clause}"
            ))?,
            orphan_embeddings: count(format!(
                "SELECT COUNT(*) FROM embeddings e \
                 LEFT JOIN nodes n ON n.qualified_name = e.qualified_name \
                 WHERE n.qualified_name IS NULL {clause}"
            ))?,
        })
    }
}

/// The counts `dagayn.tools._common.graph_answerability_summary` queries,
/// each 0 when its query failed, with the failure codes in query order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnswerabilityCounts {
    pub communities: i64,
    pub unresolved_markdown_code_spans: i64,
    pub unresolved_cross_artifact_edges: i64,
    pub unassigned_nodes: i64,
    pub failures: Vec<&'static str>,
}

impl GraphStore {
    /// The answerability counts, with the SQL and failure codes of the Python
    /// summary.
    pub fn answerability_counts(&self) -> AnswerabilityCounts {
        let mut counts = AnswerabilityCounts::default();
        let mut count = |sql: &str, failure: &'static str| -> i64 {
            match self.conn.query_row(sql, [], |row| row.get::<_, i64>(0)) {
                Ok(value) => value,
                Err(_) => {
                    counts.failures.push(failure);
                    0
                }
            }
        };
        let communities = count(
            "SELECT COUNT(*) FROM communities",
            "missing_communities_table",
        );
        let code_spans = count(
            "SELECT COUNT(*) FROM edges WHERE kind = 'CROSS_ARTIFACT' \
             AND target_qualified LIKE '<unresolved:%' \
             AND extra LIKE '%markdown_code_span%' AND extra LIKE '%code_span%'",
            "missing_cross_artifact_edge_metadata",
        );
        let unresolved = count(
            "SELECT COUNT(*) FROM edges \
             WHERE kind = 'CROSS_ARTIFACT' AND target_qualified LIKE '<unresolved:%'",
            "missing_cross_artifact_edges",
        );
        let unassigned = count(
            "SELECT COUNT(*) FROM nodes n WHERE n.community_id IS NULL AND n.kind != 'File' \
             AND EXISTS (SELECT 1 FROM communities LIMIT 1)",
            "missing_community_assignment_metadata",
        );
        counts.communities = communities;
        counts.unresolved_markdown_code_spans = code_spans;
        counts.unresolved_cross_artifact_edges = unresolved;
        counts.unassigned_nodes = unassigned;
        counts
    }
}
