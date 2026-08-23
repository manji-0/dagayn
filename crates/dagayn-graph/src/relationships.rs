use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn get_nodes_by_community_id(&self, community_id: i64) -> Result<Vec<GraphNode>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM nodes WHERE community_id = ?")?;
        let rows = stmt.query_map([community_id], node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_files_matching(&self, pattern: &str) -> Result<Vec<String>> {
        let like = format!("%{pattern}");
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT file_path FROM nodes WHERE file_path LIKE ?")?;
        let rows = stmt.query_map([like], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn count_flow_memberships(&self, node_id: i64) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) as cnt FROM flow_memberships WHERE node_id = ?",
                [node_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn count_flow_memberships_for_nodes(&self, node_ids: &[i64]) -> Result<HashMap<i64, i64>> {
        let mut out = node_ids
            .iter()
            .map(|node_id| (*node_id, 0))
            .collect::<HashMap<_, _>>();
        if node_ids.is_empty() {
            return Ok(out);
        }

        for chunk in node_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT node_id, COUNT(*) FROM flow_memberships \
                 WHERE node_id IN ({placeholders}) GROUP BY node_id"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (node_id, count) = row?;
                out.insert(node_id, count);
            }
        }
        Ok(out)
    }

    pub fn get_flow_criticalities_for_node(&self, node_id: i64) -> Result<Vec<f64>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.criticality FROM flows f \
             JOIN flow_memberships fm ON fm.flow_id = f.id \
             WHERE fm.node_id = ?",
        )?;
        let rows = stmt.query_map([node_id], |row| row.get::<_, f64>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_flow_criticalities_for_nodes(
        &self,
        node_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<f64>>> {
        let mut out = node_ids
            .iter()
            .map(|node_id| (*node_id, Vec::new()))
            .collect::<HashMap<_, _>>();
        if node_ids.is_empty() {
            return Ok(out);
        }

        for chunk in node_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT fm.node_id, f.criticality FROM flows f \
                 JOIN flow_memberships fm ON fm.flow_id = f.id \
                 WHERE fm.node_id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })?;
            for row in rows {
                let (node_id, criticality) = row?;
                out.entry(node_id).or_default().push(criticality);
            }
        }
        Ok(out)
    }

    pub fn get_node_community_id(&self, node_id: i64) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT community_id FROM nodes WHERE id = ?",
                [node_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(|row| row.flatten())
            .map_err(Into::into)
    }

    pub fn get_community_ids_by_node_ids(
        &self,
        node_ids: &[i64],
    ) -> Result<HashMap<i64, Option<i64>>> {
        let mut out = node_ids
            .iter()
            .map(|node_id| (*node_id, None))
            .collect::<HashMap<_, _>>();
        if node_ids.is_empty() {
            return Ok(out);
        }

        for chunk in node_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id, community_id FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
            })?;
            for row in rows {
                let (node_id, community_id) = row?;
                out.insert(node_id, community_id);
            }
        }
        Ok(out)
    }

    pub fn get_community_ids_by_qualified_names(
        &self,
        qns: &[String],
    ) -> Result<HashMap<String, Option<i64>>> {
        let mut out = HashMap::new();
        for chunk in qns.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT qualified_name, community_id FROM nodes \
                 WHERE qualified_name IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })?;
            for row in rows {
                let (qualified_name, community_id) = row?;
                out.insert(qualified_name, community_id);
            }
        }
        Ok(out)
    }

    pub fn get_all_community_ids(&self) -> Result<HashMap<String, Option<i64>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT qualified_name, community_id FROM nodes")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })?;
        rows.collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(Into::into)
    }

    pub fn count_affected_communities(&self, file_paths: &[String]) -> Result<i64> {
        let mut community_ids = HashSet::new();
        for chunk in file_paths.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT community_id FROM nodes \
                 WHERE community_id IS NOT NULL AND file_path IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, i64>(0)
            })?;
            for row in rows {
                community_ids.insert(row?);
            }
        }
        if !community_ids.is_empty() {
            return Ok(community_ids.len() as i64);
        }

        let community_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM communities", [], |row| row.get(0))?;
        if community_count == 0 {
            return Ok(0);
        }

        for chunk in file_paths.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT COUNT(*) FROM nodes \
                 WHERE community_id IS NULL AND kind != 'File' \
                 AND file_path IN ({placeholders})"
            );
            let count: i64 =
                self.conn
                    .query_row(&sql, rusqlite::params_from_iter(chunk), |row| row.get(0))?;
            if count > 0 {
                return Ok(1);
            }
        }
        Ok(0)
    }
}
