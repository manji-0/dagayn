use crate::*;
use serde::Serialize;

/// Begin a write transaction that takes the write lock up front.
///
/// rusqlite's `transaction()` is `BEGIN DEFERRED`, which acquires a *read*
/// snapshot first. Any of our write paths that reads before writing then has to
/// upgrade, and if another connection committed in between, the upgrade fails
/// **immediately** with `SQLITE_BUSY` — `busy_timeout` does not apply to a
/// read-to-write upgrade, so the 5 s we configure was never spent. `IMMEDIATE`
/// takes the write lock at `BEGIN`, where `busy_timeout` does apply, which is
/// also what the Python backend has always done.
pub(crate) fn write_tx(conn: &mut Connection) -> Result<Transaction<'_>> {
    Ok(conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?)
}

pub(crate) fn now_seconds() -> Result<f64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| GraphError::Clock)?;
    Ok(duration.as_secs_f64())
}

pub(crate) fn make_qualified_parts(
    kind: &str,
    name: &str,
    file_path: &str,
    parent_name: Option<&str>,
) -> String {
    if kind == "File" {
        file_path.to_string()
    } else if let Some(parent) = parent_name {
        format!("{file_path}::{parent}.{name}")
    } else {
        format!("{file_path}::{name}")
    }
}

pub(crate) fn edge_target_name(target_qualified: &str) -> String {
    target_qualified
        .rsplit("::")
        .next()
        .unwrap_or(target_qualified)
        .to_string()
}

pub(crate) fn remove_file_data_tx(tx: &Transaction<'_>, file_path: &str) -> Result<()> {
    crate::fts_sync::delete_fts_for_file_paths_tx(tx, &[file_path.to_string()])?;
    tx.execute(
        "DELETE FROM risk_index WHERE node_id IN (SELECT id FROM nodes WHERE file_path = ?)",
        [file_path],
    )?;
    tx.execute("DELETE FROM edges WHERE file_path = ?", [file_path])?;
    tx.execute("DELETE FROM nodes WHERE file_path = ?", [file_path])?;
    crate::fts_sync::set_fts_watermark_tx(tx, None)?;
    Ok(())
}

pub(crate) fn remove_files_data_tx(tx: &Transaction<'_>, file_paths: &[String]) -> Result<()> {
    tx.execute("DELETE FROM hub_scores", [])?;
    tx.execute("DELETE FROM bridge_scores", [])?;
    crate::fts_sync::delete_fts_for_file_paths_tx(tx, file_paths)?;
    for chunk in file_paths.chunks(450) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let risk_sql = format!(
            "DELETE FROM risk_index \
             WHERE node_id IN (SELECT id FROM nodes WHERE file_path IN ({placeholders}))"
        );
        tx.execute(&risk_sql, rusqlite::params_from_iter(chunk))?;
        let edges_sql = format!("DELETE FROM edges WHERE file_path IN ({placeholders})");
        tx.execute(&edges_sql, rusqlite::params_from_iter(chunk))?;
        let nodes_sql = format!("DELETE FROM nodes WHERE file_path IN ({placeholders})");
        tx.execute(&nodes_sql, rusqlite::params_from_iter(chunk))?;
    }
    crate::fts_sync::set_fts_watermark_tx(tx, None)?;
    Ok(())
}

pub(crate) fn betweenness_centrality(
    graph_nodes: &HashSet<String>,
    adjacency: &HashMap<String, Vec<String>>,
) -> HashMap<String, f64> {
    let mut nodes = graph_nodes.iter().cloned().collect::<Vec<_>>();
    nodes.sort();
    let node_count = nodes.len();
    if node_count == 0 {
        return HashMap::new();
    }
    let sources = if node_count > 5000 {
        deterministic_centrality_sample(&nodes, 500)
    } else {
        nodes.clone()
    };
    let scale = if node_count > 5000 {
        node_count as f64 / sources.len() as f64
    } else {
        1.0
    };

    let mut centrality = nodes
        .iter()
        .map(|node| (node.clone(), 0.0_f64))
        .collect::<HashMap<_, _>>();

    for source in sources {
        let mut stack = Vec::<String>::new();
        let mut predecessors = nodes
            .iter()
            .map(|node| (node.clone(), Vec::<String>::new()))
            .collect::<HashMap<_, _>>();
        let mut sigma = nodes
            .iter()
            .map(|node| (node.clone(), 0.0_f64))
            .collect::<HashMap<_, _>>();
        let mut distance = nodes
            .iter()
            .map(|node| (node.clone(), -1_i64))
            .collect::<HashMap<_, _>>();
        sigma.insert(source.clone(), 1.0);
        distance.insert(source.clone(), 0);

        let mut queue = VecDeque::from([source.clone()]);
        while let Some(vertex) = queue.pop_front() {
            stack.push(vertex.clone());
            let vertex_distance = *distance.get(&vertex).unwrap_or(&-1);
            let vertex_sigma = *sigma.get(&vertex).unwrap_or(&0.0);
            for successor in adjacency.get(&vertex).into_iter().flatten() {
                if !distance.contains_key(successor) {
                    continue;
                }
                if *distance.get(successor).unwrap_or(&-1) < 0 {
                    queue.push_back(successor.clone());
                    distance.insert(successor.clone(), vertex_distance + 1);
                }
                if *distance.get(successor).unwrap_or(&-1) == vertex_distance + 1 {
                    *sigma.entry(successor.clone()).or_insert(0.0) += vertex_sigma;
                    predecessors
                        .entry(successor.clone())
                        .or_default()
                        .push(vertex.clone());
                }
            }
        }

        let mut dependency = nodes
            .iter()
            .map(|node| (node.clone(), 0.0_f64))
            .collect::<HashMap<_, _>>();
        while let Some(w) = stack.pop() {
            let sigma_w = *sigma.get(&w).unwrap_or(&0.0);
            if sigma_w != 0.0 {
                for v in predecessors.get(&w).into_iter().flatten() {
                    let sigma_v = *sigma.get(v).unwrap_or(&0.0);
                    let delta_w = *dependency.get(&w).unwrap_or(&0.0);
                    *dependency.entry(v.clone()).or_insert(0.0) +=
                        (sigma_v / sigma_w) * (1.0 + delta_w);
                }
            }
            if w != source {
                *centrality.entry(w.clone()).or_insert(0.0) +=
                    *dependency.get(&w).unwrap_or(&0.0) * scale;
            }
        }
    }

    if node_count > 2 {
        let norm = 1.0 / ((node_count as f64 - 1.0) * (node_count as f64 - 2.0));
        for value in centrality.values_mut() {
            *value *= norm;
        }
    }

    centrality
}

pub(crate) fn deterministic_centrality_sample(nodes: &[String], sample_size: usize) -> Vec<String> {
    let mut ranked = nodes
        .iter()
        .map(|node| (stable_fnv1a64(node.as_bytes()), node.clone()))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    ranked
        .into_iter()
        .take(sample_size.min(nodes.len()))
        .map(|(_, node)| node)
        .collect()
}

pub(crate) fn stable_fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub(crate) struct PersistedBridgeRow {
    pub(crate) name: String,
    pub(crate) qualified_name: String,
}

pub(crate) struct PersistedHubRow {
    pub(crate) name: String,
    pub(crate) qualified_name: String,
    pub(crate) total_degree: i64,
}

pub(crate) struct SurprisingQuestionInput {
    pub(crate) source_name: String,
    pub(crate) source_qualified: String,
    pub(crate) target_name: String,
    pub(crate) source_community: i64,
    pub(crate) target_community: i64,
    pub(crate) score: i64,
}

pub(crate) struct QuestionCommunity {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) size: i64,
}

pub(crate) struct QuestionHotspot {
    pub(crate) name: String,
    pub(crate) qualified_name: String,
    pub(crate) degree: i64,
}

pub(crate) struct QuestionGaps {
    pub(crate) thin_communities: Vec<QuestionCommunity>,
    pub(crate) untested_hotspots: Vec<QuestionHotspot>,
}

pub(crate) struct QuestionNode {
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) qualified_name: String,
    pub(crate) file_path: String,
    pub(crate) language: String,
    pub(crate) is_test: bool,
}

pub(crate) struct QuestionEdge {
    pub(crate) kind: String,
    pub(crate) source_qualified: String,
    pub(crate) target_qualified: String,
}

pub(crate) fn nearest_rank_percentile(values: &[i64], percentile: f64) -> i64 {
    if values.is_empty() {
        return 0;
    }
    let rank = (percentile * values.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(values.len() - 1);
    values[index]
}

pub(crate) fn is_analysis_excluded_from_test_gap(node: &QuestionNode) -> bool {
    if node.is_test || node.kind == "Test" || node.language == "markdown" {
        return true;
    }
    let normalized = node.file_path.replace('\\', "/");
    let name = normalized
        .rsplit('/')
        .next()
        .unwrap_or(normalized.as_str())
        .to_lowercase();
    let parts = normalized
        .split('/')
        .map(|part| part.to_lowercase())
        .collect::<HashSet<_>>();
    parts.contains("tests")
        || parts.contains("test")
        || parts.contains("__tests__")
        || name.starts_with("test_")
        || matches!(name.as_str(), "test.rs" | "tests.rs")
        || name.ends_with("_test.py")
        || name.ends_with("_tests.py")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || name.contains(".test.")
        || name.contains(".spec.")
}

pub(crate) fn extra_json(value: &Value) -> Result<String> {
    if value.is_null() || value.as_object().is_some_and(|object| object.is_empty()) {
        Ok("{}".to_string())
    } else {
        Ok(serde_json::to_string(value)?)
    }
}

pub(crate) fn delete_flows_for_entry_point_ids(
    tx: &Transaction<'_>,
    flows: &[FlowInput],
) -> Result<()> {
    let mut entry_point_ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for flow in flows {
        if seen.insert(flow.entry_point_id) {
            entry_point_ids.push(flow.entry_point_id);
        }
    }
    if entry_point_ids.is_empty() {
        return Ok(());
    }

    let mut qualified_names = Vec::new();
    for chunk in entry_point_ids.chunks(450) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT qualified_name FROM nodes WHERE id IN ({placeholders})");
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            qualified_names.push(row?);
        }
    }

    if qualified_names.is_empty() {
        return Ok(());
    }

    let mut flow_ids: Vec<i64> = Vec::new();
    for chunk in qualified_names.chunks(450) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT f.id FROM flows f \
             JOIN nodes n ON n.id = f.entry_point_id \
             WHERE n.qualified_name IN ({placeholders})"
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
            row.get::<_, i64>(0)
        })?;
        for row in rows {
            flow_ids.push(row?);
        }
    }

    if flow_ids.is_empty() {
        return Ok(());
    }

    let mut delete_snapshot = tx.prepare("DELETE FROM flow_snapshots WHERE flow_id = ?")?;
    let mut delete_membership = tx.prepare("DELETE FROM flow_memberships WHERE flow_id = ?")?;
    let mut delete_flow = tx.prepare("DELETE FROM flows WHERE id = ?")?;
    for flow_id in flow_ids {
        delete_snapshot.execute([flow_id])?;
        delete_membership.execute([flow_id])?;
        delete_flow.execute([flow_id])?;
    }
    Ok(())
}

pub(crate) fn store_flows_tx(tx: &Transaction<'_>, flows: &[FlowInput]) -> Result<()> {
    let mut insert_flow = tx.prepare(
        "INSERT INTO flows \
         (name, entry_point_id, depth, node_count, file_count, criticality, path_json) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )?;
    let mut insert_membership = tx.prepare(
        "INSERT OR IGNORE INTO flow_memberships (flow_id, node_id, position) \
         VALUES (?, ?, ?)",
    )?;
    for flow in flows {
        insert_flow.execute(params![
            flow.name,
            flow.entry_point_id,
            flow.depth,
            flow.node_count,
            flow.file_count,
            flow.criticality,
            serde_json::to_string(&flow.path)?,
        ])?;
        let flow_id = tx.last_insert_rowid();
        for (position, node_id) in flow.path.iter().enumerate() {
            insert_membership.execute(params![flow_id, node_id, position as i64])?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct FlowJson<'a> {
    id: i64,
    name: String,
    entry_point_id: i64,
    depth: i64,
    node_count: i64,
    file_count: i64,
    criticality: f64,
    path: &'a [i64],
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct FlowStepJson {
    node_id: i64,
    name: String,
    kind: String,
    file: String,
    line_start: i64,
    line_end: i64,
    qualified_name: String,
}

#[derive(Serialize)]
struct CommunityJson {
    id: i64,
    name: String,
    level: i64,
    cohesion: f64,
    size: i64,
    dominant_language: String,
    description: String,
    members: Vec<String>,
}

#[derive(Serialize)]
struct GraphNodeJson {
    id: i64,
    kind: String,
    name: String,
    qualified_name: String,
    file_path: String,
    line_start: i64,
    line_end: i64,
    language: String,
    parent_name: Option<String>,
    is_test: bool,
}

pub(crate) fn flow_json_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let path_json: String = row.get("path_json")?;
    let path = serde_json::from_str::<Vec<i64>>(&path_json).unwrap_or_default();
    let name: String = row.get("name")?;
    flow_json_value_from_parts(row, &name, &path)
}

pub(crate) struct FlowValue {
    pub(crate) value: Value,
    pub(crate) path_ids: Vec<i64>,
}

pub(crate) fn flow_value_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FlowValue> {
    let path_json: String = row.get("path_json")?;
    let path_ids = serde_json::from_str::<Vec<i64>>(&path_json).unwrap_or_default();
    let name: String = row.get("name")?;
    let value = flow_json_value_from_parts(row, &name, &path_ids)?;
    Ok(FlowValue { value, path_ids })
}

pub(crate) fn flow_json_value_from_parts(
    row: &rusqlite::Row<'_>,
    name: &str,
    path: &[i64],
) -> rusqlite::Result<Value> {
    Ok(json!(FlowJson {
        id: row.get::<_, i64>("id")?,
        name: sanitize_name(name),
        entry_point_id: row.get::<_, i64>("entry_point_id")?,
        depth: row.get::<_, i64>("depth")?,
        node_count: row.get::<_, i64>("node_count")?,
        file_count: row.get::<_, i64>("file_count")?,
        criticality: row.get::<_, f64>("criticality")?,
        path,
        created_at: row.get::<_, String>("created_at")?,
        updated_at: row.get::<_, String>("updated_at")?,
    }))
}

pub(crate) fn flow_steps_from_nodes(
    path_ids: &[i64],
    nodes_by_id: &HashMap<i64, GraphNode>,
) -> Vec<Value> {
    let mut steps = Vec::new();
    for node_id in path_ids {
        if let Some(node) = nodes_by_id.get(node_id) {
            steps.push(json!(FlowStepJson {
                node_id: node.id,
                name: sanitize_name(&node.name),
                kind: node.kind.clone(),
                file: node.file_path.clone(),
                line_start: node.line_start,
                line_end: node.line_end,
                qualified_name: sanitize_name(&node.qualified_name),
            }));
        }
    }
    steps
}

pub(crate) fn community_json_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let name: String = row.get("name")?;
    let description = row
        .get::<_, Option<String>>("description")?
        .unwrap_or_default();
    Ok(json!(CommunityJson {
        id: row.get::<_, i64>("id")?,
        name: sanitize_name(&name),
        level: row.get::<_, i64>("level")?,
        cohesion: row.get::<_, f64>("cohesion")?,
        size: row.get::<_, i64>("size")?,
        dominant_language: row
            .get::<_, Option<String>>("dominant_language")?
            .unwrap_or_default(),
        description: sanitize_name(&description),
        members: Vec::new(),
    }))
}

pub(crate) fn sync_fts_after_file_batch_tx(
    tx: &Transaction<'_>,
    file_paths: &[String],
) -> Result<()> {
    let repo_root = tx
        .query_row(
            "SELECT value FROM metadata WHERE key = 'repo_root'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    crate::fts_sync::sync_fts_for_file_paths_tx(
        tx,
        file_paths,
        repo_root.as_deref().map(Path::new),
    )?;
    Ok(())
}

pub(crate) fn store_file_batch_tx(
    tx: &Transaction<'_>,
    batch: &[FileBatchItem],
    suspend_indexes: bool,
) -> Result<()> {
    let now = now_seconds()?;
    let suspend_indexes = suspend_indexes && should_suspend_write_indexes(tx, batch.len())?;
    if suspend_indexes {
        drop_graph_write_indexes(tx)?;
    }
    let file_paths = batch
        .iter()
        .map(|(file_path, _, _, _, _)| file_path.clone())
        .collect::<Vec<_>>();
    remove_files_data_tx(tx, &file_paths)?;

    let mut seen_edges = HashSet::new();
    let mut node_params =
        Vec::<SqlValue>::with_capacity(NODE_INSERT_ROWS * NODE_INSERT_PARAM_COUNT);
    let mut node_rows = 0_usize;
    let mut edge_params =
        Vec::<SqlValue>::with_capacity(EDGE_INSERT_ROWS * EDGE_INSERT_PARAM_COUNT);
    let mut edge_rows = 0_usize;

    for (_file_path, nodes, edges, file_hash, mtime_ns) in batch {
        for node in nodes {
            let qualified = make_qualified_parts(
                &node.kind,
                &node.name,
                &node.file_path,
                node.parent_name.as_deref(),
            );
            let extra = extra_json(&node.extra)?;
            push_text(&mut node_params, &node.kind);
            push_text(&mut node_params, &node.name);
            node_params.push(SqlValue::Text(qualified));
            push_text(&mut node_params, &node.file_path);
            node_params.push(SqlValue::Integer(node.line_start));
            node_params.push(SqlValue::Integer(node.line_end));
            push_text(&mut node_params, &node.language);
            push_optional_text(&mut node_params, node.parent_name.as_deref());
            push_optional_text(&mut node_params, node.params.as_deref());
            push_optional_text(&mut node_params, node.return_type.as_deref());
            push_optional_text(&mut node_params, node.modifiers.as_deref());
            node_params.push(SqlValue::Integer(i64::from(node.is_test)));
            push_text(&mut node_params, file_hash);
            node_params.push(SqlValue::Integer(*mtime_ns));
            node_params.push(SqlValue::Text(extra));
            node_params.push(SqlValue::Real(now));
            node_rows += 1;
            if node_rows == NODE_INSERT_ROWS {
                insert_compact_node_rows(tx, node_rows, &node_params)?;
                node_params.clear();
                node_rows = 0;
            }
        }

        for edge in edges {
            let key = (
                edge.kind.as_str(),
                edge.source.as_str(),
                edge.target.as_str(),
                edge.file_path.as_str(),
                edge.line,
            );
            if !seen_edges.insert(key) {
                continue;
            }
            let confidence = edge
                .extra
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let confidence_tier =
                ConfidenceTier::from_raw(edge.extra.get("confidence_tier").and_then(Value::as_str));
            let (confidence, confidence_tier) =
                normalize_edge_confidence(&edge.source, &edge.target, confidence, confidence_tier);
            let extra_json = extra_json(&edge.extra)?;
            push_text(&mut edge_params, &edge.kind);
            push_text(&mut edge_params, &edge.source);
            push_text(&mut edge_params, &edge.target);
            push_text(&mut edge_params, &edge_target_name(&edge.target));
            push_text(&mut edge_params, &edge.file_path);
            edge_params.push(SqlValue::Integer(edge.line));
            edge_params.push(SqlValue::Text(extra_json));
            edge_params.push(SqlValue::Real(confidence));
            push_text(&mut edge_params, confidence_tier.as_str());
            edge_params.push(SqlValue::Real(now));
            edge_rows += 1;
            if edge_rows == EDGE_INSERT_ROWS {
                insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
                edge_params.clear();
                edge_rows = 0;
            }
        }
    }
    if node_rows > 0 {
        insert_compact_node_rows(tx, node_rows, &node_params)?;
    }
    if edge_rows > 0 {
        insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
    }
    if suspend_indexes {
        create_graph_write_indexes(tx)?;
    }
    sync_fts_after_file_batch_tx(tx, &file_paths)?;
    Ok(())
}

pub(crate) fn store_raw_compact_file_batch_tx(
    tx: &Transaction<'_>,
    batch: &[RawCompactFileBatchItem],
    suspend_indexes: bool,
) -> Result<()> {
    let now = now_seconds()?;
    let suspend_indexes = suspend_indexes && should_suspend_write_indexes(tx, batch.len())?;
    if suspend_indexes {
        drop_graph_write_indexes(tx)?;
    }
    let file_paths = batch
        .iter()
        .map(|(file_path, _, _, _, _)| file_path.clone())
        .collect::<Vec<_>>();
    remove_files_data_tx(tx, &file_paths)?;

    let mut seen_edges = HashSet::new();
    let mut node_params =
        Vec::<SqlValue>::with_capacity(NODE_INSERT_ROWS * NODE_INSERT_PARAM_COUNT);
    let mut node_rows = 0_usize;
    let mut edge_params =
        Vec::<SqlValue>::with_capacity(EDGE_INSERT_ROWS * EDGE_INSERT_PARAM_COUNT);
    let mut edge_rows = 0_usize;

    for (_file_path, nodes, edges, file_hash, mtime_ns) in batch {
        for node in nodes {
            let RawCompactNodeInput(
                kind,
                name,
                file_path,
                line_start,
                line_end,
                language,
                parent_name,
                params,
                return_type,
                modifiers,
                is_test,
                extra,
            ) = node;
            let qualified = make_qualified_parts(kind, name, file_path, parent_name.as_deref());
            push_text(&mut node_params, kind);
            push_text(&mut node_params, name);
            node_params.push(SqlValue::Text(qualified));
            push_text(&mut node_params, file_path);
            node_params.push(SqlValue::Integer(*line_start));
            node_params.push(SqlValue::Integer(*line_end));
            push_text(&mut node_params, language);
            push_optional_text(&mut node_params, parent_name.as_deref());
            push_optional_text(&mut node_params, params.as_deref());
            push_optional_text(&mut node_params, return_type.as_deref());
            push_optional_text(&mut node_params, modifiers.as_deref());
            node_params.push(SqlValue::Integer(i64::from(*is_test)));
            push_text(&mut node_params, file_hash);
            node_params.push(SqlValue::Integer(*mtime_ns));
            node_params.push(SqlValue::Text(extra.get().to_string()));
            node_params.push(SqlValue::Real(now));
            node_rows += 1;
            if node_rows == NODE_INSERT_ROWS {
                insert_compact_node_rows(tx, node_rows, &node_params)?;
                node_params.clear();
                node_rows = 0;
            }
        }

        for edge in edges {
            let RawCompactEdgeInput(kind, source, target, file_path, line, extra) = edge;
            let key = (
                kind.as_str(),
                source.as_str(),
                target.as_str(),
                file_path.as_str(),
                *line,
            );
            if !seen_edges.insert(key) {
                continue;
            }
            let (confidence, confidence_tier) = edge_metadata_from_raw_extra(extra.get())?;
            let (confidence, confidence_tier) =
                normalize_edge_confidence(source, target, confidence, confidence_tier);
            push_text(&mut edge_params, kind);
            push_text(&mut edge_params, source);
            push_text(&mut edge_params, target);
            push_text(&mut edge_params, &edge_target_name(target));
            push_text(&mut edge_params, file_path);
            edge_params.push(SqlValue::Integer(*line));
            edge_params.push(SqlValue::Text(extra.get().to_string()));
            edge_params.push(SqlValue::Real(confidence));
            edge_params.push(SqlValue::Text(confidence_tier.as_str().to_string()));
            edge_params.push(SqlValue::Real(now));
            edge_rows += 1;
            if edge_rows == EDGE_INSERT_ROWS {
                insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
                edge_params.clear();
                edge_rows = 0;
            }
        }
    }
    if node_rows > 0 {
        insert_compact_node_rows(tx, node_rows, &node_params)?;
    }
    if edge_rows > 0 {
        insert_compact_edge_rows(tx, edge_rows, &edge_params)?;
    }
    if suspend_indexes {
        create_graph_write_indexes(tx)?;
    }
    sync_fts_after_file_batch_tx(tx, &file_paths)?;
    Ok(())
}

pub(crate) fn should_suspend_write_indexes(
    tx: &Transaction<'_>,
    file_count: usize,
) -> Result<bool> {
    if file_count < SUSPEND_INDEX_FILE_THRESHOLD {
        return Ok(false);
    }
    let has_nodes: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM nodes LIMIT 1)", [], |row| {
        row.get(0)
    })?;
    if has_nodes != 0 {
        return Ok(false);
    }
    let has_edges: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM edges LIMIT 1)", [], |row| {
        row.get(0)
    })?;
    Ok(has_edges == 0)
}

pub(crate) fn drop_graph_write_indexes(tx: &Transaction<'_>) -> Result<()> {
    for (name, _) in WRITE_INDEXES {
        tx.execute(&format!("DROP INDEX IF EXISTS {name}"), [])?;
    }
    Ok(())
}

pub(crate) fn create_graph_write_indexes(tx: &Transaction<'_>) -> Result<()> {
    for (_, sql) in WRITE_INDEXES {
        tx.execute(sql, [])?;
    }
    Ok(())
}

pub(crate) fn edge_metadata_from_raw_extra(raw: &str) -> Result<(f64, ConfidenceTier)> {
    if raw == "{}" {
        return Ok((1.0, ConfidenceTier::default()));
    }
    let extra: Value = serde_json::from_str(raw)?;
    let confidence = extra
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let confidence_tier =
        ConfidenceTier::from_raw(extra.get("confidence_tier").and_then(Value::as_str));
    Ok((confidence, confidence_tier))
}

pub(crate) fn normalize_edge_confidence(
    source: &str,
    target: &str,
    confidence: f64,
    confidence_tier: ConfidenceTier,
) -> (f64, ConfidenceTier) {
    if (source.starts_with("<unresolved:") || target.starts_with("<unresolved:"))
        && matches!(confidence_tier, ConfidenceTier::Extracted | ConfidenceTier::Unknown)
    {
        return (confidence.min(0.2), ConfidenceTier::Low);
    }
    (confidence, confidence_tier)
}

pub(crate) fn push_text(params: &mut Vec<SqlValue>, value: &str) {
    params.push(SqlValue::Text(value.to_string()));
}

pub(crate) fn push_optional_text(params: &mut Vec<SqlValue>, value: Option<&str>) {
    match value {
        Some(value) => params.push(SqlValue::Text(value.to_string())),
        None => params.push(SqlValue::Null),
    }
}

pub(crate) fn insert_compact_node_rows(
    tx: &Transaction<'_>,
    rows: usize,
    values: &[SqlValue],
) -> Result<()> {
    let sql = format!(
        r#"
        INSERT INTO nodes
            (kind, name, qualified_name, file_path, line_start, line_end,
             language, parent_name, params, return_type, modifiers, is_test,
             file_hash, mtime_ns, extra, updated_at)
        VALUES {}
        ON CONFLICT(qualified_name) DO UPDATE SET
            kind=excluded.kind, name=excluded.name,
            file_path=excluded.file_path, line_start=excluded.line_start,
            line_end=excluded.line_end, language=excluded.language,
            parent_name=excluded.parent_name, params=excluded.params,
            return_type=excluded.return_type, modifiers=excluded.modifiers,
            is_test=excluded.is_test, file_hash=excluded.file_hash,
            mtime_ns=excluded.mtime_ns, extra=excluded.extra, updated_at=excluded.updated_at
        "#,
        value_placeholders(NODE_INSERT_PARAM_COUNT, rows)
    );
    tx.execute(&sql, rusqlite::params_from_iter(values.iter()))?;
    Ok(())
}

pub(crate) fn insert_compact_edge_rows(
    tx: &Transaction<'_>,
    rows: usize,
    values: &[SqlValue],
) -> Result<()> {
    let sql = format!(
        r#"
        INSERT INTO edges
            (kind, source_qualified, target_qualified, target_name, file_path, line, extra,
             confidence, confidence_tier, updated_at)
        VALUES {}
        "#,
        value_placeholders(EDGE_INSERT_PARAM_COUNT, rows)
    );
    tx.execute(&sql, rusqlite::params_from_iter(values.iter()))?;
    Ok(())
}

pub(crate) fn value_placeholders(width: usize, rows: usize) -> String {
    let row = format!(
        "({})",
        std::iter::repeat_n("?", width)
            .collect::<Vec<_>>()
            .join(",")
    );
    std::iter::repeat_n(row, rows).collect::<Vec<_>>().join(",")
}

pub(crate) fn node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphNode> {
    let extra: Option<String> = row.get("extra")?;
    Ok(GraphNode {
        id: row.get("id")?,
        kind: row.get("kind")?,
        name: row.get("name")?,
        qualified_name: row.get("qualified_name")?,
        file_path: row.get("file_path")?,
        line_start: row.get("line_start")?,
        line_end: row.get("line_end")?,
        language: row
            .get::<_, Option<String>>("language")?
            .unwrap_or_default(),
        parent_name: row.get("parent_name")?,
        params: row.get("params")?,
        return_type: row.get("return_type")?,
        is_test: row.get::<_, i64>("is_test")? != 0,
        file_hash: row.get("file_hash")?,
        extra: parse_json_column(extra).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?,
    })
}

pub(crate) fn edge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphEdge> {
    let extra: Option<String> = row.get("extra")?;
    Ok(GraphEdge {
        id: row.get("id")?,
        kind: row.get("kind")?,
        source_qualified: row.get("source_qualified")?,
        target_qualified: row.get("target_qualified")?,
        file_path: row.get("file_path")?,
        line: row.get("line")?,
        extra: parse_json_column(extra).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?,
        confidence: row.get::<_, Option<f64>>("confidence")?.unwrap_or(1.0),
        confidence_tier: ConfidenceTier::from_raw(
            row.get::<_, Option<String>>("confidence_tier")?.as_deref(),
        ),
    })
}

pub(crate) fn node_to_value(node: &GraphNode) -> Value {
    json!(GraphNodeJson {
        id: node.id,
        kind: node.kind.clone(),
        name: sanitize_name(&node.name),
        qualified_name: sanitize_name(&node.qualified_name),
        file_path: node.file_path.clone(),
        line_start: node.line_start,
        line_end: node.line_end,
        language: node.language.clone(),
        parent_name: node.parent_name.as_deref().map(sanitize_name),
        is_test: node.is_test,
    })
}

pub(crate) fn parse_json_column(raw: Option<String>) -> serde_json::Result<Value> {
    match raw {
        Some(raw) if !raw.is_empty() => serde_json::from_str(&raw),
        _ => Ok(Value::Object(Default::default())),
    }
}

pub(crate) fn identifier_search_text<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let mut tokens = Vec::new();
    for value in values {
        let mut chunk = String::new();
        for ch in value.chars() {
            if ch.is_ascii_alphanumeric() {
                chunk.push(ch);
            } else if !chunk.is_empty() {
                push_identifier_parts(&chunk, &mut tokens);
                chunk.clear();
            }
        }
        if !chunk.is_empty() {
            push_identifier_parts(&chunk, &mut tokens);
        }
    }
    tokens.join(" ")
}

pub(crate) fn contains_japanese(text: &str) -> bool {
    text.chars().any(is_japanese_char)
}

pub(crate) fn segment_japanese_fts_text(text: &str) -> String {
    if text.is_empty() || !contains_japanese(text) {
        return text.to_string();
    }

    let mut tokens = Vec::new();
    let mut chunk = String::new();
    let mut chunk_kind = ChunkKind::Other;
    for ch in text.chars() {
        let next_kind = chunk_kind_for(ch);
        if next_kind == ChunkKind::Other {
            flush_fts_chunk(&mut chunk, chunk_kind, &mut tokens);
            chunk_kind = ChunkKind::Other;
            continue;
        }
        if chunk_kind != ChunkKind::Other && next_kind != chunk_kind {
            flush_fts_chunk(&mut chunk, chunk_kind, &mut tokens);
        }
        chunk.push(ch);
        chunk_kind = next_kind;
    }
    flush_fts_chunk(&mut chunk, chunk_kind, &mut tokens);
    tokens.join(" ")
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ChunkKind {
    Ascii,
    Japanese,
    Other,
}

fn chunk_kind_for(ch: char) -> ChunkKind {
    if ch.is_ascii_alphanumeric() || ch == '_' {
        ChunkKind::Ascii
    } else if is_japanese_char(ch) {
        ChunkKind::Japanese
    } else {
        ChunkKind::Other
    }
}

fn is_japanese_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
    )
}

fn flush_fts_chunk(chunk: &mut String, kind: ChunkKind, tokens: &mut Vec<String>) {
    if chunk.is_empty() {
        return;
    }
    match kind {
        ChunkKind::Ascii => tokens.push(std::mem::take(chunk)),
        ChunkKind::Japanese => {
            let chars = chunk.chars().collect::<Vec<_>>();
            if chars.len() <= 2 {
                tokens.push(std::mem::take(chunk));
            } else {
                for idx in 0..chars.len() - 1 {
                    tokens.push(chars[idx..idx + 2].iter().collect());
                }
                chunk.clear();
            }
        }
        ChunkKind::Other => chunk.clear(),
    }
}

pub(crate) fn push_identifier_parts(chunk: &str, tokens: &mut Vec<String>) {
    let chars = chunk.chars().collect::<Vec<_>>();
    let mut start = 0;
    for idx in 1..chars.len() {
        let prev = chars[idx - 1];
        let current = chars[idx];
        let next = chars.get(idx + 1).copied();
        let lower_to_upper =
            (prev.is_ascii_lowercase() || prev.is_ascii_digit()) && current.is_ascii_uppercase();
        let acronym_boundary = prev.is_ascii_uppercase()
            && current.is_ascii_uppercase()
            && next.is_some_and(|ch| ch.is_ascii_lowercase());
        if lower_to_upper || acronym_boundary {
            tokens.push(
                chars[start..idx]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase(),
            );
            start = idx;
        }
    }
    if start < chars.len() {
        tokens.push(
            chars[start..]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase(),
        );
    }
}

pub(crate) fn read_node_source_excerpt(
    repo_root: Option<&Path>,
    kind: &str,
    file_path: &str,
    line_start: Option<i64>,
    line_end: Option<i64>,
) -> String {
    let mut path = PathBuf::from(file_path);
    if !path.is_absolute() {
        let Some(root) = repo_root else {
            return String::new();
        };
        path = root.join(path);
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let lines = text.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return String::new();
    }
    let start = line_start.unwrap_or(1).saturating_sub(1).max(0) as usize;
    let mut end = line_end
        .unwrap_or(line_start.unwrap_or(1))
        .max(line_start.unwrap_or(1)) as usize;
    let start = start.min(lines.len().saturating_sub(1));
    end = end.min(lines.len());
    if kind == "DocSection" {
        let level = markdown_heading_level(lines[start]);
        end = lines.len();
        for (idx, line) in lines.iter().enumerate().skip(start + 1) {
            if let Some(candidate_level) = markdown_heading_level(line) {
                if level.is_none_or(|current_level| candidate_level <= current_level) {
                    end = idx;
                    break;
                }
            }
        }
    }
    lines[start..end].join("\n").chars().take(4096).collect()
}

pub(crate) fn markdown_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&level) && trimmed.chars().nth(level).is_some_and(|ch| ch == ' ') {
        Some(level)
    } else {
        None
    }
}

pub(crate) fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?",
        [table],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub(crate) fn common_prefix(values: &[String]) -> String {
    let Some((first, rest)) = values.split_first() else {
        return String::new();
    };
    let mut prefix = first.clone();
    for value in rest {
        while !value.starts_with(&prefix) {
            if prefix.pop().is_none() {
                return String::new();
            }
        }
    }
    prefix
}

pub(crate) fn community_purpose(paths: &[String]) -> String {
    let prefix = common_prefix(paths);
    if !prefix.contains('/') {
        return String::new();
    }
    prefix
        .rsplit_once('/')
        .map(|(before_last, _)| before_last.rsplit('/').next().unwrap_or(""))
        .unwrap_or("")
        .to_string()
}

pub(crate) fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| *ch == '\t' || *ch == '\n' || (*ch as u32) >= 0x20)
        .take(256)
        .collect()
}
