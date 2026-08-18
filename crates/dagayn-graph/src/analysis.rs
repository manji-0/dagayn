use crate::helpers::*;
use crate::*;

type HubRow = (
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    Option<i64>,
    f64,
);

type BridgeRow = (String, String, String, String, f64, Option<i64>, f64);

fn is_documentation_node(node: &GraphNode) -> bool {
    if node.language.eq_ignore_ascii_case("markdown") {
        return true;
    }
    let path = node.file_path.replace('\\', "/").to_lowercase();
    path.ends_with(".md")
        || path.ends_with(".markdown")
        || path.ends_with(".mdown")
        || path.ends_with(".mkdn")
}

fn is_analysis_excluded_from_test_gap(node: &GraphNode) -> bool {
    if node.is_test || node.kind == "Test" || node.language.eq_ignore_ascii_case("markdown") {
        return true;
    }
    let path = node.file_path.replace('\\', "/");
    let normalized = format!("/{path}/");
    if normalized.contains("/tests/")
        || normalized.contains("/test/")
        || normalized.contains("/__tests__/")
    {
        return true;
    }
    let name = path.rsplit('/').next().unwrap_or("").to_lowercase();
    name.starts_with("test_")
        || name == "test.rs"
        || name == "tests.rs"
        || name.ends_with("_test.py")
        || name.ends_with("_tests.py")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || name.contains(".test.")
        || name.contains(".spec.")
        || normalized.contains("/test_")
}

fn node_in_code_scope(node: &GraphNode) -> bool {
    !is_documentation_node(node) && !is_analysis_excluded_from_test_gap(node)
}

fn compute_hub_rows(
    node_by_qn: &HashMap<String, GraphNode>,
    in_degree: &HashMap<String, i64>,
    out_degree: &HashMap<String, i64>,
    store: &GraphStore,
    now: f64,
    code_scope_only: bool,
) -> Result<Vec<HubRow>> {
    let mut hubs = Vec::new();
    for node in node_by_qn.values() {
        if code_scope_only && !node_in_code_scope(node) {
            continue;
        }
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
                store.get_node_community_id(node.id)?,
                now,
            ));
        }
    }
    hubs.sort_by(|a, b| b.6.cmp(&a.6).then_with(|| a.0.cmp(&b.0)));
    Ok(hubs)
}

fn compute_bridge_rows(
    node_by_qn: &HashMap<String, GraphNode>,
    graph_nodes: &HashSet<String>,
    adjacency: &HashMap<String, Vec<String>>,
    store: &GraphStore,
    now: f64,
    code_scope_only: bool,
) -> Result<Vec<BridgeRow>> {
    let scoped_nodes: HashSet<String> = if code_scope_only {
        graph_nodes
            .iter()
            .filter(|qn| node_by_qn.get(*qn).is_some_and(node_in_code_scope))
            .cloned()
            .collect()
    } else {
        graph_nodes.clone()
    };
    let scoped_adjacency: HashMap<String, Vec<String>> = if code_scope_only {
        adjacency
            .iter()
            .filter(|(source, _)| scoped_nodes.contains(*source))
            .map(|(source, targets)| {
                (
                    source.clone(),
                    targets
                        .iter()
                        .filter(|target| scoped_nodes.contains(*target))
                        .cloned()
                        .collect(),
                )
            })
            .collect()
    } else {
        adjacency.clone()
    };

    let bridge_scores = betweenness_centrality(&scoped_nodes, &scoped_adjacency);
    let mut bridges = Vec::new();
    for (qualified_name, score) in bridge_scores {
        if score <= 0.0 {
            continue;
        }
        if let Some(node) = node_by_qn.get(&qualified_name) {
            if code_scope_only && !node_in_code_scope(node) {
                continue;
            }
            bridges.push((
                node.qualified_name.clone(),
                sanitize_name(&node.name),
                node.kind.clone(),
                node.file_path.clone(),
                (score * 1_000_000.0).round() / 1_000_000.0,
                store.get_node_community_id(node.id)?,
                now,
            ));
        }
    }
    bridges.sort_by(|a, b| b.4.total_cmp(&a.4).then_with(|| a.0.cmp(&b.0)));
    Ok(bridges)
}

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

        let hubs = compute_hub_rows(&node_by_qn, &in_degree, &out_degree, self, now, false)?;
        let bridges = compute_bridge_rows(
            &node_by_qn,
            &graph_nodes,
            &adjacency,
            self,
            now,
            false,
        )?;
        let hubs_code = compute_hub_rows(&node_by_qn, &in_degree, &out_degree, self, now, true)?;
        let bridges_code = compute_bridge_rows(
            &node_by_qn,
            &graph_nodes,
            &adjacency,
            self,
            now,
            true,
        )?;

        let tx = write_tx(&mut self.conn)?;
        tx.execute("DELETE FROM hub_scores", [])?;
        tx.execute("DELETE FROM bridge_scores", [])?;
        tx.execute("DELETE FROM hub_scores_code", [])?;
        tx.execute("DELETE FROM bridge_scores_code", [])?;
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
        {
            let mut stmt = tx.prepare(
                "INSERT INTO hub_scores_code \
                 (qualified_name, name, kind, file_path, in_degree, out_degree, total_degree, \
                  community_id, computed_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            for hub in &hubs_code {
                stmt.execute(params![
                    &hub.0, &hub.1, &hub.2, &hub.3, hub.4, hub.5, hub.6, hub.7, hub.8
                ])?;
            }
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO bridge_scores_code \
                 (qualified_name, name, kind, file_path, betweenness, community_id, computed_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;
            for bridge in &bridges_code {
                stmt.execute(params![
                    &bridge.0, &bridge.1, &bridge.2, &bridge.3, bridge.4, bridge.5, bridge.6
                ])?;
            }
        }
        tx.commit()?;

        Ok(HashMap::from([
            ("hub_scores_persisted".to_string(), hubs.len() as i64),
            ("bridge_scores_persisted".to_string(), bridges.len() as i64),
            (
                "hub_scores_code_persisted".to_string(),
                hubs_code.len() as i64,
            ),
            (
                "bridge_scores_code_persisted".to_string(),
                bridges_code.len() as i64,
            ),
        ]))
    }
}
