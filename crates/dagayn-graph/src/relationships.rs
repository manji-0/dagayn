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

    pub fn get_all_community_ids(&self) -> Result<HashMap<String, Option<i64>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT qualified_name, community_id FROM nodes")?;
        let rows = stmt.query_map([], |row| <(String, Option<i64>)>::try_from(row))?;
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

impl GraphStore {}
