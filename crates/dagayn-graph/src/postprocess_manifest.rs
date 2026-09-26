use crate::helpers::*;
use crate::postprocess_bridges::extra_json;
use crate::*;

impl GraphStore {
    pub fn replace_manifest_bridges(
        &mut self,
        extractor_id: &str,
        nodes: &[NodeInput],
        edges: &[EdgeInput],
    ) -> Result<i64> {
        let now = now_seconds()?;
        let tx = write_tx(&mut self.conn)?;
        tx.execute(
            "DELETE FROM edges WHERE kind='CROSS_ARTIFACT' \
             AND json_extract(extra, '$.extractor') = ?",
            [extractor_id],
        )?;
        tx.execute(
            "DELETE FROM nodes WHERE kind='File' \
             AND json_extract(extra, '$.extractor') = ?",
            [extractor_id],
        )?;

        let mut nodes_upserted = 0_i64;
        let mut touched_files: HashSet<String> = HashSet::new();
        for node in nodes {
            let qualified = make_qualified_parts(
                &node.kind,
                &node.name,
                &node.file_path,
                node.parent_name.as_deref(),
            );
            if node.kind == "File" {
                let exists: bool = tx
                    .query_row(
                        "SELECT 1 FROM nodes WHERE qualified_name = ?",
                        [&qualified],
                        |_| Ok(true),
                    )
                    .optional()?
                    .unwrap_or(false);
                if exists {
                    continue;
                }
            }
            let extra = extra_json(&node.extra)?;
            tx.execute(
                "INSERT INTO nodes
                    (kind, name, qualified_name, file_path, line_start, line_end,
                     language, parent_name, params, return_type, modifiers, is_test,
                     file_hash, mtime_ns, extra, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(qualified_name) DO UPDATE SET
                    kind=excluded.kind, name=excluded.name,
                    file_path=excluded.file_path, line_start=excluded.line_start,
                    line_end=excluded.line_end, language=excluded.language,
                    parent_name=excluded.parent_name, params=excluded.params,
                    return_type=excluded.return_type, modifiers=excluded.modifiers,
                    is_test=excluded.is_test, extra=excluded.extra, updated_at=excluded.updated_at",
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
                    "",
                    0_i64,
                    extra,
                    now
                ],
            )?;
            touched_files.insert(node.file_path.clone());
            nodes_upserted += 1;
        }

        for edge in edges {
            let extra_val = &edge.extra;
            let extra = extra_json(extra_val)?;
            let (confidence, tier) = edge_metadata_from_raw_extra(&extra)?;
            let (confidence, tier) =
                normalize_edge_confidence(&edge.source, &edge.target, confidence, tier);
            let target_name = edge_target_name(&edge.target);
            let updated = tx.execute(
                "UPDATE edges
                 SET target_name=?, extra=?, confidence=?, confidence_tier=?, updated_at=?
                 WHERE kind=? AND source_qualified=? AND target_qualified=?
                       AND file_path=? AND line=?",
                params![
                    target_name,
                    extra,
                    confidence,
                    tier.as_str(),
                    now,
                    edge.kind,
                    edge.source,
                    edge.target,
                    edge.file_path,
                    edge.line
                ],
            )?;
            if updated == 0 {
                tx.execute(
                    "INSERT INTO edges
                        (kind, source_qualified, target_qualified, target_name, file_path, line, extra,
                         confidence, confidence_tier, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        edge.kind,
                        edge.source,
                        edge.target,
                        target_name,
                        edge.file_path,
                        edge.line,
                        extra,
                        confidence,
                        tier.as_str(),
                        now
                    ],
                )?;
            }
        }

        let files: Vec<String> = touched_files.into_iter().collect();
        crate::fts_sync::sync_fts_for_file_paths_tx(&tx, &files, None)?;
        tx.commit()?;
        Ok(nodes_upserted)
    }

    pub fn replace_manifest_bridges_json(
        &mut self,
        extractor_id: &str,
        nodes_json: &str,
        edges_json: &str,
    ) -> Result<i64> {
        let nodes: Vec<NodeInput> = serde_json::from_str(nodes_json)?;
        let edges: Vec<EdgeInput> = serde_json::from_str(edges_json)?;
        self.replace_manifest_bridges(extractor_id, &nodes, &edges)
    }
}
