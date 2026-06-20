use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn get_flow_edge_data(&self) -> Result<FlowEdgeData> {
        let mut calls_out: HashMap<String, Vec<String>> = HashMap::new();
        let mut has_tested_by: HashSet<String> = HashSet::new();
        let mut stmt = self.conn.prepare(
            "SELECT kind, source_qualified, target_qualified FROM edges \
             WHERE kind IN ('CALLS', 'TESTED_BY')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (kind, source, target) = row?;
            if kind == "CALLS" {
                calls_out.entry(source).or_default().push(target);
            } else {
                has_tested_by.insert(source);
            }
        }
        Ok((calls_out, has_tested_by))
    }

    pub fn store_flows(&mut self, flows: &[FlowInput]) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM flow_snapshots", [])?;
        tx.execute("DELETE FROM flow_memberships", [])?;
        tx.execute("DELETE FROM flows", [])?;
        store_flows_tx(&tx, flows)?;
        tx.commit()?;
        Ok(flows.len() as i64)
    }

    pub fn store_flows_json(&mut self, flows_json: &str) -> Result<i64> {
        let flows: Vec<FlowInput> = serde_json::from_str(flows_json)?;
        self.store_flows(&flows)
    }

    pub fn insert_flows_json(&mut self, flows_json: &str) -> Result<i64> {
        let flows: Vec<FlowInput> = serde_json::from_str(flows_json)?;
        let tx = self.conn.transaction()?;
        store_flows_tx(&tx, &flows)?;
        tx.commit()?;
        Ok(flows.len() as i64)
    }

    pub fn get_flows_json(&self, sort_by: &str, limit: i64) -> Result<String> {
        let sort_by = FlowSortBy::from_raw(sort_by);
        let sql = format!(
            "SELECT * FROM flows ORDER BY {} {} LIMIT ?",
            sort_by.column(),
            sort_by.order()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([limit], flow_json_from_row)?;
        let flows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        serde_json::to_string(&flows).map_err(Into::into)
    }

    pub fn get_flow_by_id_json(&self, flow_id: i64) -> Result<Option<String>> {
        self.get_flow_values_by_ids(&[flow_id])?
            .into_iter()
            .next()
            .map(|flow| serde_json::to_string(&flow).map_err(Into::into))
            .transpose()
    }

    pub fn get_affected_flows_json(&self, changed_files: &[String]) -> Result<String> {
        let flows = self.get_affected_flow_values(changed_files)?;
        serde_json::to_string(&flows).map_err(Into::into)
    }

    pub fn get_node_kind_by_id(&self, node_id: i64) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT kind FROM nodes WHERE id = ?", [node_id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }
}
