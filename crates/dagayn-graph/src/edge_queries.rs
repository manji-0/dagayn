use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn get_edges_by_source(&self, qualified_name: &str) -> Result<Vec<GraphEdge>> {
        self.get_edges_by_endpoint("source_qualified", qualified_name)
    }

    pub fn get_edges_by_target(&self, qualified_name: &str) -> Result<Vec<GraphEdge>> {
        self.get_edges_by_endpoint("target_qualified", qualified_name)
    }

    pub fn get_edges_by_endpoints(
        &self,
        qualified_names: &[String],
    ) -> Result<(EdgeEndpointMap, EdgeEndpointMap)> {
        let mut outgoing = qualified_names
            .iter()
            .map(|qn| (qn.clone(), Vec::new()))
            .collect::<HashMap<_, _>>();
        let mut incoming = qualified_names
            .iter()
            .map(|qn| (qn.clone(), Vec::new()))
            .collect::<HashMap<_, _>>();
        if qualified_names.is_empty() {
            return Ok((outgoing, incoming));
        }

        let mut keys = HashSet::new();
        let mut normalized_to_originals: HashMap<String, Vec<String>> = HashMap::new();
        for qn in qualified_names {
            keys.insert(qn.clone());
            let normalized = self.normalize_qualified_key(qn)?;
            keys.insert(normalized.clone());
            normalized_to_originals
                .entry(normalized)
                .or_default()
                .push(qn.clone());
        }

        let mut seen_out = qualified_names
            .iter()
            .map(|qn| (qn.clone(), HashSet::new()))
            .collect::<HashMap<_, _>>();
        let mut seen_in = qualified_names
            .iter()
            .map(|qn| (qn.clone(), HashSet::new()))
            .collect::<HashMap<_, _>>();
        let keys = keys.into_iter().collect::<Vec<_>>();
        for chunk in keys.chunks(225) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT * FROM edges \
                 WHERE source_qualified IN ({placeholders}) \
                 OR target_qualified IN ({placeholders})"
            );
            let mut params = Vec::with_capacity(chunk.len() * 2);
            params.extend(chunk.iter().cloned());
            params.extend(chunk.iter().cloned());
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), edge_from_row)?;
            for row in rows {
                let edge = row?;
                if let Some(source_originals) = normalized_to_originals.get(&edge.source_qualified)
                {
                    for original in source_originals {
                        if let Some(seen) = seen_out.get_mut(original)
                            && seen.insert(edge.id)
                        {
                            outgoing
                                .entry(original.clone())
                                .or_default()
                                .push(edge.clone());
                        }
                    }
                } else if outgoing.contains_key(&edge.source_qualified)
                    && let Some(seen) = seen_out.get_mut(&edge.source_qualified)
                    && seen.insert(edge.id)
                {
                    outgoing
                        .entry(edge.source_qualified.clone())
                        .or_default()
                        .push(edge.clone());
                }
                if let Some(target_originals) = normalized_to_originals.get(&edge.target_qualified)
                {
                    for original in target_originals {
                        if let Some(seen) = seen_in.get_mut(original)
                            && seen.insert(edge.id)
                        {
                            incoming
                                .entry(original.clone())
                                .or_default()
                                .push(edge.clone());
                        }
                    }
                } else if incoming.contains_key(&edge.target_qualified)
                    && let Some(seen) = seen_in.get_mut(&edge.target_qualified)
                    && seen.insert(edge.id)
                {
                    incoming
                        .entry(edge.target_qualified.clone())
                        .or_default()
                        .push(edge.clone());
                }
            }
        }
        Ok((outgoing, incoming))
    }

    pub fn get_direct_dependents(&self, file_paths: &[String]) -> Result<Vec<String>> {
        if file_paths.is_empty() {
            return Ok(Vec::new());
        }

        let mut dependents = HashSet::new();
        let mut fp_keys = Vec::new();
        let mut seen_keys = HashSet::new();
        for file_path in file_paths {
            for key in self.qualified_key_candidates(file_path)? {
                if seen_keys.insert(key.clone()) {
                    fp_keys.push(key);
                }
            }
        }

        for chunk in fp_keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT file_path FROM edges \
                 WHERE target_qualified IN ({placeholders}) AND kind = 'IMPORTS_FROM'"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                dependents.insert(row?);
            }
        }

        let file_keys = self.expand_file_keys(file_paths)?;
        let mut node_qns = Vec::new();
        for chunk in file_keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("SELECT qualified_name FROM nodes WHERE file_path IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                node_qns.push(row?);
            }
        }

        for chunk in node_qns.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT file_path FROM edges \
                 WHERE target_qualified IN ({placeholders}) \
                   AND kind IN ('CALLS', 'IMPORTS_FROM', 'INHERITS', 'IMPLEMENTS')"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                dependents.insert(row?);
            }
        }

        for file_path in file_paths {
            dependents.remove(file_path);
        }
        let mut out = dependents.into_iter().collect::<Vec<_>>();
        out.sort();
        Ok(out)
    }
}
