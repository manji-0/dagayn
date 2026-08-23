use crate::fts_sync::*;
use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn rebuild_fts_index(&mut self) -> Result<i64> {
        let repo_root = self.get_metadata("repo_root")?.map(PathBuf::from);
        rebuild_fts_index_tx(&self.conn, repo_root.as_deref())
    }

    /// Refresh FTS rows for *file_paths* only. Falls back to a full rebuild when
    /// `nodes_fts` has not been created yet so incremental updates cannot leave
    /// the rest of the corpus unindexed.
    pub fn sync_fts_for_file_paths(&mut self, file_paths: &[String]) -> Result<i64> {
        if file_paths.is_empty() {
            return Ok(0);
        }
        if !table_exists(&self.conn, "nodes_fts")? {
            return self.rebuild_fts_index();
        }
        let repo_root = self.get_metadata("repo_root")?.map(PathBuf::from);
        let tx = write_tx(&mut self.conn)?;
        let count =
            crate::fts_sync::sync_fts_for_file_paths_tx(&tx, file_paths, repo_root.as_deref())?;
        tx.commit()?;
        Ok(count)
    }

    pub fn compute_missing_signatures(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        tx.execute(
            "UPDATE nodes \
             SET signature = CASE \
               WHEN kind IN ('Function', 'Test') THEN \
                 substr('def ' || name || '(' || COALESCE(params, '') || ')' || \
                   CASE WHEN return_type IS NOT NULL THEN ' -> ' || return_type ELSE '' END, 1, 512) \
               WHEN kind = 'Class' THEN substr('class ' || name, 1, 512) \
               ELSE substr(name, 1, 512) \
             END \
             WHERE signature IS NULL",
            [],
        )?;
        let count = tx.query_row("SELECT changes()", [], |row| row.get::<_, i64>(0))?;
        tx.commit()?;
        Ok(count)
    }
}
