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

    /// `dagayn.changes.compute_risk_score` with its inputs prefetched.
    pub fn compute_change_risk_score(&self, inputs: ChangeRiskInputs<'_>) -> Result<f64> {
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
            // One count per caller, as Python's `{qn: cid}` map keeps it.
            let callers: HashSet<&str> = caller_edges
                .iter()
                .map(|edge| edge.source_qualified.as_str())
                .collect();
            let cross_community = callers
                .into_iter()
                .filter(|caller| {
                    inputs
                        .caller_community_ids
                        .get(*caller)
                        .and_then(|cid| *cid)
                        .is_some_and(|cid| cid != node_cid)
                })
                .count();
            score += (cross_community as f64 * 0.05).min(0.15);
        }

        score += 0.30 - ((inputs.transitive_test_count as f64 / 5.0).min(1.0) * 0.25);

        if is_security_sensitive_identifier(&inputs.node.name, &inputs.node.qualified_name) {
            score += 0.20;
        }

        score += (caller_edges.len() as f64 / 20.0).min(0.10);
        // Python's `round(score, 4)`: correctly rounded, as formatting is.
        let clamped = score.clamp(0.0, 1.0);
        Ok(format!("{clamped:.4}").parse().unwrap_or(clamped))
    }
}
