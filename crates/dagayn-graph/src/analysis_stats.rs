use crate::*;

impl GraphStore {
    pub fn get_stats(&self) -> Result<GraphStats> {
        let total_nodes = self
            .conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?;
        let total_edges = self
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;

        let mut nodes_by_kind = HashMap::new();
        let mut node_stmt = self
            .conn
            .prepare("SELECT kind, COUNT(*) as cnt FROM nodes GROUP BY kind")?;
        let node_rows = node_stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in node_rows {
            let (kind, count) = row?;
            nodes_by_kind.insert(kind, count);
        }

        let mut edges_by_kind = HashMap::new();
        let mut edge_stmt = self
            .conn
            .prepare("SELECT kind, COUNT(*) as cnt FROM edges GROUP BY kind")?;
        let edge_rows = edge_stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in edge_rows {
            let (kind, count) = row?;
            edges_by_kind.insert(kind, count);
        }

        let mut lang_stmt = self.conn.prepare(
            "SELECT DISTINCT language FROM nodes WHERE language IS NOT NULL AND language != ''",
        )?;
        let lang_rows = lang_stmt.query_map([], |row| row.get::<_, String>(0))?;
        let languages = lang_rows.collect::<std::result::Result<Vec<_>, _>>()?;

        let files_count = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE kind = 'File'",
            [],
            |row| row.get(0),
        )?;
        let last_updated = self.get_metadata("last_updated")?;

        Ok(GraphStats {
            total_nodes,
            total_edges,
            nodes_by_kind,
            edges_by_kind,
            languages,
            files_count,
            last_updated,
        })
    }
}
