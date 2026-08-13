use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub(crate) fn compute_community_summaries(&mut self) -> Result<()> {
        let tx = write_tx(&mut self.conn)?;
        tx.execute("DELETE FROM community_summaries", [])?;

        let mut edge_counts: HashMap<String, i64> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT source_qualified, COUNT(*) FROM edges GROUP BY source_qualified",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (qualified, count) = row?;
                *edge_counts.entry(qualified).or_default() += count;
            }
        }
        {
            let mut stmt = tx.prepare(
                "SELECT target_qualified, COUNT(*) FROM edges GROUP BY target_qualified",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (qualified, count) = row?;
                *edge_counts.entry(qualified).or_default() += count;
            }
        }

        let mut nodes_by_comm: HashMap<i64, Vec<(String, i64)>> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT community_id, name, qualified_name FROM nodes \
                 WHERE community_id IS NOT NULL AND kind != 'File'",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (community_id, name, qualified_name) = row?;
                nodes_by_comm
                    .entry(community_id)
                    .or_default()
                    .push((name, *edge_counts.get(&qualified_name).unwrap_or(&0)));
            }
        }

        let mut files_by_comm: HashMap<i64, Vec<String>> = HashMap::new();
        let mut seen_files: HashMap<i64, HashSet<String>> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT community_id, file_path FROM nodes WHERE community_id IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (community_id, file_path) = row?;
                let seen = seen_files.entry(community_id).or_default();
                if seen.insert(file_path.clone()) {
                    files_by_comm
                        .entry(community_id)
                        .or_default()
                        .push(file_path);
                }
            }
        }

        let community_rows = {
            let mut stmt =
                tx.prepare("SELECT id, name, size, dominant_language FROM communities")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut insert = tx.prepare(
            "INSERT OR REPLACE INTO community_summaries \
             (community_id, name, purpose, key_symbols, size, dominant_language) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )?;
        for (community_id, name, size, dominant_language) in community_rows {
            let mut members = nodes_by_comm.remove(&community_id).unwrap_or_default();
            members.sort_by(|left, right| right.1.cmp(&left.1));
            let key_symbols = members
                .into_iter()
                .take(5)
                .map(|(name, _)| name)
                .collect::<Vec<_>>();
            let paths = files_by_comm
                .get(&community_id)
                .map(|paths| paths.iter().take(20).cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            let purpose = community_purpose(&paths);
            insert.execute(params![
                community_id,
                name,
                purpose,
                serde_json::to_string(&key_symbols)?,
                size,
                dominant_language.unwrap_or_default()
            ])?;
        }
        drop(insert);
        tx.commit()?;
        Ok(())
    }
}
