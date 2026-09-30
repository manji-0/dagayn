//! Bulk file-batch writes and write-index management.

use super::*;

/// Index the batch's freshly inserted nodes. Their files' old FTS rows were
/// removed with the old nodes, so there is nothing to delete first.
/// `defer_watermark` skips the full FTS count for bulk loads, which set it
/// once when they finish.
pub(crate) fn sync_fts_after_file_batch_tx(
    tx: &Transaction<'_>,
    file_paths: &[String],
    defer_watermark: bool,
) -> Result<()> {
    let repo_root = tx
        .query_row(
            "SELECT value FROM metadata WHERE key = 'repo_root'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    crate::fts_sync::insert_fts_for_file_paths_tx(
        tx,
        file_paths,
        repo_root.as_deref().map(Path::new),
    )?;
    if !defer_watermark {
        crate::fts_sync::set_fts_watermark_tx(tx, None)?;
    }
    Ok(())
}

pub(crate) fn store_file_batch_tx(
    tx: &Transaction<'_>,
    batch: &[FileBatchItem],
    suspend_indexes: bool,
    defer_fts_watermark: bool,
) -> Result<()> {
    let now = now_seconds()?;
    let suspend_indexes = suspend_indexes && should_suspend_write_indexes(tx, batch.len())?;
    if suspend_indexes {
        drop_graph_write_indexes(tx)?;
    }
    let file_paths = batch
        .iter()
        .map(|(file_path, _, _, _, _)| file_path.clone())
        .collect::<Vec<_>>();
    remove_files_rows_tx(tx, &file_paths)?;

    let mut seen_edges = HashSet::new();
    let mut node_params =
        Vec::<SqlValue>::with_capacity(NODE_INSERT_ROWS * NODE_INSERT_PARAM_COUNT);
    let mut node_rows = 0_usize;
    let mut edge_params =
        Vec::<SqlValue>::with_capacity(EDGE_INSERT_ROWS * EDGE_INSERT_PARAM_COUNT);
    let mut edge_rows = 0_usize;

    for (_file_path, nodes, edges, file_hash, mtime_ns) in batch {
        for node in nodes {
            let qualified = make_qualified_parts(
                &node.kind,
                &node.name,
                &node.file_path,
                node.parent_name.as_deref(),
            );
            let extra = extra_json(&node.extra)?;
            push_text(&mut node_params, &node.kind);
            push_text(&mut node_params, &node.name);
            node_params.push(SqlValue::Text(qualified));
            push_text(&mut node_params, &node.file_path);
            node_params.push(SqlValue::Integer(node.line_start));
            node_params.push(SqlValue::Integer(node.line_end));
            push_text(&mut node_params, &node.language);
            push_optional_text(&mut node_params, node.parent_name.as_deref());
            push_optional_text(&mut node_params, node.params.as_deref());
            push_optional_text(&mut node_params, node.return_type.as_deref());
            push_optional_text(&mut node_params, node.modifiers.as_deref());
            node_params.push(SqlValue::Integer(i64::from(node.is_test)));
            push_text(&mut node_params, file_hash);
            node_params.push(SqlValue::Integer(*mtime_ns));
            node_params.push(SqlValue::Text(extra));
            node_params.push(SqlValue::Real(now));
            node_rows += 1;
            if node_rows == NODE_INSERT_ROWS {
                insert_compact_node_rows(tx, node_rows, &node_params)?;
                node_params.clear();
                node_rows = 0;
            }
        }

        for edge in edges {
            let key = (
                edge.kind.as_str(),
                edge.source.as_str(),
                edge.target.as_str(),
                edge.file_path.as_str(),
                edge.line,
            );
            if !seen_edges.insert(key) {
                continue;
            }
            let confidence = edge
                .extra
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let confidence_tier =
                ConfidenceTier::from_raw(edge.extra.get("confidence_tier").and_then(Value::as_str));
            let (confidence, confidence_tier) =
                normalize_edge_confidence(&edge.source, &edge.target, confidence, confidence_tier);
            let extra_json = extra_json(&edge.extra)?;
            push_text(&mut edge_params, &edge.kind);
            push_text(&mut edge_params, &edge.source);
            push_text(&mut edge_params, &edge.target);
            push_text(&mut edge_params, &edge_target_name(&edge.target));
            push_text(&mut edge_params, &edge.file_path);
            edge_params.push(SqlValue::Integer(edge.line));
            edge_params.push(SqlValue::Text(extra_json));
            edge_params.push(SqlValue::Real(confidence));
            push_text(&mut edge_params, confidence_tier.as_str());
            edge_params.push(SqlValue::Real(now));
            edge_rows += 1;
            if edge_rows == EDGE_INSERT_ROWS {
                insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
                edge_params.clear();
                edge_rows = 0;
            }
        }
    }
    if node_rows > 0 {
        insert_compact_node_rows(tx, node_rows, &node_params)?;
    }
    if edge_rows > 0 {
        insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
    }
    if suspend_indexes {
        create_graph_write_indexes(tx)?;
    }
    sync_fts_after_file_batch_tx(tx, &file_paths, defer_fts_watermark)?;
    Ok(())
}

pub(crate) fn store_raw_compact_file_batch_tx(
    tx: &Transaction<'_>,
    batch: &[RawCompactFileBatchItem],
    suspend_indexes: bool,
    defer_fts_watermark: bool,
) -> Result<()> {
    let now = now_seconds()?;
    let suspend_indexes = suspend_indexes && should_suspend_write_indexes(tx, batch.len())?;
    if suspend_indexes {
        drop_graph_write_indexes(tx)?;
    }
    let file_paths = batch
        .iter()
        .map(|(file_path, _, _, _, _)| file_path.clone())
        .collect::<Vec<_>>();
    remove_files_rows_tx(tx, &file_paths)?;

    let mut seen_edges = HashSet::new();
    let mut node_params =
        Vec::<SqlValue>::with_capacity(NODE_INSERT_ROWS * NODE_INSERT_PARAM_COUNT);
    let mut node_rows = 0_usize;
    let mut edge_params =
        Vec::<SqlValue>::with_capacity(EDGE_INSERT_ROWS * EDGE_INSERT_PARAM_COUNT);
    let mut edge_rows = 0_usize;

    for (_file_path, nodes, edges, file_hash, mtime_ns) in batch {
        for node in nodes {
            let RawCompactNodeInput(
                kind,
                name,
                file_path,
                line_start,
                line_end,
                language,
                parent_name,
                params,
                return_type,
                modifiers,
                is_test,
                extra,
            ) = node;
            let qualified = make_qualified_parts(kind, name, file_path, parent_name.as_deref());
            push_text(&mut node_params, kind);
            push_text(&mut node_params, name);
            node_params.push(SqlValue::Text(qualified));
            push_text(&mut node_params, file_path);
            node_params.push(SqlValue::Integer(*line_start));
            node_params.push(SqlValue::Integer(*line_end));
            push_text(&mut node_params, language);
            push_optional_text(&mut node_params, parent_name.as_deref());
            push_optional_text(&mut node_params, params.as_deref());
            push_optional_text(&mut node_params, return_type.as_deref());
            push_optional_text(&mut node_params, modifiers.as_deref());
            node_params.push(SqlValue::Integer(i64::from(*is_test)));
            push_text(&mut node_params, file_hash);
            node_params.push(SqlValue::Integer(*mtime_ns));
            node_params.push(SqlValue::Text(extra.get().to_string()));
            node_params.push(SqlValue::Real(now));
            node_rows += 1;
            if node_rows == NODE_INSERT_ROWS {
                insert_compact_node_rows(tx, node_rows, &node_params)?;
                node_params.clear();
                node_rows = 0;
            }
        }

        for edge in edges {
            let RawCompactEdgeInput(kind, source, target, file_path, line, extra) = edge;
            let key = (
                kind.as_str(),
                source.as_str(),
                target.as_str(),
                file_path.as_str(),
                *line,
            );
            if !seen_edges.insert(key) {
                continue;
            }
            let (confidence, confidence_tier) = edge_metadata_from_raw_extra(extra.get())?;
            let (confidence, confidence_tier) =
                normalize_edge_confidence(source, target, confidence, confidence_tier);
            push_text(&mut edge_params, kind);
            push_text(&mut edge_params, source);
            push_text(&mut edge_params, target);
            push_text(&mut edge_params, &edge_target_name(target));
            push_text(&mut edge_params, file_path);
            edge_params.push(SqlValue::Integer(*line));
            edge_params.push(SqlValue::Text(extra.get().to_string()));
            edge_params.push(SqlValue::Real(confidence));
            edge_params.push(SqlValue::Text(confidence_tier.as_str().to_string()));
            edge_params.push(SqlValue::Real(now));
            edge_rows += 1;
            if edge_rows == EDGE_INSERT_ROWS {
                insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
                edge_params.clear();
                edge_rows = 0;
            }
        }
    }
    if node_rows > 0 {
        insert_compact_node_rows(tx, node_rows, &node_params)?;
    }
    if edge_rows > 0 {
        insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
    }
    if suspend_indexes {
        create_graph_write_indexes(tx)?;
    }
    sync_fts_after_file_batch_tx(tx, &file_paths, defer_fts_watermark)?;
    Ok(())
}

pub(crate) fn should_suspend_write_indexes(
    tx: &Transaction<'_>,
    file_count: usize,
) -> Result<bool> {
    if file_count < SUSPEND_INDEX_FILE_THRESHOLD {
        return Ok(false);
    }
    let has_nodes: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM nodes LIMIT 1)", [], |row| {
        row.get(0)
    })?;
    if has_nodes != 0 {
        return Ok(false);
    }
    let has_edges: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM edges LIMIT 1)", [], |row| {
        row.get(0)
    })?;
    Ok(has_edges == 0)
}

pub(crate) fn drop_graph_write_indexes(tx: &Transaction<'_>) -> Result<()> {
    for (name, _) in WRITE_INDEXES {
        tx.execute(&format!("DROP INDEX IF EXISTS {name}"), [])?;
    }
    Ok(())
}

pub(crate) fn create_graph_write_indexes(tx: &Transaction<'_>) -> Result<()> {
    for (_, sql) in WRITE_INDEXES {
        tx.execute(sql, [])?;
    }
    Ok(())
}

pub(crate) fn edge_metadata_from_raw_extra(raw: &str) -> Result<(f64, ConfidenceTier)> {
    if raw == "{}" {
        return Ok((1.0, ConfidenceTier::default()));
    }
    let extra: Value = serde_json::from_str(raw)?;
    let confidence = extra
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let confidence_tier =
        ConfidenceTier::from_raw(extra.get("confidence_tier").and_then(Value::as_str));
    Ok((confidence, confidence_tier))
}

pub(crate) fn normalize_edge_confidence(
    source: &str,
    target: &str,
    confidence: f64,
    confidence_tier: ConfidenceTier,
) -> (f64, ConfidenceTier) {
    if (source.starts_with("<unresolved:") || target.starts_with("<unresolved:"))
        && matches!(
            confidence_tier,
            ConfidenceTier::Extracted | ConfidenceTier::Unknown
        )
    {
        return (confidence.min(0.2), ConfidenceTier::Low);
    }
    (confidence, confidence_tier)
}

pub(crate) fn insert_compact_node_rows(
    tx: &Transaction<'_>,
    rows: usize,
    values: &[SqlValue],
) -> Result<()> {
    let sql = format!(
        r#"
        INSERT INTO nodes
            (kind, name, qualified_name, file_path, line_start, line_end,
             language, parent_name, params, return_type, modifiers, is_test,
             file_hash, mtime_ns, extra, updated_at)
        VALUES {}
        ON CONFLICT(qualified_name) DO UPDATE SET
            kind=excluded.kind, name=excluded.name,
            file_path=excluded.file_path, line_start=excluded.line_start,
            line_end=excluded.line_end, language=excluded.language,
            parent_name=excluded.parent_name, params=excluded.params,
            return_type=excluded.return_type, modifiers=excluded.modifiers,
            is_test=excluded.is_test, file_hash=excluded.file_hash,
            mtime_ns=excluded.mtime_ns, extra=excluded.extra, updated_at=excluded.updated_at
        "#,
        value_placeholders(NODE_INSERT_PARAM_COUNT, rows)
    );
    tx.execute(&sql, rusqlite::params_from_iter(values.iter()))?;
    Ok(())
}

pub(crate) fn insert_compact_edge_rows(
    tx: &Transaction<'_>,
    rows: usize,
    values: &[SqlValue],
) -> Result<()> {
    let sql = format!(
        r#"
        INSERT INTO edges
            (kind, source_qualified, target_qualified, target_name, file_path, line, extra,
             confidence, confidence_tier, updated_at)
        VALUES {}
        "#,
        value_placeholders(EDGE_INSERT_PARAM_COUNT, rows)
    );
    tx.execute(&sql, rusqlite::params_from_iter(values.iter()))?;
    Ok(())
}
