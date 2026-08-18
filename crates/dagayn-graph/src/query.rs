use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn get_node(&self, qualified_name: &str) -> Result<Option<GraphNode>> {
        for key in self.qualified_key_candidates(qualified_name)? {
            let row = self
                .conn
                .query_row(
                    "SELECT * FROM nodes WHERE qualified_name = ?",
                    [key],
                    node_from_row,
                )
                .optional()?;
            if row.is_some() {
                return Ok(row);
            }
        }
        Ok(None)
    }

    pub fn get_nodes_by_qualified_names(
        &self,
        qualified_names: &[String],
    ) -> Result<HashMap<String, GraphNode>> {
        if qualified_names.is_empty() {
            return Ok(HashMap::new());
        }

        let mut normalized_for = HashMap::new();
        let mut keys = HashSet::new();
        for qualified_name in qualified_names {
            let normalized = self.normalize_qualified_key(qualified_name)?;
            normalized_for.insert(qualified_name.clone(), normalized.clone());
            keys.insert(qualified_name.clone());
            if normalized != *qualified_name {
                keys.insert(normalized);
            }
        }

        let keys = keys.into_iter().collect::<Vec<_>>();
        let mut rows_by_qn = HashMap::new();
        for chunk in keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM nodes WHERE qualified_name IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), node_from_row)?;
            for row in rows {
                let node = row?;
                rows_by_qn
                    .entry(node.qualified_name.clone())
                    .or_insert(node);
            }
        }

        let mut out = HashMap::new();
        for original in qualified_names {
            if let Some(node) = rows_by_qn.get(original) {
                out.insert(original.clone(), node.clone());
                continue;
            }
            if let Some(normalized) = normalized_for.get(original) {
                if let Some(node) = rows_by_qn.get(normalized) {
                    out.insert(original.clone(), node.clone());
                }
            }
        }
        Ok(out)
    }

    pub fn get_nodes_by_ids(&self, node_ids: &[i64]) -> Result<HashMap<i64, GraphNode>> {
        let mut out = HashMap::new();
        if node_ids.is_empty() {
            return Ok(out);
        }

        let mut unique_ids = Vec::new();
        let mut seen = HashSet::new();
        for node_id in node_ids {
            if seen.insert(*node_id) {
                unique_ids.push(*node_id);
            }
        }

        for chunk in unique_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), node_from_row)?;
            for row in rows {
                let node = row?;
                out.insert(node.id, node);
            }
        }
        Ok(out)
    }

    pub fn get_node_signatures_by_ids(
        &self,
        node_ids: &[i64],
    ) -> Result<HashMap<i64, Option<String>>> {
        let mut out = HashMap::new();
        if node_ids.is_empty() {
            return Ok(out);
        }
        for chunk in node_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id, signature FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            for row in rows {
                let (node_id, signature) = row?;
                out.insert(node_id, signature);
            }
        }
        Ok(out)
    }

    pub fn get_nodes_by_file(&self, file_path: &str) -> Result<Vec<GraphNode>> {
        let mut seen = std::collections::HashSet::<i64>::new();
        let mut nodes = Vec::new();
        for key in self.file_key_candidates(file_path)? {
            let mut stmt = self
                .conn
                .prepare("SELECT * FROM nodes WHERE file_path = ?")?;
            let rows = stmt.query_map([key], node_from_row)?;
            for row in rows {
                let node = row?;
                if seen.insert(node.id) {
                    nodes.push(node);
                }
            }
        }
        Ok(nodes)
    }

    pub fn get_nodes_by_files(
        &self,
        file_paths: &[String],
    ) -> Result<HashMap<String, Vec<GraphNode>>> {
        let mut out = file_paths
            .iter()
            .map(|file_path| (file_path.clone(), Vec::new()))
            .collect::<HashMap<_, _>>();
        if file_paths.is_empty() {
            return Ok(out);
        }

        let mut key_to_originals: HashMap<String, Vec<String>> = HashMap::new();
        for file_path in file_paths {
            for key in self.file_key_candidates(file_path)? {
                key_to_originals
                    .entry(key)
                    .or_default()
                    .push(file_path.clone());
            }
        }

        let mut seen_by_original = file_paths
            .iter()
            .map(|file_path| (file_path.clone(), HashSet::new()))
            .collect::<HashMap<_, _>>();
        let keys = key_to_originals.keys().cloned().collect::<Vec<_>>();
        for chunk in keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM nodes WHERE file_path IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), node_from_row)?;
            for row in rows {
                let node = row?;
                if let Some(originals) = key_to_originals.get(&node.file_path) {
                    for original in originals {
                        if let Some(seen) = seen_by_original.get_mut(original) {
                            if seen.insert(node.id) {
                                out.entry(original.clone()).or_default().push(node.clone());
                            }
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn get_nodes_by_kind(
        &self,
        kinds: &[String],
        file_pattern: Option<&str>,
    ) -> Result<Vec<GraphNode>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", kinds.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = if file_pattern.is_some() {
            format!("SELECT * FROM nodes WHERE kind IN ({placeholders}) AND file_path LIKE ?")
        } else {
            format!("SELECT * FROM nodes WHERE kind IN ({placeholders})")
        };
        let mut params = kinds.iter().map(String::as_str).collect::<Vec<_>>();
        let pattern;
        if let Some(file_pattern) = file_pattern {
            pattern = format!("%{file_pattern}%");
            params.push(&pattern);
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_call_targets(&self, include_file_sources: bool) -> Result<HashSet<String>> {
        if include_file_sources {
            let mut stmt = self
                .conn
                .prepare("SELECT DISTINCT target_qualified FROM edges WHERE kind = 'CALLS'")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            return rows
                .collect::<std::result::Result<HashSet<_>, _>>()
                .map_err(Into::into);
        }
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT e.target_qualified FROM edges e \
             LEFT JOIN nodes n ON n.qualified_name = e.source_qualified \
             WHERE e.kind = 'CALLS' \
             AND (n.kind IS NULL OR n.kind != 'File')",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<HashSet<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_nodes(&self) -> Result<Vec<GraphNode>> {
        let mut stmt = self.conn.prepare("SELECT * FROM nodes")?;
        let rows = stmt.query_map([], node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_nodes_filtered(&self, exclude_files: bool) -> Result<Vec<GraphNode>> {
        if !exclude_files {
            return self.get_all_nodes();
        }
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM nodes WHERE kind != 'File'")?;
        let rows = stmt.query_map([], node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_edges(&self) -> Result<Vec<GraphEdge>> {
        let mut stmt = self.conn.prepare("SELECT * FROM edges")?;
        let rows = stmt.query_map([], edge_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}
