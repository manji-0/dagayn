//! Repository-wide sweep of derived rows orphaned by re-parses.

use std::collections::HashMap;

use dagayn_graph::{GraphError, GraphStore, ORPHAN_PRUNE_STEPS};
use serde_json::Value;

use crate::communities::refresh_community_stats_json;

type Result<T> = std::result::Result<T, GraphError>;

/// Delete derived rows whose nodes no longer exist.
///
/// Re-parsing a file deletes its nodes and inserts new ones with fresh
/// autoincrement ids, so every re-parse orphans the community assignments and
/// risk rows that pointed at the old ids. Nothing else removes them:
/// `remove_files_data` drops nodes and edges only, and community detection
/// runs at `postprocess=full`.
///
/// Lives here rather than in `dagayn-graph` because the `communities` step is
/// [`refresh_community_stats_json`], which needs the Leiden cohesion code.
///
/// Returns `{table: rows_deleted}` for the tables that lost rows.
pub fn prune_orphaned_graph_structures(store: &mut GraphStore) -> Result<HashMap<String, i64>> {
    let mut deleted: HashMap<String, i64> = HashMap::new();

    // Ordered so a parent table is only pruned after the children that could
    // keep it alive; `communities` goes before community_summaries.
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
