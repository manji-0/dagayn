use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn store_file_nodes_edges(
        &mut self,
        file_path: &str,
        nodes: &[NodeInput],
        edges: &[EdgeInput],
        file_hash: &str,
        mtime_ns: i64,
    ) -> Result<()> {
        self.store_file_batch(&[(
            file_path.to_string(),
            nodes.to_vec(),
            edges.to_vec(),
            file_hash.to_string(),
            mtime_ns,
        )])
    }

    pub fn store_file_batch(&mut self, batch: &[FileBatchItem]) -> Result<()> {
        let suspend_indexes = !self.bulk_load_indexes_suspended;
        let tx = write_tx(&mut self.conn)?;
        store_file_batch_tx(&tx, batch, suspend_indexes)?;
        tx.commit()?;
        Ok(())
    }

    pub fn store_file_batch_json(&mut self, batch_json: &str) -> Result<()> {
        let compact: Vec<RawCompactFileBatchItem> = serde_json::from_str(batch_json)?;
        let suspend_indexes = !self.bulk_load_indexes_suspended;
        let tx = write_tx(&mut self.conn)?;
        store_raw_compact_file_batch_tx(&tx, &compact, suspend_indexes)?;
        tx.commit()?;
        Ok(())
    }

    pub fn begin_bulk_load(&mut self) -> Result<()> {
        if self.bulk_load_indexes_suspended {
            return Ok(());
        }
        let tx = write_tx(&mut self.conn)?;
        drop_graph_write_indexes(&tx)?;
        tx.commit()?;
        self.bulk_load_indexes_suspended = true;
        Ok(())
    }

    pub fn finish_bulk_load(&mut self) -> Result<()> {
        if !self.bulk_load_indexes_suspended {
            return Ok(());
        }
        let tx = write_tx(&mut self.conn)?;
        create_graph_write_indexes(&tx)?;
        tx.commit()?;
        self.bulk_load_indexes_suspended = false;
        Ok(())
    }

    pub fn get_all_files(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT file_path FROM nodes WHERE kind = 'File'")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_file_hashes(&self, file_paths: &[String]) -> Result<HashMap<String, String>> {
        if file_paths.is_empty() {
            return Ok(HashMap::new());
        }
        let mut out = HashMap::new();
        for chunk in file_paths.chunks(900) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT file_path, file_hash FROM nodes \
                 WHERE kind = 'File' AND file_path IN ({placeholders}) \
                   AND file_hash IS NOT NULL"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (file_path, file_hash) = row?;
                out.insert(file_path, file_hash);
            }
        }
        Ok(out)
    }

    pub fn get_file_meta_map(&self) -> Result<HashMap<String, (String, i64)>> {
        let mut out = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT file_path, file_hash, mtime_ns FROM nodes \
             WHERE file_hash IS NOT NULL AND file_hash != ''",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            ))
        })?;
        for row in rows {
            let (file_path, file_hash, mtime_ns) = row?;
            out.insert(file_path, (file_hash, mtime_ns));
        }
        Ok(out)
    }

    pub fn get_file_meta_for_files(
        &self,
        file_paths: &[String],
    ) -> Result<HashMap<String, (String, i64)>> {
        let mut out = HashMap::new();
        for chunk in file_paths.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT file_path, file_hash, mtime_ns FROM nodes \
                 WHERE file_hash IS NOT NULL AND file_hash != '' \
                   AND file_path IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ))
            })?;
            for row in rows {
                let (file_path, file_hash, mtime_ns) = row?;
                out.insert(file_path, (file_hash, mtime_ns));
            }
        }
        Ok(out)
    }

    pub fn update_file_mtime(&self, file_path: &str, mtime_ns: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET mtime_ns = ? WHERE file_path = ?",
            params![mtime_ns, file_path],
        )?;
        Ok(())
    }

    pub fn update_file_mtimes(&mut self, updates: &[(String, i64)]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let tx = write_tx(&mut self.conn)?;
        {
            let mut stmt = tx.prepare("UPDATE nodes SET mtime_ns = ? WHERE file_path = ?")?;
            for (file_path, mtime_ns) in updates {
                stmt.execute(params![mtime_ns, file_path])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
