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
