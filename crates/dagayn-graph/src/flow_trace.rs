use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, OnceLock};

use regex::Regex;
use serde_json::{Value, json};

use crate::helpers::*;
use crate::*;

const DEFAULT_MAX_DEPTH: i64 = 15;
const DEFAULT_MAX_NODES: i64 = 512;

struct TraceGraph {
    calls_out: HashMap<String, Box<[String]>>,
    calls_in: HashMap<String, Box<[String]>>,
    nodes_by_qn: HashMap<String, Arc<GraphNode>>,
    nodes_by_id: HashMap<i64, Arc<GraphNode>>,
    has_tested_by: HashSet<String>,
}

fn freeze_adjacency(map: HashMap<String, Vec<String>>) -> HashMap<String, Box<[String]>> {
    map.into_iter()
        .map(|(key, values)| (key, values.into_boxed_slice()))
        .collect()
}

fn invert_calls(calls_out: &HashMap<String, Box<[String]>>) -> HashMap<String, Box<[String]>> {
    let mut calls_in: HashMap<String, Vec<String>> = HashMap::new();
    for (source, targets) in calls_out {
        for target in targets.iter() {
            calls_in
                .entry(target.clone())
                .or_default()
                .push(source.clone());
        }
    }
    freeze_adjacency(calls_in)
}

fn index_nodes(
    nodes: impl IntoIterator<Item = GraphNode>,
) -> (
    HashMap<String, Arc<GraphNode>>,
    HashMap<i64, Arc<GraphNode>>,
) {
    let mut nodes_by_qn = HashMap::new();
    let mut nodes_by_id = HashMap::new();
    for node in nodes {
        let node = Arc::new(node);
        nodes_by_qn.insert(node.qualified_name.clone(), Arc::clone(&node));
        nodes_by_id.insert(node.id, node);
    }
    (nodes_by_qn, nodes_by_id)
}

struct ExistingEntry {
    flow_id: i64,
    qualified_name: String,
}

impl GraphStore {
    pub fn detect_entry_points_json(&self, include_tests: bool) -> Result<String> {
        let entries = self.detect_entry_points(include_tests)?;
        let payload: Vec<Value> = entries
            .into_iter()
            .map(|node| {
                json!({
                    "id": node.id,
                    "name": node.name,
                    "qualified_name": node.qualified_name,
                    "file_path": node.file_path,
                    "kind": node.kind,
                    "is_test": node.is_test,
                })
            })
            .collect();
        serde_json::to_string(&payload).map_err(Into::into)
    }

    pub fn rebuild_flows_json(&mut self, max_depth: i64, include_tests: bool) -> Result<String> {
        let flows = self.trace_reachable_sets(max_depth, include_tests)?;
        let count = self.store_flows(&flows)?;
        Ok(json!({ "count": count }).to_string())
    }

    pub fn incremental_trace_flows_json(
        &mut self,
        changed_files: &[String],
        max_depth: i64,
    ) -> Result<String> {
        let count = self.incremental_trace_flows(changed_files, max_depth)?;
        Ok(json!({ "count": count }).to_string())
    }

    pub fn incremental_trace_flows(
        &mut self,
        changed_files: &[String],
        max_depth: i64,
    ) -> Result<i64> {
        if changed_files.is_empty() {
            return Ok(0);
        }
        let graph = match self.load_trace_graph_for_files(changed_files, max_depth)? {
            Some(graph) => graph,
            None => self.load_trace_graph()?,
        };
        let changed_qns = self.changed_qualified_names(changed_files)?;
        let existing = self.existing_flow_entries()?;
        let existing_qns: HashSet<String> = existing
            .iter()
            .map(|entry| entry.qualified_name.clone())
            .collect();

        let file_keys = self.expand_file_keys(changed_files)?;
        let mut relevant_qns = reverse_call_entries(&graph, &changed_qns, &existing_qns);
        for node in detect_entries_in_files(&graph, &file_keys, false) {
            relevant_qns.insert(node.qualified_name.clone());
        }

        let mut flow_ids: HashSet<i64> = self
            .get_affected_flow_ids(changed_files)?
            .into_iter()
            .collect();
        for entry in &existing {
            if relevant_qns.contains(&entry.qualified_name) {
                flow_ids.insert(entry.flow_id);
            }
        }
        if !flow_ids.is_empty() {
            self.delete_flows_by_ids(&flow_ids.into_iter().collect::<Vec<_>>())?;
        }

        let mut new_flows = Vec::new();
        for qn in &relevant_qns {
            let Some(node) = graph.nodes_by_qn.get(qn) else {
                continue;
            };
            if let Some(flow) = trace_single_flow(&graph, node, max_depth, DEFAULT_MAX_NODES) {
                new_flows.push(flow);
            }
        }
        let count = if new_flows.is_empty() {
            0
        } else {
            self.insert_flows(&new_flows)?
        };

        let tested_flow_ids = self.flow_ids_for_tested_by_files(changed_files)?;
        let mut refresh_ids: HashSet<i64> = tested_flow_ids.into_iter().collect();
        if count > 0 {
            for entry in self.existing_flow_entries()? {
                if relevant_qns.contains(&entry.qualified_name) {
                    refresh_ids.insert(entry.flow_id);
                }
            }
        }
        self.refresh_flow_criticalities_on(&graph, Some(&refresh_ids))?;
        Ok(count)
    }

    pub fn detect_entry_points(&self, include_tests: bool) -> Result<Vec<GraphNode>> {
        let graph = self.load_trace_graph()?;
        Ok(detect_entries(&graph, include_tests))
    }

    fn load_trace_graph(&self) -> Result<TraceGraph> {
        let (calls_out, has_tested_by) = self.get_flow_edge_data()?;
        let calls_out = freeze_adjacency(calls_out);
        let calls_in = invert_calls(&calls_out);
        let (nodes_by_qn, nodes_by_id) = index_nodes(self.get_all_nodes_filtered(false)?);
        Ok(TraceGraph {
            calls_out,
            calls_in,
            nodes_by_qn,
            nodes_by_id,
            has_tested_by,
        })
    }

    fn load_trace_graph_for_files(
        &self,
        changed_files: &[String],
        max_depth: i64,
    ) -> Result<Option<TraceGraph>> {
        let changed_qns = self.changed_qualified_names(changed_files)?;
        let total = self.count_non_file_nodes()?;
        let cap = if total <= 0 {
            usize::MAX
        } else {
            (total as usize / 2).max(1)
        };

        let Some(mut visited) = self.expand_flow_hops(&changed_qns, true, None, cap)? else {
            return Ok(None);
        };

        let existing = self.existing_flow_entries()?;
        let existing_qns: HashSet<String> = existing
            .iter()
            .map(|entry| entry.qualified_name.clone())
            .collect();
        let mut relevant: HashSet<String> = visited.intersection(&existing_qns).cloned().collect();

        let called = self.called_targets_among(&changed_qns)?;
        let changed_list: Vec<String> = changed_qns.iter().cloned().collect();
        let changed_nodes = self.get_nodes_by_qualified_names(&changed_list)?;
        for node in changed_nodes.values() {
            if !is_entry_kind(node) {
                continue;
            }
            if node.is_test || is_test_file(&node.file_path) {
                continue;
            }
            if is_entry_point(node, called.contains(&node.qualified_name)) {
                relevant.insert(node.qualified_name.clone());
            }
        }

        let mut seed_flow_ids = self.get_affected_flow_ids(changed_files)?;
        seed_flow_ids.extend(self.flow_ids_for_tested_by_files(changed_files)?);
        visited.extend(self.qualified_names_for_flow_ids(&seed_flow_ids)?);
        if visited.len() > cap {
            return Ok(None);
        }

        let depth_limit = if max_depth <= 0 {
            DEFAULT_MAX_DEPTH
        } else {
            max_depth
        };
        let Some(reached) = self.expand_flow_hops(&relevant, false, Some(depth_limit), cap)? else {
            return Ok(None);
        };
        visited.extend(reached);
        visited.extend(changed_qns);
        if visited.len() > cap {
            return Ok(None);
        }
        Ok(Some(self.load_trace_graph_from_qns(&visited)?))
    }

    fn expand_flow_hops(
        &self,
        seeds: &HashSet<String>,
        incoming: bool,
        max_hops: Option<i64>,
        cap: usize,
    ) -> Result<Option<HashSet<String>>> {
        let mut visited = seeds.clone();
        let mut frontier: Vec<String> = seeds.iter().cloned().collect();
        let mut hops = 0_i64;
        while !frontier.is_empty() {
            if visited.len() > cap {
                return Ok(None);
            }
            if let Some(limit) = max_hops
                && hops >= limit
            {
                break;
            }
            let next = self.flow_neighbor_qualified_names(&frontier, incoming)?;
            frontier = next
                .into_iter()
                .filter(|qn| visited.insert(qn.clone()))
                .collect();
            hops += 1;
        }
        if visited.len() > cap {
            return Ok(None);
        }
        Ok(Some(visited))
    }

    fn load_trace_graph_from_qns(&self, qns: &HashSet<String>) -> Result<TraceGraph> {
        let names: Vec<String> = qns.iter().cloned().collect();
        let fetched = self.get_nodes_by_qualified_names(&names)?;
        let (nodes_by_qn, nodes_by_id) = index_nodes(fetched.into_values());
        let loaded_qns: HashSet<String> = nodes_by_qn.keys().cloned().collect();
        let (calls_out, has_tested_by) = self.get_flow_edge_data_for_qns(&loaded_qns)?;
        let calls_out = freeze_adjacency(calls_out);
        let calls_in = invert_calls(&calls_out);
        Ok(TraceGraph {
            calls_out,
            calls_in,
            nodes_by_qn,
            nodes_by_id,
            has_tested_by,
        })
    }

    fn qualified_names_for_flow_ids(&self, flow_ids: &[i64]) -> Result<HashSet<String>> {
        let mut out = HashSet::new();
        if flow_ids.is_empty() {
            return Ok(out);
        }
        for chunk in flow_ids.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT n.qualified_name FROM flow_memberships fm \
                 JOIN nodes n ON n.id = fm.node_id \
                 WHERE fm.flow_id IN ({placeholders}) \
                 UNION \
                 SELECT n.qualified_name FROM flows f \
                 JOIN nodes n ON n.id = f.entry_point_id \
                 WHERE f.id IN ({placeholders})"
            );
            let mut params = Vec::with_capacity(chunk.len() * 2);
            params.extend_from_slice(chunk);
            params.extend_from_slice(chunk);
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                out.insert(row?);
            }
        }
        Ok(out)
    }

    fn changed_qualified_names(&self, changed_files: &[String]) -> Result<HashSet<String>> {
        let file_keys = self.expand_file_keys(changed_files)?;
        if file_keys.is_empty() {
            return Ok(HashSet::new());
        }
        let mut out = HashSet::new();
        for chunk in file_keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("SELECT qualified_name FROM nodes WHERE file_path IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                out.insert(row?);
            }
        }
        Ok(out)
    }

    fn existing_flow_entries(&self) -> Result<Vec<ExistingEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, n.qualified_name \
             FROM flows f JOIN nodes n ON n.id = f.entry_point_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ExistingEntry {
                flow_id: row.get(0)?,
                qualified_name: row.get(1)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn delete_flows_by_ids(&mut self, flow_ids: &[i64]) -> Result<()> {
        if flow_ids.is_empty() {
            return Ok(());
        }
        let tx = write_tx(&mut self.conn)?;
        for chunk in flow_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            tx.execute(
                &format!("DELETE FROM flow_snapshots WHERE flow_id IN ({placeholders})"),
                rusqlite::params_from_iter(chunk),
            )?;
            tx.execute(
                &format!("DELETE FROM flow_memberships WHERE flow_id IN ({placeholders})"),
                rusqlite::params_from_iter(chunk),
            )?;
            tx.execute(
                &format!("DELETE FROM flows WHERE id IN ({placeholders})"),
                rusqlite::params_from_iter(chunk),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn insert_flows(&mut self, flows: &[FlowInput]) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        delete_flows_for_entry_point_ids(&tx, flows)?;
        store_flows_tx(&tx, flows)?;
        tx.commit()?;
        Ok(flows.len() as i64)
    }

    fn flow_ids_for_tested_by_files(&self, changed_files: &[String]) -> Result<Vec<i64>> {
        let file_keys = self.expand_file_keys(changed_files)?;
        if file_keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for chunk in file_keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT fm.flow_id \
                 FROM edges e \
                 JOIN nodes n ON n.qualified_name = e.source_qualified \
                 JOIN flow_memberships fm ON fm.node_id = n.id \
                 WHERE e.kind = 'TESTED_BY' AND e.file_path IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, i64>(0)
            })?;
            for row in rows {
                let flow_id = row?;
                if seen.insert(flow_id) {
                    out.push(flow_id);
                }
            }
        }
        Ok(out)
    }

    fn refresh_flow_criticalities_on(
        &mut self,
        graph: &TraceGraph,
        flow_ids: Option<&HashSet<i64>>,
    ) -> Result<i64> {
        if matches!(flow_ids, Some(ids) if ids.is_empty()) {
            return Ok(0);
        }
        let rows = self.load_flow_criticality_rows(flow_ids)?;
        let mut updates = Vec::new();
        for (flow_id, depth, path_json, previous) in rows {
            let path: Vec<i64> = serde_json::from_str(&path_json).unwrap_or_default();
            let recomputed = compute_criticality(graph, &path, depth);
            if (recomputed - previous).abs() > 1e-9 {
                updates.push((flow_id, recomputed));
            }
        }
        if updates.is_empty() {
            return Ok(0);
        }
        let payload = serde_json::to_string(&updates)?;
        self.update_flow_criticalities_json(&payload)
    }

    fn load_flow_criticality_rows(
        &self,
        flow_ids: Option<&HashSet<i64>>,
    ) -> Result<Vec<(i64, i64, String, f64)>> {
        let mut out = Vec::new();
        match flow_ids {
            None => {
                let mut stmt = self
                    .conn
                    .prepare("SELECT id, depth, path_json, criticality FROM flows")?;
                let mapped = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, f64>(3)?,
                    ))
                })?;
                for row in mapped {
                    out.push(row?);
                }
            }
            Some(ids) => {
                let ids: Vec<i64> = ids.iter().copied().collect();
                for chunk in ids.chunks(450) {
                    if chunk.is_empty() {
                        continue;
                    }
                    let placeholders = std::iter::repeat_n("?", chunk.len())
                        .collect::<Vec<_>>()
                        .join(",");
                    let sql = format!(
                        "SELECT id, depth, path_json, criticality FROM flows \
                         WHERE id IN ({placeholders})"
                    );
                    let mut stmt = self.conn.prepare(&sql)?;
                    let mapped = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, f64>(3)?,
                        ))
                    })?;
                    for row in mapped {
                        out.push(row?);
                    }
                }
            }
        }
        Ok(out)
    }

    fn trace_reachable_sets(&self, max_depth: i64, include_tests: bool) -> Result<Vec<FlowInput>> {
        let graph = self.load_trace_graph()?;
        let mut flows = Vec::new();
        for node in detect_entries(&graph, include_tests) {
            if let Some(flow) = trace_single_flow(&graph, &node, max_depth, DEFAULT_MAX_NODES) {
                flows.push(flow);
            }
        }
        flows.sort_by(|a, b| b.criticality.total_cmp(&a.criticality));
        Ok(flows)
    }
}

fn reverse_call_entries(
    graph: &TraceGraph,
    changed_qns: &HashSet<String>,
    existing_entry_qns: &HashSet<String>,
) -> HashSet<String> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    for qn in changed_qns {
        visited.insert(qn.clone());
        queue.push_back(qn.clone());
    }
    let mut affected = HashSet::new();
    while let Some(qn) = queue.pop_front() {
        if existing_entry_qns.contains(&qn) {
            affected.insert(qn.clone());
        }
        if let Some(callers) = graph.calls_in.get(&qn) {
            for caller in callers {
                if visited.insert(caller.clone()) {
                    queue.push_back(caller.clone());
                }
            }
        }
    }
    affected
}

fn detect_entries(graph: &TraceGraph, include_tests: bool) -> Vec<GraphNode> {
    let called: HashSet<&str> = graph
        .calls_out
        .values()
        .flatten()
        .map(String::as_str)
        .collect();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for node in graph.nodes_by_qn.values() {
        if !is_entry_kind(node) {
            continue;
        }
        if !include_tests && (node.is_test || is_test_file(&node.file_path)) {
            continue;
        }
        if is_entry_point(node, called.contains(node.qualified_name.as_str()))
            && seen.insert(node.qualified_name.clone())
        {
            entries.push((**node).clone());
        }
    }
    entries
}

fn detect_entries_in_files(
    graph: &TraceGraph,
    changed_files: &[String],
    include_tests: bool,
) -> Vec<GraphNode> {
    let files: HashSet<&str> = changed_files.iter().map(String::as_str).collect();
    let called: HashSet<&str> = graph
        .calls_out
        .values()
        .flatten()
        .map(String::as_str)
        .collect();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for node in graph.nodes_by_qn.values() {
        if !files.contains(node.file_path.as_str()) {
            continue;
        }
        if !is_entry_kind(node) {
            continue;
        }
        if !include_tests && (node.is_test || is_test_file(&node.file_path)) {
            continue;
        }
        if is_entry_point(node, called.contains(node.qualified_name.as_str()))
            && seen.insert(node.qualified_name.clone())
        {
            entries.push((**node).clone());
        }
    }
    entries
}

fn is_entry_kind(node: &GraphNode) -> bool {
    node.kind == "Function" || node.kind == "Test"
}

fn is_test_file(file_path: &str) -> bool {
    test_file_re().is_match(file_path)
}

fn is_entry_point(node: &GraphNode, is_called: bool) -> bool {
    if !is_called {
        return true;
    }
    has_framework_decorator(node) || matches_entry_name(&node.name)
}

fn has_framework_decorator(node: &GraphNode) -> bool {
    let Some(decorators) = node.extra.get("decorators") else {
        return false;
    };
    let values = match decorators {
        Value::String(value) => vec![value.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let patterns = decorator_res();
    values
        .iter()
        .any(|value| patterns.iter().any(|pattern| pattern.is_match(value)))
}

fn matches_entry_name(name: &str) -> bool {
    entry_name_res()
        .iter()
        .any(|pattern| pattern.is_match(name))
}

fn trace_single_flow(
    graph: &TraceGraph,
    entry: &GraphNode,
    max_depth: i64,
    max_nodes: i64,
) -> Option<FlowInput> {
    let mut path_ids = vec![entry.id];
    let mut path_qns = vec![entry.qualified_name.clone()];
    let mut visited = HashSet::from([entry.qualified_name.clone()]);
    let mut queue = VecDeque::from([(entry.qualified_name.clone(), 0_i64)]);
    let mut actual_depth = 0_i64;
    let mut truncated = false;
    let mut truncation_reason = None;
    let max_depth = if max_depth <= 0 {
        DEFAULT_MAX_DEPTH
    } else {
        max_depth
    };

    while let Some((current_qn, depth)) = queue.pop_front() {
        if depth > actual_depth {
            actual_depth = depth;
        }
        if depth >= max_depth {
            if graph
                .calls_out
                .get(&current_qn)
                .is_some_and(|targets| !targets.is_empty())
            {
                truncated = true;
                if truncation_reason.is_none() {
                    truncation_reason = Some("max_depth".to_string());
                }
            }
            continue;
        }
        for target_qn in graph.calls_out.get(&current_qn).into_iter().flatten() {
            if !visited.insert(target_qn.clone()) {
                continue;
            }
            let Some(target) = graph.nodes_by_qn.get(target_qn) else {
                continue;
            };
            if path_ids.len() as i64 >= max_nodes {
                truncated = true;
                truncation_reason = Some("max_nodes".to_string());
                queue.clear();
                break;
            }
            path_ids.push(target.id);
            path_qns.push(target_qn.clone());
            queue.push_back((target_qn.clone(), depth + 1));
        }
    }
    if path_ids.len() < 2 {
        return None;
    }
    let files: HashSet<&str> = path_qns
        .iter()
        .filter_map(|qn| {
            graph
                .nodes_by_qn
                .get(qn)
                .map(|node| node.file_path.as_str())
        })
        .collect();
    let mut flow = FlowInput {
        name: sanitize_name(&entry.name),
        entry_point_id: entry.id,
        depth: actual_depth,
        node_count: path_ids.len() as i64,
        file_count: files.len() as i64,
        criticality: 0.0,
        path: path_ids.clone().into(),
        kind: "reachable_set".to_string(),
        truncated,
        truncation_reason,
    };
    flow.criticality = compute_criticality(graph, &path_ids, actual_depth);
    Some(flow)
}

fn compute_criticality(graph: &TraceGraph, path_ids: &[i64], depth: i64) -> f64 {
    let nodes: Vec<&GraphNode> = path_ids
        .iter()
        .filter_map(|id| graph.nodes_by_id.get(id).map(Arc::as_ref))
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
        ((file_count as f64 - 1.0) / 4.0).min(1.0)
    } else {
        0.0
    };
    let mut external_count = 0_i64;
    for node in &nodes {
        for target in graph
            .calls_out
            .get(&node.qualified_name)
            .into_iter()
            .flatten()
        {
            if !graph.nodes_by_qn.contains_key(target) {
                external_count += 1;
            }
        }
    }
    let external_score = (external_count as f64 / 5.0).min(1.0);
    let security_hits = nodes
        .iter()
        .filter(|node| {
            let name = node.name.to_ascii_lowercase();
            let qn = node.qualified_name.to_ascii_lowercase();
            SECURITY_KEYWORDS
                .iter()
                .any(|keyword| name.contains(keyword) || qn.contains(keyword))
        })
        .count();
    let security_score = (security_hits as f64 / nodes.len() as f64).min(1.0);
    let tested_count = nodes
        .iter()
        .filter(|node| graph.has_tested_by.contains(&node.qualified_name))
        .count();
    let test_gap = 1.0 - (tested_count as f64 / nodes.len() as f64);
    let depth_score = (depth as f64 / 10.0).min(1.0);
    let criticality = file_spread * 0.30
        + external_score * 0.20
        + security_score * 0.25
        + test_gap * 0.15
        + depth_score * 0.10;
    (criticality.clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0
}

fn test_file_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"([\\/]__tests__[\\/]|\.spec\.[jt]sx?$|\.test\.[jt]sx?$|[\\/]test_[^/\\]*\.py$)",
        )
        .expect("test file regex")
    })
}

fn decorator_res() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        [
            r"(?i)app\.(get|post|put|delete|patch|route|websocket|on_event)",
            r"(?i)router\.(get|post|put|delete|patch|route)",
            r"(?i)blueprint\.(route|before_request|after_request)",
            r"(?i)(before|after)_(request|response)",
            r"(?i)click\.(command|group)",
            r"(?i)\w+\.(command|group)\b",
            r"(?i)(field|model)_(serializer|validator)",
            r"(?i)(celery\.)?(task|shared_task|periodic_task)",
            r"(?i)receiver",
            r"(?i)api_view",
            r"(?i)\baction\b",
            r"pytest\.(fixture|mark)",
            r"(?i)(override_settings|modify_settings)",
            r"(?i)(event\.)?listens_for",
            r"(?i)(Get|Post|Put|Delete|Patch|RequestMapping)Mapping",
            r"(?i)(Scheduled|EventListener|Bean|Configuration)",
            r"(?i)(Component|Injectable|Controller|Module|Guard|Pipe)",
            r"(?i)(Subscribe|Mutation|Query|Resolver)",
            r"(app|router)\.(get|post|put|delete|patch|use|all)\b",
            r"(?i)@(Override|OnLifecycleEvent|Composable)",
            r"(?i)(HiltViewModel|AndroidEntryPoint|Inject)",
            r"(?i)\w+\.(tool|tool_plain|system_prompt|result_validator)\b",
            r"^tool\b",
            r"(?i)\w+\.(middleware|exception_handler|on_exception)\b",
            r"(?i)\w+\.route\b",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("decorator regex"))
        .collect()
    })
}

fn entry_name_res() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        [
            r"^main$",
            r"^__main__$",
            r"^test_",
            r"^Test[A-Z]",
            r"^on_",
            r"^handle_",
            r"^handler$",
            r"^handle$",
            r"^lambda_handler$",
            r"^upgrade$",
            r"^downgrade$",
            r"^lifespan$",
            r"^get_db$",
            r"^on(Create|Start|Resume|Pause|Stop|Destroy|Bind|Receive)",
            r"^do(Get|Post|Put|Delete)$",
            r"^do_(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)$",
            r"^log_message$",
            r"^(middleware|errorHandler)$",
            r"^ng(OnInit|OnChanges|OnDestroy|DoCheck|AfterContentInit|AfterContentChecked|AfterViewInit|AfterViewChecked)$",
            r"^(transform|writeValue|registerOnChange|registerOnTouched|setDisabledState)$",
            r"^(canActivate|canDeactivate|canActivateChild|canLoad|canMatch|resolve)$",
            r"^(componentDidMount|componentDidUpdate|componentWillUnmount|shouldComponentUpdate|render)$",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("entry name regex"))
        .collect()
    })
}
