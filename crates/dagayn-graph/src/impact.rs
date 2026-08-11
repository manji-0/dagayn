use crate::helpers::*;
use crate::*;
use serde::Serialize;

#[derive(Debug)]
enum ChangedRangeInput {
    Empty,
    Parsed(ChangedRanges),
}

impl ChangedRangeInput {
    fn parse(raw: Option<&str>) -> Result<Self> {
        match raw {
            Some(raw) if !raw.is_empty() => Ok(Self::Parsed(serde_json::from_str(raw)?)),
            _ => Ok(Self::Empty),
        }
    }

    fn as_ranges(&self) -> Option<&ChangedRanges> {
        match self {
            Self::Empty => None,
            Self::Parsed(ranges) => Some(ranges),
        }
    }
}

#[derive(Serialize)]
struct ChangeAnalysisJson {
    summary: String,
    risk_score: f64,
    changed_functions: Vec<Value>,
    affected_flows: Vec<Value>,
    test_gaps: Vec<Value>,
    review_priorities: Vec<Value>,
}

#[derive(Serialize)]
struct TestGapJson {
    name: String,
    qualified_name: String,
    file: String,
    line_start: i64,
    line_end: i64,
}

impl GraphStore {
    pub(crate) fn get_affected_flow_values(&self, changed_files: &[String]) -> Result<Vec<Value>> {
        if changed_files.is_empty() {
            return Ok(Vec::new());
        }
        let flow_ids = self.get_affected_flow_ids(changed_files)?;
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
        let changed_ranges = ChangedRangeInput::parse(changed_ranges_json)?;
        let changed_nodes = if let Some(ranges) = changed_ranges.as_ranges() {
            self.changed_nodes_by_ranges(ranges)?
        } else {
            self.changed_nodes_by_files(changed_files)?
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
                test_gaps.push(json!(TestGapJson {
                    name: sanitize_name(&node.name),
                    qualified_name: sanitize_name(&node.qualified_name),
                    file: node.file_path.clone(),
                    line_start: node.line_start,
                    line_end: node.line_end,
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

        serde_json::to_string(&ChangeAnalysisJson {
            summary: summary_parts.join("\n"),
            risk_score: overall_risk,
            changed_functions: node_risks,
            affected_flows,
            test_gaps,
            review_priorities,
        })
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: &str, name: &str, file_path: &str, line_start: i64, line_end: i64) -> NodeInput {
        NodeInput {
            kind: kind.to_string(),
            name: name.to_string(),
            file_path: file_path.to_string(),
            line_start,
            line_end,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: kind == "Test",
            extra: Value::Object(Default::default()),
        }
    }

    fn edge(kind: &str, source: &str, target: &str, file_path: &str, line: i64) -> EdgeInput {
        EdgeInput {
            kind: kind.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            file_path: file_path.to_string(),
            line,
            extra: Value::Object(Default::default()),
        }
    }

    fn populated_store() -> GraphStore {
        let mut store = GraphStore::open(":memory:").expect("open graph store");
        store
            .store_file_batch(&[
                (
                    "app.py".to_string(),
                    vec![
                        node("File", "app.py", "app.py", 1, 1),
                        node("Function", "auth_token", "app.py", 10, 20),
                        node("Function", "helper", "app.py", 30, 35),
                    ],
                    vec![
                        edge(
                            "CALLS",
                            "caller.py::caller",
                            "app.py::auth_token",
                            "caller.py",
                            3,
                        ),
                        edge(
                            "TESTED_BY",
                            "app.py::helper",
                            "test_app.py::test_helper",
                            "test_app.py",
                            4,
                        ),
                    ],
                    "app-hash".to_string(),
                    0,
                ),
                (
                    "caller.py".to_string(),
                    vec![
                        node("File", "caller.py", "caller.py", 1, 1),
                        node("Function", "caller", "caller.py", 1, 5),
                    ],
                    Vec::new(),
                    "caller-hash".to_string(),
                    0,
                ),
                (
                    "test_app.py".to_string(),
                    vec![
                        node("File", "test_app.py", "test_app.py", 1, 1),
                        node("Test", "test_helper", "test_app.py", 1, 5),
                    ],
                    Vec::new(),
                    "test-hash".to_string(),
                    0,
                ),
            ])
            .expect("store fixture graph");

        let auth_id = store
            .get_node("app.py::auth_token")
            .expect("read auth node")
            .expect("auth node exists")
            .id;
        let helper_id = store
            .get_node("app.py::helper")
            .expect("read helper node")
            .expect("helper node exists")
            .id;
        store
            .store_flows(&[
                FlowInput {
                    name: "auth flow".to_string(),
                    entry_point_id: auth_id,
                    depth: 0,
                    node_count: 1,
                    file_count: 1,
                    criticality: 0.25,
                    path: vec![auth_id],
                },
                FlowInput {
                    name: "helper flow".to_string(),
                    entry_point_id: helper_id,
                    depth: 0,
                    node_count: 1,
                    file_count: 1,
                    criticality: 0.75,
                    path: vec![helper_id],
                },
            ])
            .expect("store flows");
        store
    }

    #[test]
    fn get_affected_flow_values_returns_matching_flows_by_criticality() {
        let store = populated_store();

        let flows = store
            .get_affected_flow_values(&["app.py".to_string()])
            .expect("affected flows");

        assert_eq!(flows.len(), 2);
        assert_eq!(flows[0]["name"], json!("helper flow"));
        assert_eq!(flows[0]["criticality"], json!(0.75));
        assert_eq!(
            flows[0]["steps"][0]["qualified_name"],
            json!("app.py::helper")
        );
        assert_eq!(flows[1]["name"], json!("auth flow"));
        assert_eq!(flows[1]["criticality"], json!(0.25));
        assert_eq!(
            flows[1]["steps"][0]["qualified_name"],
            json!("app.py::auth_token")
        );
    }

    #[test]
    fn analyze_changes_json_returns_summary_risk_gaps_and_priorities() {
        let store = populated_store();

        let analysis: Value = serde_json::from_str(
            &store
                .analyze_changes_json(&["app.py".to_string()], None)
                .expect("analyze changes"),
        )
        .expect("analysis json");

        let summary = analysis["summary"].as_str().expect("summary string");
        assert!(summary.contains("Analyzed 1 changed file(s):"));
        assert!(summary.contains("2 changed function(s)/class(es)"));
        assert!(summary.contains("2 affected flow(s)"));
        assert!(summary.contains("1 test gap(s)"));
        assert!(summary.contains("Overall risk score: 0.80"));

        assert_eq!(analysis["risk_score"], json!(0.8));

        let changed_functions = analysis["changed_functions"]
            .as_array()
            .expect("changed functions array");
        assert_eq!(changed_functions.len(), 2);
        assert!(changed_functions.iter().any(|node| {
            node["qualified_name"] == json!("app.py::auth_token")
                && node["risk_score"] == json!(0.8)
        }));
        assert!(changed_functions.iter().any(|node| {
            node["qualified_name"] == json!("app.py::helper") && node["risk_score"] == json!(0.5)
        }));

        let affected_flows = analysis["affected_flows"]
            .as_array()
            .expect("affected flows array");
        assert_eq!(affected_flows.len(), 2);
        assert_eq!(affected_flows[0]["name"], json!("helper flow"));
        assert_eq!(affected_flows[1]["name"], json!("auth flow"));

        let test_gaps = analysis["test_gaps"].as_array().expect("test gaps array");
        assert_eq!(test_gaps.len(), 1);
        assert_eq!(test_gaps[0]["name"], json!("auth_token"));
        assert_eq!(test_gaps[0]["qualified_name"], json!("app.py::auth_token"));
        assert_eq!(test_gaps[0]["file"], json!("app.py"));

        let review_priorities = analysis["review_priorities"]
            .as_array()
            .expect("review priorities array");
        assert_eq!(review_priorities.len(), 2);
        assert_eq!(
            review_priorities[0]["qualified_name"],
            json!("app.py::auth_token")
        );
        assert_eq!(review_priorities[0]["risk_score"], json!(0.8));
        assert_eq!(
            review_priorities[1]["qualified_name"],
            json!("app.py::helper")
        );
        assert_eq!(review_priorities[1]["risk_score"], json!(0.5));
    }

    #[test]
    fn impact_analysis_empty_inputs_return_empty_json_shapes() {
        let store = GraphStore::open(":memory:").expect("open graph store");

        assert_eq!(
            store
                .get_affected_flow_values(&[])
                .expect("empty affected flows"),
            Vec::<Value>::new()
        );
        assert_eq!(
            store
                .get_affected_flow_values(&["missing.py".to_string()])
                .expect("missing affected flows"),
            Vec::<Value>::new()
        );

        let analysis: Value = serde_json::from_str(
            &store
                .analyze_changes_json(&[], Some(""))
                .expect("empty analysis"),
        )
        .expect("analysis json");

        assert!(analysis["summary"]
            .as_str()
            .expect("summary string")
            .contains("Analyzed 0 changed file(s):"));
        assert_eq!(analysis["risk_score"], json!(0.0));
        assert_eq!(
            analysis["changed_functions"]
                .as_array()
                .expect("changed functions array"),
            &Vec::<Value>::new()
        );
        assert_eq!(
            analysis["affected_flows"]
                .as_array()
                .expect("affected flows array"),
            &Vec::<Value>::new()
        );
        assert_eq!(
            analysis["test_gaps"].as_array().expect("test gaps array"),
            &Vec::<Value>::new()
        );
        assert_eq!(
            analysis["review_priorities"]
                .as_array()
                .expect("review priorities array"),
            &Vec::<Value>::new()
        );
    }
}
