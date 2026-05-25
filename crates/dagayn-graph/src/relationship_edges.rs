use crate::*;

impl GraphStore {
    pub(crate) fn get_node_kinds_by_qualified_names(
        &self,
        qualified_names: &[String],
    ) -> Result<HashMap<String, String>> {
        let mut out = HashMap::new();
        for chunk in qualified_names.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT qualified_name, kind FROM nodes WHERE qualified_name IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (qualified_name, kind) = row?;
                out.insert(qualified_name, kind);
            }
        }
        Ok(out)
    }

    pub(crate) fn get_contains_targets_by_sources(
        &self,
        source_qualified_names: &[String],
    ) -> Result<HashMap<String, Vec<String>>> {
        self.get_edge_targets_by_sources(source_qualified_names, "CONTAINS")
    }

    pub(crate) fn get_call_targets_by_sources(
        &self,
        source_qualified_names: &[String],
    ) -> Result<HashMap<String, Vec<String>>> {
        self.get_edge_targets_by_sources(source_qualified_names, "CALLS")
    }

    pub(crate) fn get_edge_targets_by_sources(
        &self,
        source_qualified_names: &[String],
        kind: &str,
    ) -> Result<HashMap<String, Vec<String>>> {
        let mut out = HashMap::new();
        for chunk in source_qualified_names.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT source_qualified, target_qualified FROM edges \
                 WHERE kind = ? AND source_qualified IN ({placeholders})"
            );
            let mut params = Vec::with_capacity(chunk.len() + 1);
            params.push(kind.to_string());
            params.extend(chunk.iter().cloned());
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (source, target) = row?;
                out.entry(source).or_insert_with(Vec::new).push(target);
            }
        }
        Ok(out)
    }

    pub(crate) fn get_test_targets_by_sources(
        &self,
        sources: &[String],
    ) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        for chunk in sources.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT e.source_qualified, e.target_qualified FROM edges e \
                 JOIN nodes n ON n.qualified_name = e.target_qualified \
                 WHERE e.kind = 'TESTED_BY' AND e.source_qualified IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }
}
