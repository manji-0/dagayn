//! Repository-wide sweep of derived rows orphaned by re-parses.

use std::collections::HashMap;

use dagayn_graph::{GraphError, GraphStore, ORPHAN_PRUNE_STEPS};
use serde_json::{json, Value};

use crate::communities::refresh_community_stats_json;

type Result<T> = std::result::Result<T, GraphError>;

/// Delete derived rows whose nodes no longer exist.
///
/// Re-parsing a file deletes its nodes and inserts new ones with fresh
/// autoincrement ids, so every re-parse orphans the flow memberships, community
/// assignments, and risk rows that pointed at the old ids. Nothing else removes
/// them: `remove_files_data` drops nodes and edges only, and flow/community
/// detection runs at `postprocess=full`. Left alone, `flow_tool` keeps serving
/// flows whose whole path was deleted commits ago.
///
/// Flow rows whose `path_json` still references deleted ids are rewritten from
/// surviving memberships before the sweep deletes empty flows.
///
/// Lives here rather than in `dagayn-graph` because the `communities` step is
/// [`refresh_community_stats_json`], which needs the Leiden cohesion code.
///
/// Returns `{table: rows_deleted}` for the tables that lost rows.
pub fn prune_orphaned_graph_structures(store: &mut GraphStore) -> Result<HashMap<String, i64>> {
    let mut deleted: HashMap<String, i64> = HashMap::new();

    let repaired = store.repair_stale_flow_paths()?;
    if repaired > 0 {
        deleted.insert("flows_repaired".to_string(), repaired);
    }

    // Ordered so a parent table is only pruned after the children that could
    // keep it alive; `communities` sits between flow_snapshots and
    // community_summaries in the Python original, so it runs here.
    for (table, predicate) in ORPHAN_PRUNE_STEPS {
        if *table == "community_summaries" {
            let stats: Value = serde_json::from_str(&refresh_community_stats_json(store)?)?;
            let updated = stats.get("updated").and_then(Value::as_i64).unwrap_or(0);
            let removed = stats.get("deleted").and_then(Value::as_i64).unwrap_or(0);
            if updated != 0 || removed != 0 {
                deleted.insert("communities".to_string(), updated + removed);
            }
        }
        let rows = store.prune_orphan_table(table, predicate)?;
        if rows > 0 {
            deleted.insert((*table).to_string(), rows);
        }
    }

    Ok(deleted)
}

/// `{table: rows_deleted}` as JSON, for the pyo3 boundary.
pub fn prune_orphaned_graph_structures_json(store: &mut GraphStore) -> Result<String> {
    let deleted = prune_orphaned_graph_structures(store)?;
    serde_json::to_string(&json!(deleted)).map_err(GraphError::from)
}
