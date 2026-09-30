//! Write transactions, timestamps, qualified-name parts, and file removal.

use super::*;

/// Begin a write transaction that takes the write lock up front.
///
/// rusqlite's `transaction()` is `BEGIN DEFERRED`, which acquires a *read*
/// snapshot first. Any of our write paths that reads before writing then has to
/// upgrade, and if another connection committed in between, the upgrade fails
/// **immediately** with `SQLITE_BUSY` — `busy_timeout` does not apply to a
/// read-to-write upgrade, so the 5 s we configure was never spent. `IMMEDIATE`
/// takes the write lock at `BEGIN`, where `busy_timeout` does apply, which is
/// also what the Python backend has always done.
pub(crate) fn write_tx(conn: &mut Connection) -> Result<Transaction<'_>> {
    Ok(conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?)
}

pub(crate) fn now_seconds() -> Result<f64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| GraphError::Clock)?;
    Ok(duration.as_secs_f64())
}

pub(crate) fn make_qualified_parts(
    kind: &str,
    name: &str,
    file_path: &str,
    parent_name: Option<&str>,
) -> String {
    if kind == "File" {
        file_path.to_string()
    } else if let Some(parent) = parent_name {
        format!("{file_path}::{parent}.{name}")
    } else {
        format!("{file_path}::{name}")
    }
}

pub(crate) fn edge_target_name(target_qualified: &str) -> String {
    target_qualified
        .rsplit("::")
        .next()
        .unwrap_or(target_qualified)
        .to_string()
}

/// Delete rows keyed on `nodes.id` for the nodes matching `node_predicate`.
///
/// These have to go before the nodes do: node ids are autoincremented, so once
/// the file is re-parsed the old rows point at ids that will never come back.
/// Missing them left orphaned `flow_memberships` behind, which kept
/// `prune_orphaned_graph_structures` finding work the Python store had already
/// done here.
fn delete_node_keyed_rows_tx(
    tx: &Transaction<'_>,
    node_predicate: &str,
    params: &[String],
) -> Result<()> {
    for table in ["flow_memberships", "risk_index"] {
        let sql = format!(
            "DELETE FROM {table} \
             WHERE node_id IN (SELECT id FROM nodes WHERE {node_predicate})"
        );
        // A table absent on an older schema has nothing to remove.
        let _ = tx.execute(&sql, rusqlite::params_from_iter(params));
    }
    Ok(())
}

pub(crate) fn remove_file_data_tx(tx: &Transaction<'_>, file_path: &str) -> Result<()> {
    crate::fts_sync::delete_fts_for_file_paths_tx(tx, &[file_path.to_string()])?;
    tx.execute("DELETE FROM hub_scores WHERE file_path = ?", [file_path])?;
    tx.execute("DELETE FROM bridge_scores WHERE file_path = ?", [file_path])?;
    delete_node_keyed_rows_tx(tx, "file_path = ?", &[file_path.to_string()])?;
    tx.execute("DELETE FROM edges WHERE file_path = ?", [file_path])?;
    tx.execute("DELETE FROM nodes WHERE file_path = ?", [file_path])?;
    crate::fts_sync::set_fts_watermark_tx(tx, None)?;
    Ok(())
}

pub(crate) fn remove_files_data_tx(tx: &Transaction<'_>, file_paths: &[String]) -> Result<()> {
    remove_files_rows_tx(tx, file_paths)?;
    crate::fts_sync::set_fts_watermark_tx(tx, None)?;
    Ok(())
}

/// [`remove_files_data_tx`] without the FTS watermark, which counts the whole
/// FTS table and is left to callers that write more rows afterwards.
pub(super) fn remove_files_rows_tx(tx: &Transaction<'_>, file_paths: &[String]) -> Result<()> {
    crate::fts_sync::delete_fts_for_file_paths_tx(tx, file_paths)?;
    for chunk in file_paths.chunks(450) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        delete_node_keyed_rows_tx(tx, &format!("file_path IN ({placeholders})"), chunk)?;
        let hub_sql = format!("DELETE FROM hub_scores WHERE file_path IN ({placeholders})");
        tx.execute(&hub_sql, rusqlite::params_from_iter(chunk))?;
        let bridge_sql = format!("DELETE FROM bridge_scores WHERE file_path IN ({placeholders})");
        tx.execute(&bridge_sql, rusqlite::params_from_iter(chunk))?;
        let edges_sql = format!("DELETE FROM edges WHERE file_path IN ({placeholders})");
        tx.execute(&edges_sql, rusqlite::params_from_iter(chunk))?;
        let nodes_sql = format!("DELETE FROM nodes WHERE file_path IN ({placeholders})");
        tx.execute(&nodes_sql, rusqlite::params_from_iter(chunk))?;
    }
    Ok(())
}
