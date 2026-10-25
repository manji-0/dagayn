//! Subgraph extraction mirroring the Python `GraphStore` surface.

use crate::*;

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
}
