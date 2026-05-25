use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn persist_centrality_scores(&mut self) -> Result<HashMap<String, i64>> {
        self.conn.execute_batch(CENTRALITY_SCORE_SCHEMA_SQL)?;
        let now = now_seconds()?;
        let nodes = self.get_all_nodes_filtered(true)?;
        let edges = self.get_all_edges()?;

        let mut node_by_qn = HashMap::<String, GraphNode>::new();
        for node in nodes {
            node_by_qn.insert(node.qualified_name.clone(), node);
        }

        let mut in_degree = HashMap::<String, i64>::new();
        let mut out_degree = HashMap::<String, i64>::new();
        let mut adjacency = HashMap::<String, Vec<String>>::new();
        let mut graph_nodes = HashSet::<String>::new();
        for edge in &edges {
            *out_degree.entry(edge.source_qualified.clone()).or_insert(0) += 1;
            *in_degree.entry(edge.target_qualified.clone()).or_insert(0) += 1;
            adjacency
                .entry(edge.source_qualified.clone())
                .or_default()
                .push(edge.target_qualified.clone());
            graph_nodes.insert(edge.source_qualified.clone());
            graph_nodes.insert(edge.target_qualified.clone());
        }

        let mut hubs = Vec::new();
        for node in node_by_qn.values() {
            let ind = *in_degree.get(&node.qualified_name).unwrap_or(&0);
            let outd = *out_degree.get(&node.qualified_name).unwrap_or(&0);
            let total = ind + outd;
            if total > 0 {
                hubs.push((
                    node.qualified_name.clone(),
                    sanitize_name(&node.name),
                    node.kind.clone(),
                    node.file_path.clone(),
                    ind,
                    outd,
                    total,
                    self.get_node_community_id(node.id)?,
                    now,
                ));
            }
        }
        hubs.sort_by(|a, b| b.6.cmp(&a.6).then_with(|| a.0.cmp(&b.0)));

        let bridge_scores = betweenness_centrality(&graph_nodes, &adjacency);
        let mut bridges = Vec::new();
        for (qualified_name, score) in bridge_scores {
            if score <= 0.0 {
                continue;
            }
            if let Some(node) = node_by_qn.get(&qualified_name) {
                bridges.push((
                    node.qualified_name.clone(),
                    sanitize_name(&node.name),
                    node.kind.clone(),
                    node.file_path.clone(),
                    (score * 1_000_000.0).round() / 1_000_000.0,
                    self.get_node_community_id(node.id)?,
                    now,
                ));
            }
        }
        bridges.sort_by(|a, b| b.4.total_cmp(&a.4).then_with(|| a.0.cmp(&b.0)));

        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM hub_scores", [])?;
        tx.execute("DELETE FROM bridge_scores", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO hub_scores \
                 (qualified_name, name, kind, file_path, in_degree, out_degree, total_degree, \
                  community_id, computed_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            for hub in &hubs {
                stmt.execute(params![
                    &hub.0, &hub.1, &hub.2, &hub.3, hub.4, hub.5, hub.6, hub.7, hub.8
                ])?;
            }
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO bridge_scores \
                 (qualified_name, name, kind, file_path, betweenness, community_id, computed_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;
            for bridge in &bridges {
                stmt.execute(params![
                    &bridge.0, &bridge.1, &bridge.2, &bridge.3, bridge.4, bridge.5, bridge.6
                ])?;
            }
        }
        tx.commit()?;

        Ok(HashMap::from([
            ("hub_scores_persisted".to_string(), hubs.len() as i64),
            ("bridge_scores_persisted".to_string(), bridges.len() as i64),
        ]))
    }
}
