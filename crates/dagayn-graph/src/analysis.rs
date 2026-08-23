use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn persist_centrality_scores(&mut self) -> Result<HashMap<String, i64>> {
        self.persist_centrality_scores_filtered(None)
    }

    pub fn persist_centrality_scores_filtered(
        &mut self,
        changed_files: Option<&[String]>,
    ) -> Result<HashMap<String, i64>> {
        let nodes = self.get_all_nodes_filtered(true)?;
        let edges = self.get_all_edges()?;
        self.persist_centrality_from_graph(&nodes, &edges, changed_files)
    }

    pub fn persist_centrality_from_graph(
        &mut self,
        nodes: &[GraphNode],
        edges: &[GraphEdge],
        changed_files: Option<&[String]>,
    ) -> Result<HashMap<String, i64>> {
        self.conn.execute_batch(CENTRALITY_SCORE_SCHEMA_SQL)?;
        let now = now_seconds()?;

        let region = match changed_files {
            Some(files) if !files.is_empty() => self.community_region_qualified_names(files)?,
            _ => None,
        };

        let mut node_by_qn = HashMap::<String, GraphNode>::new();
        for node in nodes {
            if region
                .as_ref()
                .is_none_or(|allowed| allowed.contains(&node.qualified_name))
            {
                node_by_qn.insert(node.qualified_name.clone(), node.clone());
            }
        }

        let mut in_degree = HashMap::<String, i64>::new();
        let mut out_degree = HashMap::<String, i64>::new();
        let mut adjacency = HashMap::<String, Vec<String>>::new();
        let mut graph_nodes = HashSet::<String>::new();
        for edge in edges {
            let src_in = region
                .as_ref()
                .is_none_or(|allowed| allowed.contains(&edge.source_qualified));
            let tgt_in = region
                .as_ref()
                .is_none_or(|allowed| allowed.contains(&edge.target_qualified));
            if src_in {
                *out_degree.entry(edge.source_qualified.clone()).or_insert(0) += 1;
            }
            if tgt_in {
                *in_degree.entry(edge.target_qualified.clone()).or_insert(0) += 1;
            }
            if src_in && tgt_in {
                adjacency
                    .entry(edge.source_qualified.clone())
                    .or_default()
                    .push(edge.target_qualified.clone());
                graph_nodes.insert(edge.source_qualified.clone());
                graph_nodes.insert(edge.target_qualified.clone());
            }
        }

        let mut hubs = Vec::new();
        let node_ids: Vec<i64> = node_by_qn.values().map(|node| node.id).collect();
        let community_by_id = self.get_community_ids_by_node_ids(&node_ids)?;
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
                    community_by_id.get(&node.id).copied().flatten(),
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
                    community_by_id.get(&node.id).copied().flatten(),
                    now,
                ));
            }
        }
        bridges.sort_by(|a, b| b.4.total_cmp(&a.4).then_with(|| a.0.cmp(&b.0)));

        let tx = write_tx(&mut self.conn)?;
        if let Some(allowed) = region.as_ref() {
            let names: Vec<String> = allowed.iter().cloned().collect();
            delete_scores_for_qualified_names(&tx, "hub_scores", &names)?;
            delete_scores_for_qualified_names(&tx, "bridge_scores", &names)?;
        } else {
            tx.execute("DELETE FROM hub_scores", [])?;
            tx.execute("DELETE FROM bridge_scores", [])?;
        }
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

fn delete_scores_for_qualified_names(
    tx: &Transaction<'_>,
    table: &str,
    names: &[String],
) -> Result<()> {
    for chunk in names.chunks(450) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("DELETE FROM {table} WHERE qualified_name IN ({placeholders})");
        tx.execute(&sql, rusqlite::params_from_iter(chunk))?;
    }
    Ok(())
}
