use crate::helpers::write_tx;
use crate::*;

impl GraphStore {
    pub(crate) fn compute_flow_snapshots(&mut self) -> Result<()> {
        let tx = write_tx(&mut self.conn)?;
        tx.execute("DELETE FROM flow_snapshots", [])?;

        let flow_rows = {
            let mut stmt = tx.prepare(
                "SELECT id, name, entry_point_id, criticality, node_count, \
                 file_count, path_json FROM flows",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut needed_ids: HashSet<i64> = HashSet::new();
        let mut parsed_paths = Vec::with_capacity(flow_rows.len());
        for (_, _, entry_point_id, _, _, _, path_json) in &flow_rows {
            needed_ids.insert(*entry_point_id);
            let path_ids = if path_json.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str::<Vec<i64>>(path_json)?
            };
            for node_id in path_ids.iter().skip(1).take(3) {
                needed_ids.insert(*node_id);
            }
            if let Some(last) = path_ids.last() {
                needed_ids.insert(*last);
            }
            parsed_paths.push(path_ids);
        }

        let mut id_to_name: HashMap<i64, String> = HashMap::new();
        let id_list = needed_ids.into_iter().collect::<Vec<_>>();
        for chunk in id_list.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id, qualified_name FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = tx.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (node_id, qualified_name) = row?;
                id_to_name.insert(node_id, qualified_name);
            }
        }

        let mut insert = tx.prepare(
            "INSERT OR REPLACE INTO flow_snapshots \
             (flow_id, name, entry_point, critical_path, criticality, node_count, file_count) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )?;
        for ((flow_id, name, entry_point_id, criticality, node_count, file_count, _), path_ids) in
            flow_rows.into_iter().zip(parsed_paths)
        {
            let entry_point = id_to_name
                .get(&entry_point_id)
                .cloned()
                .unwrap_or_else(|| entry_point_id.to_string());
            let mut critical_path = Vec::new();
            if !path_ids.is_empty() {
                critical_path.push(entry_point.clone());
                if path_ids.len() > 2 {
                    for node_id in path_ids.iter().skip(1).take(3) {
                        if let Some(name) = id_to_name.get(node_id) {
                            critical_path.push(name.clone());
                        }
                    }
                }
                if path_ids.len() > 1 {
                    if let Some(last) = path_ids.last().and_then(|node_id| id_to_name.get(node_id))
                    {
                        if !critical_path.contains(last) {
                            critical_path.push(last.clone());
                        }
                    }
                }
            }
            insert.execute(params![
                flow_id,
                name,
                entry_point,
                serde_json::to_string(&critical_path)?,
                criticality,
                node_count,
                file_count
            ])?;
        }
        drop(insert);
        tx.commit()?;
        Ok(())
    }
}
