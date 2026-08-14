use crate::*;

impl GraphStore {
    pub(crate) fn test_node_json(
        &self,
        qualified_name: &str,
        indirect: bool,
    ) -> Result<Option<Value>> {
        self.conn
            .query_row(
                "SELECT name, qualified_name, file_path, kind FROM nodes \
                 WHERE qualified_name = ?",
                [qualified_name],
                |row| {
                    Ok(json!({
                        "name": row.get::<_, String>(0)?,
                        "qualified_name": row.get::<_, String>(1)?,
                        "file_path": row.get::<_, String>(2)?,
                        "kind": row.get::<_, String>(3)?,
                        "indirect": indirect,
                    }))
                },
            )
            .optional()
            .map_err(Into::into)
    }

    #[allow(dead_code)] // Mirrors Python GraphStore.get_node_ids_by_files for later pyo3 wiring.
    pub(crate) fn get_node_ids_by_files(&self, file_paths: &[String]) -> Result<HashSet<i64>> {
        let mut out = HashSet::new();
        let file_keys = self.expand_file_keys(file_paths)?;
        for chunk in file_keys.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id FROM nodes WHERE file_path IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| row.get(0))?;
            for row in rows {
                out.insert(row?);
            }
        }
        Ok(out)
    }

    pub(crate) fn expand_file_keys(&self, file_paths: &[String]) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut seen = HashSet::new();
        for file_path in file_paths {
            for key in self.file_key_candidates(file_path)? {
                if seen.insert(key.clone()) {
                    keys.push(key);
                }
            }
        }
        Ok(keys)
    }

    pub(crate) fn changed_nodes_by_files(
        &self,
        changed_files: &[String],
    ) -> Result<Vec<GraphNode>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let nodes_by_file = self.get_nodes_by_files(changed_files)?;
        for file_path in changed_files {
            if let Some(nodes) = nodes_by_file.get(file_path) {
                for node in nodes {
                    if seen.insert(node.qualified_name.clone()) {
                        out.push(node.clone());
                    }
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn changed_nodes_by_ranges(
        &self,
        changed_ranges: &ChangedRanges,
    ) -> Result<Vec<GraphNode>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let file_paths = changed_ranges.keys().cloned().collect::<Vec<_>>();
        let nodes_by_file = self.get_nodes_by_files(&file_paths)?;
        for (file_path, ranges) in changed_ranges {
            let mut nodes = nodes_by_file.get(file_path).cloned().unwrap_or_default();
            if nodes.is_empty() {
                let matched_paths = self.get_files_matching(file_path)?;
                let matched_nodes = self.get_nodes_by_files(&matched_paths)?;
                for matched_path in matched_paths {
                    if let Some(found) = matched_nodes.get(&matched_path) {
                        nodes.extend(found.iter().cloned());
                    }
                }
            }
            for node in nodes {
                if seen.contains(&node.qualified_name) {
                    continue;
                }
                if ranges
                    .iter()
                    .any(|(start, end)| node.line_start <= *end && node.line_end >= *start)
                    && seen.insert(node.qualified_name.clone())
                {
                    out.push(node);
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn compute_change_risk_score(&self, inputs: ChangeRiskInputs<'_>) -> Result<f64> {
        let mut score = 0.0_f64;

        if inputs.flow_criticalities.is_empty() {
            score += (inputs.flow_count as f64 * 0.05).min(0.25);
        } else {
            score += inputs.flow_criticalities.iter().sum::<f64>().min(0.25);
        }

        let caller_edges = inputs
            .inbound_edges
            .iter()
            .filter(|edge| edge.kind == "CALLS")
            .collect::<Vec<_>>();
        if let Some(node_cid) = inputs.node_community_id {
            let cross_community = caller_edges
                .iter()
                .filter(|edge| {
                    inputs
                        .caller_community_ids
                        .get(&edge.source_qualified)
                        .and_then(|cid| *cid)
                        .is_some_and(|cid| cid != node_cid)
                })
                .count();
            score += (cross_community as f64 * 0.05).min(0.15);
        }

        score += 0.30 - ((inputs.transitive_test_count as f64 / 5.0).min(1.0) * 0.25);

        let name_lower = inputs.node.name.to_lowercase();
        let qn_lower = inputs.node.qualified_name.to_lowercase();
        if SECURITY_KEYWORDS
            .iter()
            .any(|keyword| name_lower.contains(keyword) || qn_lower.contains(keyword))
        {
            score += 0.20;
        }

        score += (caller_edges.len() as f64 / 20.0).min(0.10);
        Ok((score.clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0)
    }
}
