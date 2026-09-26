use crate::helpers::*;
use crate::*;

const NODE_QUALIFIED_EDGE_KINDS: [&str; 6] = [
    "CALLS",
    "INHERITS",
    "IMPLEMENTS",
    "CONTAINS",
    "REFERENCES",
    "TESTED_BY",
];

impl GraphStore {
    pub fn demote_unresolved_endpoint_edges(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        let placeholders = std::iter::repeat_n("?", NODE_QUALIFIED_EDGE_KINDS.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE edges
             SET confidence = MIN(confidence, 0.2),
                 confidence_tier = 'LOW'
             WHERE kind IN ({placeholders})
               AND UPPER(COALESCE(confidence_tier, 'EXTRACTED')) NOT IN ('LOW', 'UNKNOWN')
               AND (
                 target_qualified LIKE '<unresolved:%'
                 OR source_qualified LIKE '<unresolved:%'
                 OR NOT EXISTS (
                     SELECT 1 FROM nodes n WHERE n.qualified_name = edges.target_qualified
                 )
                 OR NOT EXISTS (
                     SELECT 1 FROM nodes n WHERE n.qualified_name = edges.source_qualified
                 )
               )"
        );
        let updated = tx.execute(&sql, rusqlite::params_from_iter(NODE_QUALIFIED_EDGE_KINDS))?;
        tx.commit()?;
        Ok(updated as i64)
    }
}
