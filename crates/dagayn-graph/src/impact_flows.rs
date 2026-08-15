use std::collections::{HashMap, HashSet, VecDeque};

use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn delete_affected_flows(&mut self, changed_files: &[String]) -> Result<Vec<i64>> {
        if changed_files.is_empty() {
            return Ok(Vec::new());
        }

        let flow_ids = self.get_affected_flow_ids(changed_files)?;
        if flow_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut entry_point_ids = Vec::new();
        let mut seen_entry_points = HashSet::new();
        for chunk in flow_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT entry_point_id FROM flows WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, i64>(0)
            })?;
            for row in rows {
                let entry_point_id = row?;
                if seen_entry_points.insert(entry_point_id) {
                    entry_point_ids.push(entry_point_id);
                }
            }
        }

        let tx = write_tx(&mut self.conn)?;
        {
            let mut delete_snapshot = tx.prepare("DELETE FROM flow_snapshots WHERE flow_id = ?")?;
            let mut delete_membership =
                tx.prepare("DELETE FROM flow_memberships WHERE flow_id = ?")?;
            let mut delete_flow = tx.prepare("DELETE FROM flows WHERE id = ?")?;
            for flow_id in flow_ids {
                delete_snapshot.execute([flow_id])?;
                delete_membership.execute([flow_id])?;
                delete_flow.execute([flow_id])?;
            }
        }
        tx.commit()?;
        Ok(entry_point_ids)
    }

    pub(crate) fn get_affected_flow_ids(&self, changed_files: &[String]) -> Result<Vec<i64>> {
        if changed_files.is_empty() {
            return Ok(Vec::new());
        }

        let file_keys = self.expand_file_keys(changed_files)?;
        if file_keys.is_empty() {
            return Ok(Vec::new());
        }

        let mut affected = HashSet::new();
        let placeholders = std::iter::repeat_n("?", file_keys.len())
            .collect::<Vec<_>>()
            .join(",");

        let membership_sql = format!(
            "SELECT DISTINCT fm.flow_id FROM flow_memberships fm \
             JOIN nodes n ON n.id = fm.node_id \
             WHERE n.file_path IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&membership_sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&file_keys), |row| row.get(0))?;
        for row in rows {
            affected.insert(row?);
        }

        let entry_sql = format!(
            "SELECT f.id FROM flows f \
             JOIN nodes n ON n.id = f.entry_point_id \
             WHERE n.file_path IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&entry_sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&file_keys), |row| row.get(0))?;
        for row in rows {
            affected.insert(row?);
        }

        let path_sql = format!(
            "SELECT DISTINCT f.id FROM flows f, json_each(f.path_json) AS je \
             JOIN nodes n ON n.id = CAST(je.value AS INTEGER) \
             WHERE n.file_path IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&path_sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&file_keys), |row| row.get(0))?;
        for row in rows {
            affected.insert(row?);
        }

        let dangling_entry_sql = "SELECT f.id, f.name FROM flows f \
                                  LEFT JOIN nodes n ON n.id = f.entry_point_id \
                                  WHERE n.id IS NULL";
        let mut stmt = self.conn.prepare(dangling_entry_sql)?;
        let dangling_rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in dangling_rows {
            let (flow_id, flow_name) = row?;
            let match_sql = format!(
                "SELECT 1 FROM nodes WHERE file_path IN ({placeholders}) AND name = ? LIMIT 1"
            );
            let matched = self.conn.query_row(
                &match_sql,
                rusqlite::params_from_iter(file_keys.iter().chain(std::iter::once(&flow_name))),
                |_| Ok(()),
            );
            if matched.is_ok() {
                affected.insert(flow_id);
            }
        }

        let mut changed_qnames = HashSet::new();
        let qn_sql =
            format!("SELECT qualified_name FROM nodes WHERE file_path IN ({placeholders})");
        let mut stmt = self.conn.prepare(&qn_sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&file_keys), |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            changed_qnames.insert(row?);
        }

        let changed_file_set: HashSet<String> = file_keys.iter().cloned().collect();
        let stale_sql = "SELECT DISTINCT f.id, f.entry_point_id, f.path_json \
                         FROM flows f \
                         WHERE EXISTS (\
                           SELECT 1 FROM json_each(f.path_json) AS je \
                           LEFT JOIN nodes n ON n.id = CAST(je.value AS INTEGER) \
                           WHERE n.id IS NULL\
                         )";
        let mut stmt = self.conn.prepare(stale_sql)?;
        let stale_rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        let mut calls_out: Option<HashMap<String, Vec<String>>> = None;
        for row in stale_rows {
            let (flow_id, entry_point_id, path_json) = row?;
            if affected.contains(&flow_id) {
                continue;
            }
            let path_ids: Vec<i64> = serde_json::from_str(&path_json).unwrap_or_default();
            let entry_qn = self.resolve_flow_entry_qn(flow_id, entry_point_id, &path_ids)?;
            let Some(entry_qn) = entry_qn else {
                affected.insert(flow_id);
                continue;
            };
            if calls_out.is_none() {
                let (edges, _) = self.get_flow_edge_data()?;
                calls_out = Some(edges);
            }
            if entry_reaches_changed_files(
                calls_out.as_ref().expect("calls_out initialized"),
                &entry_qn,
                &changed_file_set,
                &changed_qnames,
            ) {
                affected.insert(flow_id);
            }
        }

        let mut out: Vec<i64> = affected.into_iter().collect();
        out.sort_unstable();
        Ok(out)
    }

    fn resolve_flow_entry_qn(
        &self,
        flow_id: i64,
        entry_point_id: i64,
        path_ids: &[i64],
    ) -> Result<Option<String>> {
        if let Some(node) = self
            .get_nodes_by_ids(&[entry_point_id])?
            .get(&entry_point_id)
        {
            return Ok(Some(node.qualified_name.clone()));
        }

        let membership_sql = "SELECT n.qualified_name FROM flow_memberships fm \
                              JOIN nodes n ON n.id = fm.node_id \
                              WHERE fm.flow_id = ? ORDER BY fm.position LIMIT 1";
        if let Some(qn) = self
            .conn
            .query_row(membership_sql, [flow_id], |row| row.get::<_, String>(0))
            .optional()?
        {
            return Ok(Some(qn));
        }

        for node_id in path_ids {
            if let Some(node) = self.get_nodes_by_ids(&[*node_id])?.get(node_id) {
                return Ok(Some(node.qualified_name.clone()));
            }
        }
        Ok(None)
    }

    pub fn get_flow_ids_by_node_ids(&self, node_ids: &HashSet<i64>) -> Result<Vec<i64>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let node_ids = node_ids.iter().copied().collect::<Vec<_>>();
        for chunk in node_ids.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT flow_id FROM flow_memberships WHERE node_id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| row.get(0))?;
            for row in rows {
                let flow_id = row?;
                if seen.insert(flow_id) {
                    out.push(flow_id);
                }
            }
        }
        Ok(out)
    }

    pub fn get_flow_qualified_names_for_flows(
        &self,
        flow_ids: &[i64],
    ) -> Result<HashMap<i64, HashSet<String>>> {
        let mut out = flow_ids
            .iter()
            .map(|flow_id| (*flow_id, HashSet::new()))
            .collect::<HashMap<_, _>>();
        if flow_ids.is_empty() {
            return Ok(out);
        }

        let mut unique_ids = Vec::new();
        let mut seen = HashSet::new();
        for flow_id in flow_ids {
            if seen.insert(*flow_id) {
                unique_ids.push(*flow_id);
            }
        }

        for chunk in unique_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT fm.flow_id, n.qualified_name \
                 FROM flow_memberships fm \
                 JOIN nodes n ON fm.node_id = n.id \
                 WHERE fm.flow_id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (flow_id, qualified_name) = row?;
                out.entry(flow_id).or_default().insert(qualified_name);
            }
        }
        Ok(out)
    }

    pub(crate) fn get_flow_values_by_ids(&self, flow_ids: &[i64]) -> Result<Vec<Value>> {
        if flow_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut flows = Vec::new();
        for chunk in flow_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM flows WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), flow_value_from_row)?;
            for row in rows {
                flows.push(row?);
            }
        }

        let mut path_node_ids = HashSet::new();
        for flow in &flows {
            path_node_ids.extend(flow.path_ids.iter().copied());
        }
        let path_node_ids = path_node_ids.into_iter().collect::<Vec<_>>();
        let nodes_by_id = self.get_nodes_by_ids(&path_node_ids)?;

        for flow in &mut flows {
            let steps = flow_steps_from_nodes(&flow.path_ids, &nodes_by_id);
            if let Some(obj) = flow.value.as_object_mut() {
                obj.insert("steps".to_string(), Value::Array(steps));
            }
        }
        Ok(flows.into_iter().map(|flow| flow.value).collect())
    }
}

fn qualified_name_file(qualified_name: &str) -> &str {
    qualified_name
        .rsplit_once("::")
        .map(|(file, _)| file)
        .unwrap_or(qualified_name)
}

fn entry_reaches_changed_files(
    calls_out: &HashMap<String, Vec<String>>,
    entry_qn: &str,
    changed_files: &HashSet<String>,
    changed_qnames: &HashSet<String>,
) -> bool {
    if changed_qnames.contains(entry_qn) || changed_files.contains(qualified_name_file(entry_qn)) {
        return true;
    }

    let mut visited = HashSet::new();
    let mut queue = VecDeque::from([(entry_qn.to_string(), 0i64)]);
    while let Some((qn, depth)) = queue.pop_front() {
        if !visited.insert(qn.clone()) {
            continue;
        }
        if changed_qnames.contains(&qn) || changed_files.contains(qualified_name_file(&qn)) {
            return true;
        }
        if depth >= 15 {
            continue;
        }
        if let Some(targets) = calls_out.get(&qn) {
            for target in targets {
                queue.push_back((target.clone(), depth + 1));
            }
        }
    }
    false
}
