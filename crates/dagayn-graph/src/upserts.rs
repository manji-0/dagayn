//! Single-row node/edge upserts mirroring `GraphStoreStorageMixin`.
//!
//! The build hot paths use `store_file_batch` (delete-then-bulk-insert); these
//! exist for post-processing passes that add a handful of derived rows.

use crate::helpers::*;
use crate::*;

impl GraphStore {
    /// Insert or update one node, keyed on `qualified_name`. Returns its id.
    pub fn upsert_node(&mut self, node: &NodeInput, file_hash: &str, mtime_ns: i64) -> Result<i64> {
        let qualified = make_qualified_parts(
            &node.kind,
            &node.name,
            &node.file_path,
            node.parent_name.as_deref(),
        );
        let extra = if node.extra.is_null() {
            "{}".to_string()
        } else {
            serde_json::to_string(&node.extra)?
        };
        let now = now_seconds()?;
        let repo_root = self.get_metadata("repo_root")?.map(PathBuf::from);

        let tx = write_tx(&mut self.conn)?;
        let node_id: i64 = tx.query_row(
            "INSERT INTO nodes \
                 (kind, name, qualified_name, file_path, line_start, line_end, \
                  language, parent_name, params, return_type, modifiers, is_test, \
                  file_hash, mtime_ns, extra, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(qualified_name) DO UPDATE SET \
                 kind=excluded.kind, name=excluded.name, \
                 file_path=excluded.file_path, line_start=excluded.line_start, \
                 line_end=excluded.line_end, language=excluded.language, \
                 parent_name=excluded.parent_name, params=excluded.params, \
                 return_type=excluded.return_type, modifiers=excluded.modifiers, \
                 is_test=excluded.is_test, file_hash=excluded.file_hash, \
                 mtime_ns=excluded.mtime_ns, \
                 extra=excluded.extra, updated_at=excluded.updated_at \
             RETURNING id",
            params![
                node.kind,
                node.name,
                qualified,
                node.file_path,
                node.line_start,
                node.line_end,
                node.language,
                node.parent_name,
                node.params,
                node.return_type,
                node.modifiers,
                i64::from(node.is_test),
                file_hash,
                mtime_ns,
                extra,
                now,
            ],
            |row| row.get(0),
        )?;
        crate::fts_sync::upsert_fts_for_node_ids_tx(&tx, &[node_id], repo_root.as_deref())?;
        tx.commit()?;
        Ok(node_id)
    }

    /// Insert or update one edge. Returns its id.
    ///
    /// Edge identity includes `line`, so several call sites to the same target
    /// stay distinct rows.
    pub fn upsert_edge(&mut self, edge: &EdgeInput) -> Result<i64> {
        let extra = if edge.extra.is_null() {
            "{}".to_string()
        } else {
            serde_json::to_string(&edge.extra)?
        };
        let (confidence, confidence_tier) = edge_metadata_from_raw_extra(&extra)?;
        let (confidence, confidence_tier) =
            normalize_edge_confidence(&edge.source, &edge.target, confidence, confidence_tier);
        let target_name = edge_target_name(&edge.target);
        let now = now_seconds()?;

        let tx = write_tx(&mut self.conn)?;
        let existing: Option<i64> = tx
            .query_row(
                "UPDATE edges \
                 SET target_name=?, extra=?, confidence=?, confidence_tier=?, updated_at=? \
                 WHERE kind=? AND source_qualified=? AND target_qualified=? \
                       AND file_path=? AND line=? \
                 RETURNING id",
                params![
                    target_name,
                    extra,
                    confidence,
                    confidence_tier.as_str(),
                    now,
                    edge.kind,
                    edge.source,
                    edge.target,
                    edge.file_path,
                    edge.line,
                ],
                |row| row.get(0),
            )
            .optional()?;
        let edge_id = match existing {
            Some(edge_id) => edge_id,
            None => tx.query_row(
                "INSERT INTO edges \
                     (kind, source_qualified, target_qualified, target_name, file_path, line, \
                      extra, confidence, confidence_tier, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 RETURNING id",
                params![
                    edge.kind,
                    edge.source,
                    edge.target,
                    target_name,
                    edge.file_path,
                    edge.line,
                    extra,
                    confidence,
                    confidence_tier.as_str(),
                    now,
                ],
                |row| row.get(0),
            )?,
        };
        tx.commit()?;
        Ok(edge_id)
    }
}
