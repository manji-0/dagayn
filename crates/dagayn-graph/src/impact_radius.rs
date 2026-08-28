//! Impact radius via SQLite recursive CTE.
//!
//! Port of `GraphStoreImpactMixin.get_impact_radius_sql`. Reportable
//! CROSS_ARTIFACT edges are traversed as first-class hops; low-confidence
//! bridges are omitted from expansion and reported as caveats instead.

use crate::bridges::*;
use crate::helpers::*;
use crate::*;

/// Expand through non-bridge edges and reportable CROSS_ARTIFACT only.
const REPORTABLE_BRIDGE_SQL: &str = "( \
     e.kind != 'CROSS_ARTIFACT' \
     OR ( \
         UPPER(COALESCE(e.confidence_tier, 'EXTRACTED')) IN ('EXACT', 'HIGH', 'EXTRACTED') \
         AND e.target_qualified NOT LIKE '<unresolved:%' \
         AND e.source_qualified NOT LIKE '<unresolved:%' \
     ) \
 )";

#[derive(Debug, Default)]
pub struct ImpactRadius {
    pub changed_nodes: Vec<GraphNode>,
    pub impacted_nodes: Vec<GraphNode>,
    pub impacted_files: Vec<String>,
    pub edges: Vec<GraphEdge>,
    pub bridge_transitions: Vec<Value>,
    pub low_confidence_bridges: Vec<Value>,
    pub truncated: bool,
    pub total_impacted: i64,
}

impl GraphStore {
    pub fn get_impact_radius(
        &self,
        changed_files: &[String],
        max_depth: i64,
        max_nodes: i64,
    ) -> Result<ImpactRadius> {
        if changed_files.is_empty() {
            return Ok(ImpactRadius::default());
        }

        let nodes_by_file = self.get_nodes_by_files(changed_files)?;
        let mut seeds = HashSet::new();
        for file_path in changed_files {
            for node in nodes_by_file.get(file_path).into_iter().flatten() {
                seeds.insert(node.qualified_name.clone());
            }
        }
        if seeds.is_empty() {
            return Ok(ImpactRadius::default());
        }

        let seed_list = seeds.iter().cloned().collect::<Vec<_>>();
        self.conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS _impact_seeds (qn TEXT PRIMARY KEY); \
             DELETE FROM _impact_seeds;",
        )?;
        self.insert_impact_seeds(&seed_list)?;

        let cte_sql = format!(
            "WITH RECURSIVE impacted(node_qn, depth) AS ( \
                 SELECT qn, 0 FROM _impact_seeds \
                 UNION \
                 SELECT e.target_qualified, i.depth + 1 \
                 FROM impacted i \
                 JOIN edges e ON e.source_qualified = i.node_qn \
                 WHERE i.depth < ? AND {REPORTABLE_BRIDGE_SQL} \
                 UNION \
                 SELECT e.source_qualified, i.depth + 1 \
                 FROM impacted i \
                 JOIN edges e ON e.target_qualified = i.node_qn \
                 WHERE i.depth < ? AND {REPORTABLE_BRIDGE_SQL} \
             ), \
             aggregated AS ( \
                 SELECT node_qn, MIN(depth) AS min_depth FROM impacted GROUP BY node_qn \
             )"
        );

        let total_impacted: i64 = self.conn.query_row(
            &format!(
                "{cte_sql} SELECT COUNT(*) FROM aggregated \
                 WHERE node_qn NOT IN (SELECT qn FROM _impact_seeds)"
            ),
            params![max_depth, max_depth],
            |row| row.get(0),
        )?;

        let mut depth_by_qn = Vec::new();
        {
            let sql = format!(
                "{cte_sql} SELECT node_qn, min_depth FROM aggregated \
                 WHERE node_qn NOT IN (SELECT qn FROM _impact_seeds) \
                 ORDER BY min_depth, node_qn LIMIT ?"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![max_depth, max_depth, max_nodes], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                depth_by_qn.push(row?);
            }
        }

        let depth_of = depth_by_qn
            .iter()
            .cloned()
            .collect::<HashMap<String, i64>>();
        let impacted_qns = depth_by_qn
            .iter()
            .map(|(qn, _)| qn.clone())
            .collect::<Vec<_>>();

        let changed_nodes = self.batch_get_nodes(&seed_list)?;
        let mut impacted_nodes = self.batch_get_nodes(&impacted_qns)?;
        impacted_nodes.sort_by(|left, right| {
            let left_key = (
                depth_of.get(&left.qualified_name).copied().unwrap_or(0),
                &left.qualified_name,
            );
            let right_key = (
                depth_of.get(&right.qualified_name).copied().unwrap_or(0),
                &right.qualified_name,
            );
            left_key.cmp(&right_key)
        });

        let mut impacted_files = Vec::new();
        let mut seen_files = HashSet::new();
        for node in &impacted_nodes {
            if seen_files.insert(node.file_path.clone()) {
                impacted_files.push(node.file_path.clone());
            }
        }

        if !impacted_nodes.is_empty() {
            let qns = impacted_nodes
                .iter()
                .map(|node| node.qualified_name.clone())
                .collect::<Vec<_>>();
            self.insert_impact_seeds(&qns)?;
        }

        let edges = {
            let mut stmt = self.conn.prepare(
                "SELECT e.* FROM edges e \
                 INNER JOIN _impact_seeds s ON e.source_qualified = s.qn \
                 INNER JOIN _impact_seeds t ON e.target_qualified = t.qn",
            )?;
            let rows = stmt.query_map([], edge_from_row)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let (bridge_transitions, _) = collect_bridge_transitions(&edges);
        let low_confidence_bridges = self.low_confidence_bridges_near_seeds(&seed_list)?;

        Ok(ImpactRadius {
            changed_nodes,
            impacted_nodes,
            impacted_files,
            edges,
            bridge_transitions,
            low_confidence_bridges,
            truncated: total_impacted > max_nodes,
            total_impacted,
        })
    }

    fn insert_impact_seeds(&self, qns: &[String]) -> Result<()> {
        for chunk in qns.chunks(450) {
            let placeholders = std::iter::repeat_n("(?)", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("INSERT OR IGNORE INTO _impact_seeds (qn) VALUES {placeholders}");
            self.conn.execute(&sql, rusqlite::params_from_iter(chunk))?;
        }
        Ok(())
    }

    /// Low-confidence CROSS_ARTIFACT edges touching a seed node, as caveats.
    fn low_confidence_bridges_near_seeds(&self, seeds: &[String]) -> Result<Vec<Value>> {
        let mut caveats = Vec::new();
        if seeds.is_empty() {
            return Ok(caveats);
        }
        let mut seen = HashSet::new();
        for chunk in seeds.chunks(450) {
            let placeholders = placeholder_list(chunk.len());
            let sql = format!(
                "SELECT * FROM edges WHERE kind = 'CROSS_ARTIFACT' \
                 AND (source_qualified IN ({placeholders}) \
                      OR target_qualified IN ({placeholders}))"
            );
            let mut params = chunk.iter().map(String::as_str).collect::<Vec<_>>();
            params.extend(chunk.iter().map(String::as_str));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), edge_from_row)?;
            for row in rows {
                let edge = row?;
                if !is_low_confidence_bridge(&edge) {
                    continue;
                }
                let key = (edge.source_qualified.clone(), edge.target_qualified.clone());
                if seen.insert(key) {
                    caveats.push(low_confidence_bridge_missingness(&edge));
                }
            }
        }
        Ok(caveats)
    }
}
