use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn delete_affected_flows(&mut self, changed_files: &[String]) -> Result<Vec<i64>> {
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

        let tx = self.conn.transaction()?;
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

    pub(crate) fn get_flow_ids_by_node_ids(&self, node_ids: &HashSet<i64>) -> Result<Vec<i64>> {
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
