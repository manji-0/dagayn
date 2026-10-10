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

    /// A changed node's review-priority score (the retired Python
    /// `compute_risk_score`), with its inputs prefetched.
    pub fn compute_change_risk_score(&self, inputs: ChangeRiskInputs<'_>) -> Result<f64> {
        let mut score = 0.0_f64;

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
