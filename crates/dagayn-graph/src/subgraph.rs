//! Subgraph extraction mirroring the Python `GraphStore` surface.

use crate::*;

/// `(nodes_by_qualified_name, bidirectional_adjacency)`.
pub type LocalSubgraph = (HashMap<String, GraphNode>, HashMap<String, Vec<String>>);

impl GraphStore {
    /// The nodes named in `qualified_names` plus the edges wholly inside that set.
    pub fn get_subgraph(
        &self,
        qualified_names: &[String],
    ) -> Result<(Vec<GraphNode>, Vec<GraphEdge>)> {
        let nodes_by_qn = self.get_nodes_by_qualified_names(qualified_names)?;
        let nodes = qualified_names
            .iter()
            .filter_map(|qn| nodes_by_qn.get(qn).cloned())
            .collect::<Vec<_>>();

        let qn_set = qualified_names.iter().cloned().collect::<HashSet<_>>();
        let (outgoing, _) =
            self.get_edges_by_endpoints(&qn_set.iter().cloned().collect::<Vec<_>>())?;
        let edges = outgoing
            .into_values()
            .flatten()
            .filter(|edge| qn_set.contains(&edge.target_qualified))
            .collect::<Vec<_>>();

        Ok((nodes, edges))
    }

    /// Every node reachable from `start_qn` within `max_depth` hops in either
    /// direction, plus an undirected adjacency map over that set.
    ///
    /// One recursive CTE instead of a round-trip per node.
    pub fn get_local_subgraph(&self, start_qn: &str, max_depth: i64) -> Result<LocalSubgraph> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE reach(qn, depth) AS ( \
                 SELECT ?, 0 \
                 UNION \
                 SELECT e.target_qualified, r.depth + 1 \
                 FROM reach r JOIN edges e ON e.source_qualified = r.qn \
                 WHERE r.depth < ? \
                 UNION \
                 SELECT e.source_qualified, r.depth + 1 \
                 FROM reach r JOIN edges e ON e.target_qualified = r.qn \
                 WHERE r.depth < ? \
             ) \
             SELECT DISTINCT qn FROM reach",
        )?;
        let rows = stmt.query_map(params![start_qn, max_depth, max_depth], |row| {
            row.get::<_, String>(0)
        })?;
        let mut all_qns = HashSet::new();
        for row in rows {
            all_qns.insert(row?);
        }

        let qns = all_qns.iter().cloned().collect::<Vec<_>>();
        let mut nodes_map = HashMap::new();
        for node in self.batch_get_nodes(&qns)? {
            nodes_map.insert(node.qualified_name.clone(), node);
        }

        let mut adjacency = qns
            .iter()
            .map(|qn| (qn.clone(), Vec::new()))
            .collect::<HashMap<String, Vec<String>>>();
        for edge in self.get_edges_among(&all_qns)? {
            if let Some(neighbors) = adjacency.get_mut(&edge.source_qualified) {
                neighbors.push(edge.target_qualified.clone());
            }
            if let Some(neighbors) = adjacency.get_mut(&edge.target_qualified) {
                neighbors.push(edge.source_qualified.clone());
            }
        }

        Ok((nodes_map, adjacency))
    }
}
