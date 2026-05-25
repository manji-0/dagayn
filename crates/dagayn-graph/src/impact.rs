use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub(crate) fn get_affected_flow_values(&self, changed_files: &[String]) -> Result<Vec<Value>> {
        if changed_files.is_empty() {
            return Ok(Vec::new());
        }
        let node_ids = self.get_node_ids_by_files(changed_files)?;
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let flow_ids = self.get_flow_ids_by_node_ids(&node_ids)?;
        if flow_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut flows = self.get_flow_values_by_ids(&flow_ids)?;
        flows.sort_by(|left, right| {
            let left = left
                .get("criticality")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let right = right
                .get("criticality")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            right
                .partial_cmp(&left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(flows)
    }

    pub fn analyze_changes_json(
        &self,
        changed_files: &[String],
        changed_ranges_json: Option<&str>,
    ) -> Result<String> {
        let changed_ranges = match changed_ranges_json {
            Some(raw) if !raw.is_empty() => serde_json::from_str::<ChangedRanges>(raw)?,
            _ => HashMap::new(),
        };
        let changed_nodes = if changed_ranges.is_empty() {
            self.changed_nodes_by_files(changed_files)?
        } else {
            self.changed_nodes_by_ranges(&changed_ranges)?
        };
        let changed_funcs = changed_nodes
            .into_iter()
            .filter(|node| matches!(node.kind.as_str(), "Function" | "Test" | "Class"))
            .collect::<Vec<_>>();

        let func_ids = changed_funcs.iter().map(|node| node.id).collect::<Vec<_>>();
        let func_qns = changed_funcs
            .iter()
            .map(|node| node.qualified_name.clone())
            .collect::<Vec<_>>();

        let flow_crit_map = self.get_flow_criticalities_for_nodes(&func_ids)?;
        let nodes_needing_count = flow_crit_map
            .iter()
            .filter_map(|(node_id, values)| {
                if values.is_empty() {
                    Some(*node_id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let flow_count_map = if nodes_needing_count.is_empty() {
            HashMap::new()
        } else {
            self.count_flow_memberships_for_nodes(&nodes_needing_count)?
        };
        let node_cid_map = self.get_community_ids_by_node_ids(&func_ids)?;
        let (outbound_map, inbound_map) = self.get_edges_by_endpoints(&func_qns)?;

        let mut caller_qns = HashSet::new();
        for edges in inbound_map.values() {
            for edge in edges {
                if edge.kind == "CALLS" {
                    caller_qns.insert(edge.source_qualified.clone());
                }
            }
        }
        let caller_qns = caller_qns.into_iter().collect::<Vec<_>>();
        let caller_cid_map = if caller_qns.is_empty() {
            HashMap::new()
        } else {
            self.get_community_ids_by_qualified_names(&caller_qns)?
        };
        let transitive_test_counts = self.get_transitive_test_counts(&func_qns, 1)?;

        let mut node_risks = Vec::new();
        for node in &changed_funcs {
            let inbound_edges = inbound_map
                .get(&node.qualified_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let flow_criticalities = flow_crit_map
                .get(&node.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let flow_count = *flow_count_map.get(&node.id).unwrap_or(&0);
            let risk = self.compute_change_risk_score(ChangeRiskInputs {
                node,
                inbound_edges,
                flow_criticalities,
                flow_count,
                node_community_id: node_cid_map.get(&node.id).copied().flatten(),
                caller_community_ids: &caller_cid_map,
                transitive_test_count: *transitive_test_counts
                    .get(&node.qualified_name)
                    .unwrap_or(&0),
            })?;
            let mut value = node_to_value(node);
            if let Some(obj) = value.as_object_mut() {
                obj.insert("risk_score".to_string(), json!(risk));
            }
            node_risks.push(value);
        }

        let overall_risk = node_risks
            .iter()
            .filter_map(|value| value.get("risk_score").and_then(Value::as_f64))
            .fold(0.0, f64::max);
        let affected_flows = self.get_affected_flow_values(changed_files)?;

        let mut test_gaps = Vec::new();
        for node in &changed_funcs {
            if node.is_test {
                continue;
            }
            let tested = outbound_map
                .get(&node.qualified_name)
                .map(|edges| edges.iter().any(|edge| edge.kind == "TESTED_BY"))
                .unwrap_or(false);
            if !tested {
                test_gaps.push(json!({
                    "name": sanitize_name(&node.name),
                    "qualified_name": sanitize_name(&node.qualified_name),
                    "file": node.file_path,
                    "line_start": node.line_start,
                    "line_end": node.line_end,
                }));
            }
        }

        let mut review_priorities = node_risks.clone();
        review_priorities.sort_by(|left, right| {
            let left = left
                .get("risk_score")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let right = right
                .get("risk_score")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            right
                .partial_cmp(&left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        review_priorities.truncate(10);

        let mut summary_parts = vec![
            format!("Analyzed {} changed file(s):", changed_files.len()),
            format!("  - {} changed function(s)/class(es)", changed_funcs.len()),
            format!("  - {} affected flow(s)", affected_flows.len()),
            format!("  - {} test gap(s)", test_gaps.len()),
            format!("  - Overall risk score: {overall_risk:.2}"),
        ];
        if !test_gaps.is_empty() {
            let gap_names = test_gaps
                .iter()
                .take(5)
                .filter_map(|gap| gap.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ");
            summary_parts.push(format!("  - Untested: {gap_names}"));
        }

        serde_json::to_string(&json!({
            "summary": summary_parts.join("\n"),
            "risk_score": overall_risk,
            "changed_functions": node_risks,
            "affected_flows": affected_flows,
            "test_gaps": test_gaps,
            "review_priorities": review_priorities,
        }))
        .map_err(Into::into)
    }
}
