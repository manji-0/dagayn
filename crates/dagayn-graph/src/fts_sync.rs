use crate::helpers::*;
use crate::*;

pub(crate) const FTS_COUNT_KEY: &str = "fts_indexed_node_count";
pub(crate) const FTS_BUILT_AT_KEY: &str = "fts_indexed_at";

const FTS_DELETE_SQL: &str = "DELETE FROM nodes_fts WHERE rowid = ?";
const FTS_INSERT_SQL: &str = "INSERT INTO nodes_fts(rowid, name, qualified_name, file_path, signature, \
    identifier_tokens, doc_text) VALUES (?, ?, ?, ?, ?, ?, ?)";

pub(crate) fn set_fts_watermark_tx(tx: &Transaction<'_>, node_count: Option<i64>) -> Result<()> {
    let count = match node_count {
        Some(count) => count,
        None => {
            if table_exists(tx, "nodes_fts")? {
                tx.query_row("SELECT count(*) FROM nodes_fts", [], |row| row.get(0))?
            } else {
                0
            }
        }
    };
    let built_at = now_seconds()?.to_string();
    tx.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES (?, ?)",
        params![FTS_COUNT_KEY, count.to_string()],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES (?, ?)",
        params![FTS_BUILT_AT_KEY, built_at],
    )?;
    Ok(())
}

pub(crate) fn fts_index_counts_tx(tx: &Transaction<'_>) -> Result<(i64, i64)> {
    let nodes_count: i64 = tx.query_row("SELECT count(*) FROM nodes", [], |row| row.get(0))?;
    let fts_count = if table_exists(tx, "nodes_fts")? {
        tx.query_row("SELECT count(*) FROM nodes_fts", [], |row| row.get(0))?
    } else {
        0
    };
    Ok((nodes_count, fts_count))
}

pub(crate) fn delete_fts_for_node_ids_tx(tx: &Transaction<'_>, node_ids: &[i64]) -> Result<()> {
    if node_ids.is_empty() || !table_exists(tx, "nodes_fts")? {
        return Ok(());
    }
    for node_id in node_ids {
        tx.execute(FTS_DELETE_SQL, params![node_id])?;
    }
    Ok(())
}

pub(crate) fn delete_fts_for_file_paths_tx(tx: &Transaction<'_>, file_paths: &[String]) -> Result<()> {
    if file_paths.is_empty() || !table_exists(tx, "nodes_fts")? {
        return Ok(());
    }
    for chunk in file_paths.chunks(450) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT n.id FROM nodes n \
             INNER JOIN nodes_fts fts ON fts.rowid = n.id \
             WHERE n.file_path IN ({placeholders})"
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| row.get::<_, i64>(0))?;
        let mut node_ids = Vec::new();
        for row in rows {
            node_ids.push(row?);
        }
        delete_fts_for_node_ids_tx(tx, &node_ids)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_node_fts_values(
    repo_root: Option<&Path>,
    kind: &str,
    name: &str,
    qualified_name: &str,
    file_path: &str,
    line_start: Option<i64>,
    line_end: Option<i64>,
    signature: Option<&str>,
    extra: &Value,
) -> (String, String, String, String, String, String) {
    let display_name = extra
        .get("display_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let identifier_tokens =
        identifier_search_text([name, qualified_name, file_path, display_name]);
    let source_excerpt =
        read_node_source_excerpt(repo_root, kind, file_path, line_start, line_end);
    let structured_description = structured_code_reference_text(
        kind,
        name,
        qualified_name,
        file_path,
        display_name,
        signature,
        source_excerpt.as_str(),
    );
    let doc_text = [
        display_name,
        structured_description.as_str(),
        source_excerpt.as_str(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    let doc_text = segment_japanese_fts_text(&doc_text);
    (
        name.to_string(),
        qualified_name.to_string(),
        file_path.to_string(),
        signature.unwrap_or("").to_string(),
        identifier_tokens,
        doc_text,
    )
}

pub(crate) fn sync_fts_for_file_paths_tx(
    tx: &Transaction<'_>,
    file_paths: &[String],
    repo_root: Option<&Path>,
) -> Result<i64> {
    if file_paths.is_empty() {
        return Ok(0);
    }
    if !table_exists(tx, "nodes_fts")? {
        tx.execute_batch(
            r#"
            CREATE VIRTUAL TABLE nodes_fts USING fts5(
                name, qualified_name, file_path, signature, identifier_tokens, doc_text,
                tokenize='porter unicode61'
            );
            "#,
        )?;
    }
    delete_fts_for_file_paths_tx(tx, file_paths)?;
    for chunk in file_paths.chunks(450) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT rowid AS node_rowid, kind, name, qualified_name, file_path, line_start, \
             line_end, signature, extra FROM nodes WHERE file_path IN ({placeholders})"
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
            Ok((
                row.get::<_, i64>("node_rowid")?,
                row.get::<_, String>("kind")?,
                row.get::<_, String>("name")?,
                row.get::<_, String>("qualified_name")?,
                row.get::<_, String>("file_path")?,
                row.get::<_, Option<i64>>("line_start")?,
                row.get::<_, Option<i64>>("line_end")?,
                row.get::<_, Option<String>>("signature")?,
                row.get::<_, Option<String>>("extra")?,
            ))
        })?;
        for row in rows {
            let (
                rowid,
                kind,
                name,
                qualified_name,
                file_path,
                line_start,
                line_end,
                signature,
                extra_raw,
            ) = row?;
            let extra = parse_json_column(extra_raw)?;
            let (name, qualified_name, file_path, signature, identifier_tokens, doc_text) =
                build_node_fts_values(
                    repo_root,
                    &kind,
                    &name,
                    &qualified_name,
                    &file_path,
                    line_start,
                    line_end,
                    signature.as_deref(),
                    &extra,
                );
            tx.execute(
                FTS_INSERT_SQL,
                params![
                    rowid,
                    name,
                    qualified_name,
                    file_path,
                    signature,
                    identifier_tokens,
                    doc_text
                ],
            )?;
        }
    }
    set_fts_watermark_tx(tx, None)?;
    Ok(0)
}

pub(crate) fn structured_code_reference_text(
    kind: &str,
    name: &str,
    qualified_name: &str,
    file_path: &str,
    display_name: &str,
    signature: Option<&str>,
    source_excerpt: &str,
) -> String {
    let mut parts = vec![
        format!("kind: {kind}"),
        format!("name: {name}"),
        format!("qualified: {qualified_name}"),
        format!("file: {}", file_path.replace('/', " ")),
    ];
    if !display_name.is_empty() {
        parts.push(format!("display: {display_name}"));
    }
    if let Some(signature) = signature.filter(|value| !value.is_empty()) {
        parts.push(format!("signature: {signature}"));
    }
    if !source_excerpt.is_empty() {
        parts.push(format!("source:\n{source_excerpt}"));
    }
    parts.join("\n")
}

pub(crate) fn rebuild_fts_index_tx(conn: &Connection, repo_root: Option<&Path>) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        r#"
        DROP TABLE IF EXISTS nodes_fts;
        CREATE VIRTUAL TABLE nodes_fts USING fts5(
            name, qualified_name, file_path, signature, identifier_tokens, doc_text,
            tokenize='porter unicode61'
        );
        "#,
    )?;
    let count = {
        let mut stmt = tx.prepare(
            "SELECT rowid AS node_rowid, kind, name, qualified_name, file_path, line_start, line_end, \
             signature, extra FROM nodes",
        )?;
        let mut count = 0_i64;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>("node_rowid")?,
                row.get::<_, String>("kind")?,
                row.get::<_, String>("name")?,
                row.get::<_, String>("qualified_name")?,
                row.get::<_, String>("file_path")?,
                row.get::<_, Option<i64>>("line_start")?,
                row.get::<_, Option<i64>>("line_end")?,
                row.get::<_, Option<String>>("signature")?,
                row.get::<_, Option<String>>("extra")?,
            ))
        })?;
        for row in rows {
            let (
                rowid,
                kind,
                name,
                qualified_name,
                file_path,
                line_start,
                line_end,
                signature,
                extra_raw,
            ) = row?;
            let extra = parse_json_column(extra_raw)?;
            let (name, qualified_name, file_path, signature, identifier_tokens, doc_text) =
                build_node_fts_values(
                    repo_root,
                    &kind,
                    &name,
                    &qualified_name,
                    &file_path,
                    line_start,
                    line_end,
                    signature.as_deref(),
                    &extra,
                );
            tx.execute(
                FTS_INSERT_SQL,
                params![
                    rowid,
                    name,
                    qualified_name,
                    file_path,
                    signature,
                    identifier_tokens,
                    doc_text
                ],
            )?;
            count += 1;
        }
        count
    };
    set_fts_watermark_tx(&tx, Some(count))?;
    tx.commit()?;
    Ok(count)
}

pub(crate) fn fts_needs_rebuild_tx(tx: &Transaction<'_>) -> Result<bool> {
    let (nodes_count, fts_count) = fts_index_counts_tx(tx)?;
    if nodes_count != fts_count {
        return Ok(true);
    }
    if !table_exists(tx, "nodes_fts")? || nodes_count == 0 {
        return Ok(false);
    }
    let empty_generated: i64 = tx.query_row(
        "SELECT count(*) FROM nodes_fts \
         WHERE (identifier_tokens = '' OR identifier_tokens IS NULL) \
           AND (doc_text = '' OR doc_text IS NULL)",
        [],
        |row| row.get(0),
    )?;
    Ok(empty_generated > 0)
}
