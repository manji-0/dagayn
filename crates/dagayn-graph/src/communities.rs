use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn store_communities_json(&mut self, communities_json: &str) -> Result<i64> {
        let communities: Vec<CommunityInput> = serde_json::from_str(communities_json)?;
        let tx = write_tx(&mut self.conn)?;
        tx.execute("DELETE FROM community_summaries", [])?;
        tx.execute("DELETE FROM communities", [])?;
        tx.execute("UPDATE nodes SET community_id = NULL", [])?;
        let mut insert = tx.prepare(
            "INSERT INTO communities \
             (name, level, cohesion, size, dominant_language, description) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )?;
        for community in &communities {
            insert.execute(params![
                community.name,
                community.level,
                community.cohesion,
                community.size,
                community.dominant_language,
                community.description
            ])?;
            let community_id = tx.last_insert_rowid();
            for chunk in community.members.chunks(450) {
                if chunk.is_empty() {
                    continue;
                }
                let placeholders = std::iter::repeat_n("?", chunk.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!(
                    "UPDATE nodes SET community_id = ? WHERE qualified_name IN ({placeholders})"
                );
                let mut params = Vec::with_capacity(chunk.len() + 1);
                params.push(rusqlite::types::Value::Integer(community_id));
                params.extend(chunk.iter().cloned().map(rusqlite::types::Value::Text));
                tx.execute(&sql, rusqlite::params_from_iter(params))?;
            }
        }
        drop(insert);
        tx.commit()?;
        Ok(communities.len() as i64)
    }

    pub fn get_communities_json(&self, sort_by: &str, min_size: i64) -> Result<String> {
        let sort_by = CommunitySortBy::from_raw(sort_by);
        let sql = format!(
            "SELECT * FROM communities WHERE size >= ? ORDER BY {} {}",
            sort_by.column(),
            sort_by.order()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([min_size], community_json_from_row)?;
        let mut communities = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        let community_ids = communities
            .iter()
            .filter_map(|community| community.get("id").and_then(Value::as_i64))
            .collect::<Vec<_>>();
        let members_by_community = self.get_community_member_qns_by_ids(&community_ids)?;
        for community in &mut communities {
            let id = community.get("id").and_then(Value::as_i64).unwrap_or(0);
            let members = members_by_community.get(&id).cloned().unwrap_or_default();
            let assigned_member_count = members.len() as i64;
            if let Some(obj) = community.as_object_mut() {
                obj.insert(
                    "assigned_member_count".to_string(),
                    Value::Number(assigned_member_count.into()),
                );
                obj.insert(
                    "members".to_string(),
                    Value::Array(
                        members
                            .into_iter()
                            .map(|member| Value::String(sanitize_name(&member)))
                            .collect(),
                    ),
                );
            }
        }
        serde_json::to_string(&communities).map_err(Into::into)
    }

    pub(crate) fn get_community_member_qns_by_ids(
        &self,
        community_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<String>>> {
        let mut out = HashMap::new();
        for chunk in community_ids.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT community_id, qualified_name FROM nodes \
                 WHERE community_id IN ({placeholders}) ORDER BY community_id"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (community_id, qualified_name) = row?;
                out.entry(community_id)
                    .or_insert_with(Vec::new)
                    .push(qualified_name);
            }
        }
        Ok(out)
    }

    /// `(id, name)` for every row in the `communities` table.
    ///
    /// Empty when the table does not exist yet (pre-v4 schemas).
    pub fn get_communities_list(&self) -> Result<Vec<(i64, String)>> {
        let Ok(mut stmt) = self.conn.prepare("SELECT id, name FROM communities") else {
            return Ok(Vec::new());
        };
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Qualified names of the nodes assigned to one community.
    pub fn get_community_member_qns(&self, community_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT qualified_name FROM nodes WHERE community_id = ?")?;
        let rows = stmt.query_map([community_id], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_community_member_qns(&self) -> Result<HashMap<i64, Vec<String>>> {
        let mut out = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT community_id, qualified_name FROM nodes \
             WHERE community_id IS NOT NULL ORDER BY community_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (community_id, qualified_name) = row?;
            out.entry(community_id)
                .or_insert_with(Vec::new)
                .push(qualified_name);
        }
        Ok(out)
    }

    pub fn update_community_stats(
        &mut self,
        community_id: i64,
        size: i64,
        cohesion: f64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE communities SET size = ?, cohesion = ? WHERE id = ?",
            params![size, cohesion, community_id],
        )?;
        Ok(())
    }

    pub fn delete_community(&mut self, community_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM communities WHERE id = ?", [community_id])?;
        Ok(())
    }

    pub fn delete_orphan_communities(&mut self) -> Result<i64> {
        let deleted = self.conn.execute(
            "DELETE FROM communities WHERE NOT EXISTS \
             (SELECT 1 FROM nodes n WHERE n.community_id = communities.id)",
            [],
        )?;
        Ok(deleted as i64)
    }

    pub fn affected_community_id_set(&self, file_paths: &[String]) -> Result<HashSet<i64>> {
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
        Ok(community_ids)
    }

    pub fn expand_neighbor_community_ids(&self, ids: &HashSet<i64>) -> Result<HashSet<i64>> {
        let mut expanded = ids.clone();
        if ids.is_empty() {
            return Ok(expanded);
        }
        let id_list: Vec<i64> = ids.iter().copied().collect();
        for chunk in id_list.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let outbound = format!(
                "SELECT DISTINCT n_tgt.community_id \
                 FROM edges e \
                 JOIN nodes n_src ON n_src.qualified_name = e.source_qualified \
                 JOIN nodes n_tgt ON n_tgt.qualified_name = e.target_qualified \
                 WHERE n_src.community_id IN ({placeholders}) \
                   AND n_tgt.community_id IS NOT NULL"
            );
            let inbound = format!(
                "SELECT DISTINCT n_src.community_id \
                 FROM edges e \
                 JOIN nodes n_src ON n_src.qualified_name = e.source_qualified \
                 JOIN nodes n_tgt ON n_tgt.qualified_name = e.target_qualified \
                 WHERE n_tgt.community_id IN ({placeholders}) \
                   AND n_src.community_id IS NOT NULL"
            );
            for sql in [outbound, inbound] {
                let mut stmt = self.conn.prepare(&sql)?;
                let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                    row.get::<_, i64>(0)
                })?;
                for row in rows {
                    expanded.insert(row?);
                }
            }
        }
        Ok(expanded)
    }

    pub fn community_region_ids(&self, changed_files: &[String]) -> Result<Option<Vec<i64>>> {
        let ids = self.affected_community_id_set(changed_files)?;
        if ids.is_empty() {
            return Ok(None);
        }
        let expanded = self.expand_neighbor_community_ids(&ids)?;
        let id_vec: Vec<i64> = expanded.iter().copied().collect();
        let region_count = self.count_nodes_in_community_ids(&id_vec)?;
        let total = self.count_non_file_nodes()?;
        if region_count == 0 || total == 0 || (region_count as f64) / (total as f64) > 0.5 {
            return Ok(None);
        }
        Ok(Some(id_vec))
    }

    pub(crate) fn community_region_qualified_names(
        &self,
        changed_files: &[String],
    ) -> Result<Option<HashSet<String>>> {
        let Some(id_vec) = self.community_region_ids(changed_files)? else {
            return Ok(None);
        };
        let members = self.get_community_member_qns_by_ids(&id_vec)?;
        let mut qns = HashSet::new();
        for names in members.values() {
            qns.extend(names.iter().cloned());
        }
        Ok(Some(qns))
    }

    pub fn replace_communities(
        &mut self,
        replace_ids: &[i64],
        communities: &[CommunityInput],
    ) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        for chunk in replace_ids.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let summary_sql =
                format!("DELETE FROM community_summaries WHERE community_id IN ({placeholders})");
            tx.execute(&summary_sql, rusqlite::params_from_iter(chunk))?;
            let clear_sql = format!(
                "UPDATE nodes SET community_id = NULL WHERE community_id IN ({placeholders})"
            );
            tx.execute(&clear_sql, rusqlite::params_from_iter(chunk))?;
            let delete_sql = format!("DELETE FROM communities WHERE id IN ({placeholders})");
            tx.execute(&delete_sql, rusqlite::params_from_iter(chunk))?;
        }
        {
            let mut insert = tx.prepare(
                "INSERT INTO communities \
                 (name, level, cohesion, size, dominant_language, description) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )?;
            for community in communities {
                insert.execute(params![
                    community.name,
                    community.level,
                    community.cohesion,
                    community.size,
                    community.dominant_language,
                    community.description
                ])?;
                let community_id = tx.last_insert_rowid();
                for chunk in community.members.chunks(450) {
                    if chunk.is_empty() {
                        continue;
                    }
                    let placeholders = std::iter::repeat_n("?", chunk.len())
                        .collect::<Vec<_>>()
                        .join(",");
                    let sql = format!(
                        "UPDATE nodes SET community_id = ? WHERE qualified_name IN ({placeholders})"
                    );
                    let mut params = Vec::with_capacity(chunk.len() + 1);
                    params.push(rusqlite::types::Value::Integer(community_id));
                    params.extend(chunk.iter().cloned().map(rusqlite::types::Value::Text));
                    tx.execute(&sql, rusqlite::params_from_iter(params))?;
                }
            }
        }
        tx.commit()?;
        Ok(communities.len() as i64)
    }

    pub(crate) fn get_test_targets_for_source(
        &self,
        source_qualified: &str,
    ) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT target_qualified FROM edges \
             WHERE source_qualified = ? AND kind = 'TESTED_BY'",
        )?;
        let rows = stmt.query_map([source_qualified], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}
