//! Flow tracing (BFS reachable sets) and criticality scoring.

use std::collections::{HashMap, HashSet, VecDeque};

use dagayn_graph::{GraphError, GraphNode, GraphStore, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::entry_points::{has_framework_decorator, is_test_file, matches_entry_name};

pub const DEFAULT_FLOW_MAX_DEPTH: i64 = 15;
pub const DEFAULT_FLOW_MAX_NODES: i64 = 512;
const FLOW_KIND_REACHABLE_SET: &str = "reachable_set";

const SECURITY_KEYWORDS: &[&str] = &[
    "auth", "login", "password", "token", "session", "crypt", "secret", "credential",
    "permission", "sql", "query", "execute", "connect", "socket", "request", "http",
    "sanitize", "validate", "encrypt", "decrypt", "hash", "sign", "verify", "admin",
    "privilege",
];

#[derive(Clone, Debug)]
struct FlowAdjacency {
    calls_out: HashMap<String, Vec<String>>,
    has_tested_by: HashSet<String>,
    nodes_by_qn: HashMap<String, GraphNode>,
    nodes_by_id: HashMap<i64, GraphNode>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TracedFlow {
    pub name: String,
    pub entry_point: String,
    pub entry_point_id: i64,
    pub kind: String,
    pub path: Vec<i64>,
    pub members: Vec<i64>,
    pub depth: i64,
    pub node_count: i64,
    pub file_count: i64,
    pub files: Vec<String>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<String>,
    pub criticality: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct TraceOptions {
    pub max_depth: i64,
    pub include_tests: bool,
    pub max_nodes: i64,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_FLOW_MAX_DEPTH,
            include_tests: false,
            max_nodes: DEFAULT_FLOW_MAX_NODES,
        }
    }
}

pub fn trace_flows_json(
    store: &GraphStore,
    max_depth: i64,
    include_tests: bool,
    max_nodes: i64,
) -> Result<String> {
    let flows = trace_flows(
        store,
        TraceOptions {
            max_depth,
            include_tests,
            max_nodes,
        },
    )?;
    serde_json::to_string(&flows).map_err(GraphError::from)
}

pub fn incremental_trace_flows_json(
    store: &mut GraphStore,
    changed_files: &[String],
    max_depth: i64,
) -> Result<i64> {
    if changed_files.is_empty() {
        return Ok(0);
    }

    let changed_file_set: HashSet<String> = changed_files.iter().cloned().collect();
    let entry_point_ids: HashSet<i64> = store
        .delete_affected_flows(changed_files)?
        .into_iter()
        .collect();

    let entry_points = detect_entry_points(store, false)?;
    let relevant_eps: Vec<GraphNode> = entry_points
        .into_iter()
        .filter(|ep| {
            changed_file_set.contains(&ep.file_path) || entry_point_ids.contains(&ep.id)
        })
        .collect();

    let mut count = 0i64;
    if !relevant_eps.is_empty() {
        let adj = load_flow_adjacency(store)?;
        let mut new_flows = Vec::new();
        for ep in relevant_eps {
            if let Some(flow) = trace_single_flow(&adj, &ep, max_depth, DEFAULT_FLOW_MAX_NODES) {
                new_flows.push(flow);
            }
        }
        if !new_flows.is_empty() {
            let payload = serde_json::to_string(&new_flows).map_err(GraphError::from)?;
            count = store.insert_flows_json(&payload)?;
        }
    }

    refresh_flow_criticalities(store)?;
    Ok(count)
}

pub fn refresh_flow_criticalities(store: &mut GraphStore) -> Result<i64> {
    let adj = load_flow_adjacency(store)?;
    let flows_json = store.get_flows_json("criticality", 1_000_000)?;
    let flows: Vec<Value> = serde_json::from_str(&flows_json).map_err(GraphError::from)?;

    let mut updates = Vec::new();
    for flow in flows {
        let Some(flow_id) = flow.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let path = flow
            .get("path")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_i64)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let depth = flow.get("depth").and_then(Value::as_i64).unwrap_or(0);
        let previous = flow
            .get("criticality")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let recomputed = compute_criticality(&path, depth, &adj);
        if (recomputed - previous).abs() > 1e-9 {
            updates.push((flow_id, recomputed));
        }
    }

    if updates.is_empty() {
        return Ok(0);
    }

    let payload = serde_json::to_string(&updates).map_err(GraphError::from)?;
    store.update_flow_criticalities_json(&payload)
}

pub fn trace_flows(store: &GraphStore, options: TraceOptions) -> Result<Vec<TracedFlow>> {
    let entry_points = detect_entry_points(store, options.include_tests)?;
    if entry_points.is_empty() {
        return Ok(Vec::new());
    }

    let adj = load_flow_adjacency(store)?;
    let mut flows = Vec::new();
    for ep in entry_points {
        if let Some(flow) =
            trace_single_flow(&adj, &ep, options.max_depth, options.max_nodes)
        {
            flows.push(flow);
        }
    }

    flows.sort_by(|left, right| {
        right
            .criticality
            .partial_cmp(&left.criticality)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(flows)
}

fn detect_entry_points(store: &GraphStore, include_tests: bool) -> Result<Vec<GraphNode>> {
    let called_qnames = store.get_all_call_targets(false)?;
    let candidate_nodes = store.get_nodes_by_kind(
        &["Function".to_string(), "Test".to_string()],
        None,
    )?;

    let mut entry_points = Vec::new();
    let mut seen_qn = HashSet::new();

    for node in candidate_nodes {
        if !include_tests && (node.is_test || is_test_file(&node.file_path)) {
            continue;
        }

        let mut is_entry = false;
        if !called_qnames.contains(&node.qualified_name) {
            is_entry = true;
        }
        if has_framework_decorator(&node) {
            is_entry = true;
        }
        if matches_entry_name(&node) {
            is_entry = true;
        }

        if is_entry && seen_qn.insert(node.qualified_name.clone()) {
            entry_points.push(node);
        }
    }

    Ok(entry_points)
}

fn load_flow_adjacency(store: &GraphStore) -> Result<FlowAdjacency> {
    let nodes = store.get_all_nodes()?;
    let (calls_out, has_tested_by) = store.get_flow_edge_data()?;

    let mut nodes_by_qn = HashMap::with_capacity(nodes.len());
    let mut nodes_by_id = HashMap::with_capacity(nodes.len());
    for node in nodes {
        nodes_by_id.insert(node.id, node.clone());
        nodes_by_qn.insert(node.qualified_name.clone(), node);
    }

    Ok(FlowAdjacency {
        calls_out,
        has_tested_by,
        nodes_by_qn,
        nodes_by_id,
    })
}

fn trace_single_flow(
    adj: &FlowAdjacency,
    ep: &GraphNode,
    max_depth: i64,
    max_nodes: i64,
) -> Option<TracedFlow> {
    let mut path_ids = vec![ep.id];
    let mut path_qnames = vec![ep.qualified_name.clone()];
    let mut visited = HashSet::from([ep.qualified_name.clone()]);
    let mut queue = VecDeque::from([(ep.qualified_name.clone(), 0i64)]);

    let mut actual_depth = 0i64;
    let mut truncated = false;
    let mut truncation_reason = None;

    while let Some((current_qn, depth)) = queue.pop_front() {
        if depth > actual_depth {
            actual_depth = depth;
        }
        if depth >= max_depth {
            if adj.calls_out.get(&current_qn).is_some_and(|targets| !targets.is_empty()) {
                truncated = true;
                if truncation_reason.is_none() {
                    truncation_reason = Some("max_depth".to_string());
                }
            }
            continue;
        }

        for target_qn in adj.calls_out.get(&current_qn).into_iter().flatten() {
            if visited.contains(target_qn) {
                continue;
            }
            let Some(target_node) = adj.nodes_by_qn.get(target_qn) else {
                continue;
            };
            if path_ids.len() as i64 >= max_nodes {
                truncated = true;
                truncation_reason = Some("max_nodes".to_string());
                queue.clear();
                break;
            }
            visited.insert(target_qn.clone());
            path_ids.push(target_node.id);
            path_qnames.push(target_qn.clone());
            queue.push_back((target_qn.clone(), depth + 1));
        }
    }

    if path_ids.len() < 2 {
        return None;
    }

    let mut files = HashSet::new();
    for qn in &path_qnames {
        if let Some(node) = adj.nodes_by_qn.get(qn) {
            files.insert(node.file_path.clone());
        }
    }
    let files: Vec<String> = files.into_iter().collect();

    let mut flow = TracedFlow {
        name: sanitize_name(&ep.name),
        entry_point: ep.qualified_name.clone(),
        entry_point_id: ep.id,
        kind: FLOW_KIND_REACHABLE_SET.to_string(),
        path: path_ids.clone(),
        members: path_ids.clone(),
        depth: actual_depth,
        node_count: path_ids.len() as i64,
        file_count: files.len() as i64,
        files,
        truncated,
        truncation_reason,
        criticality: 0.0,
    };
    flow.criticality = compute_criticality(&flow.path, flow.depth, adj);
    Some(flow)
}

fn compute_criticality(path_ids: &[i64], depth: i64, adj: &FlowAdjacency) -> f64 {
    if path_ids.is_empty() {
        return 0.0;
    }

    let nodes: Vec<&GraphNode> = path_ids
        .iter()
        .filter_map(|id| adj.nodes_by_id.get(id))
        .collect();
    if nodes.is_empty() {
        return 0.0;
    }

    let file_count = nodes
        .iter()
        .map(|node| node.file_path.as_str())
        .collect::<HashSet<_>>()
        .len();
    let file_spread = if file_count > 1 {
        (((file_count - 1) as f64) / 4.0).min(1.0)
    } else {
        0.0
    };

    let mut external_count = 0usize;
    for node in &nodes {
        for target_qn in adj.calls_out.get(&node.qualified_name).into_iter().flatten() {
            if !adj.nodes_by_qn.contains_key(target_qn) {
                external_count += 1;
            }
        }
    }
    let external_score = ((external_count as f64) / 5.0).min(1.0);

    let mut security_hits = 0usize;
    for node in &nodes {
        let name_lower = node.name.to_lowercase();
        let qn_lower = node.qualified_name.to_lowercase();
        if SECURITY_KEYWORDS.iter().any(|keyword| {
            name_lower.contains(keyword) || qn_lower.contains(keyword)
        }) {
            security_hits += 1;
        }
    }
    let security_score = ((security_hits as f64) / (nodes.len() as f64)).min(1.0);

    let tested_count = nodes
        .iter()
        .filter(|node| adj.has_tested_by.contains(&node.qualified_name))
        .count();
    let coverage = (tested_count as f64) / (nodes.len() as f64);
    let test_gap = 1.0 - coverage;

    let depth_score = ((depth as f64) / 10.0).min(1.0);

    let criticality = file_spread * 0.30
        + external_score * 0.20
        + security_score * 0.25
        + test_gap * 0.15
        + depth_score * 0.10;

    let rounded = (criticality.clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0;
    rounded
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| *ch == '\t' || *ch == '\n' || (*ch as u32) >= 0x20)
        .take(256)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dagayn_graph::{EdgeInput, GraphStore, NodeInput};
    use serde_json::json;
    use std::path::PathBuf;

    fn temp_db(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "dagayn-postproc-{}-{}.db",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn sample_store() -> GraphStore {
        let path = temp_db("flows");
        let mut store = GraphStore::open(&path).expect("open graph store");
        let entry = NodeInput {
            kind: "Function".to_string(),
            name: "entry".to_string(),
            file_path: "app.py".to_string(),
            line_start: 1,
            line_end: 3,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({}),
        };
        let middle = NodeInput {
            kind: "Function".to_string(),
            name: "middle".to_string(),
            file_path: "app.py".to_string(),
            line_start: 4,
            line_end: 6,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({}),
        };
        let leaf = NodeInput {
            kind: "Function".to_string(),
            name: "leaf".to_string(),
            file_path: "app.py".to_string(),
            line_start: 7,
            line_end: 9,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({}),
        };
        let call1 = EdgeInput {
            kind: "CALLS".to_string(),
            source: "app.py::entry".to_string(),
            target: "app.py::middle".to_string(),
            file_path: "app.py".to_string(),
            line: 2,
            extra: json!({}),
        };
        let call2 = EdgeInput {
            kind: "CALLS".to_string(),
            source: "app.py::middle".to_string(),
            target: "app.py::leaf".to_string(),
            file_path: "app.py".to_string(),
            line: 5,
            extra: json!({}),
        };
        store
            .store_file_batch(&[(
                "app.py".to_string(),
                vec![entry, middle, leaf],
                vec![call1, call2],
                "hash".to_string(),
                0,
            )])
            .expect("store sample graph");
        store
    }

    #[test]
    fn trace_flows_follows_linear_call_chain() {
        let store = sample_store();
        let flows = trace_flows(&store, TraceOptions::default()).unwrap();
        let entry_flow = flows
            .iter()
            .find(|flow| flow.entry_point == "app.py::entry")
            .expect("entry flow");
        assert_eq!(entry_flow.node_count, 3);
        assert_eq!(entry_flow.kind, FLOW_KIND_REACHABLE_SET);
        assert!(!entry_flow.truncated);
    }

    #[test]
    fn trace_flows_json_round_trips() {
        let store = sample_store();
        let flows_json = trace_flows_json(&store, 15, false, 512).unwrap();
        let flows: Vec<TracedFlow> = serde_json::from_str(&flows_json).unwrap();
        assert!(!flows.is_empty());
    }
}
