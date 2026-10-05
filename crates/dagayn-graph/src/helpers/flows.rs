//! Flow and community storage and their JSON shapes.

use super::*;

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
         (name, entry_point_id, depth, node_count, file_count, criticality, path_json, \
          kind, truncated, truncation_reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )?;
    let mut insert_membership = tx.prepare(
        "INSERT OR IGNORE INTO flow_memberships (flow_id, node_id, position) \
         VALUES (?, ?, ?)",
    )?;
    for flow in flows {
        let kind = if flow.kind.is_empty() {
            "reachable_set"
        } else {
            flow.kind.as_str()
        };
        insert_flow.execute(params![
            flow.name,
            flow.entry_point_id,
            flow.depth,
            flow.node_count,
            flow.file_count,
            flow.criticality,
            serde_json::to_string(&flow.path)?,
            kind,
            if flow.truncated { 1 } else { 0 },
            flow.truncation_reason.as_deref(),
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
    kind: String,
    truncated: bool,
    truncation_reason: Option<String>,
    path: &'a [i64],
    members: &'a [i64],
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

pub(crate) fn flow_json_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    flow_value_from_row(row).map(|flow| flow.value)
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
    let kind: String = row
        .get::<_, Option<String>>("kind")?
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "reachable_set".to_string());
    let truncated = row.get::<_, Option<i64>>("truncated")?.unwrap_or(0) != 0;
    let truncation_reason: Option<String> = row.get("truncation_reason")?;
    Ok(json!(FlowJson {
        id: row.get::<_, i64>("id")?,
        name: sanitize_name(name),
        entry_point_id: row.get::<_, i64>("entry_point_id")?,
        depth: row.get::<_, i64>("depth")?,
        node_count: row.get::<_, i64>("node_count")?,
        file_count: row.get::<_, i64>("file_count")?,
        criticality: row.get::<_, f64>("criticality")?,
        kind,
        truncated,
        truncation_reason,
        path,
        members: path,
        created_at: row.get::<_, String>("created_at")?,
        updated_at: row.get::<_, String>("updated_at")?,
    }))
}

pub(crate) fn flow_steps_from_nodes(
    path_ids: &[i64],
    nodes_by_id: &HashMap<i64, GraphNode>,
) -> Vec<Value> {
    path_ids
        .iter()
        .filter_map(|node_id| nodes_by_id.get(node_id))
        .map(|node| {
            json!(FlowStepJson {
                node_id: node.id,
                name: sanitize_name(&node.name),
                kind: node.kind.clone(),
                file: node.file_path.clone(),
                line_start: node.line_start,
                line_end: node.line_end,
                qualified_name: sanitize_name(&node.qualified_name),
            })
        })
        .collect()
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
