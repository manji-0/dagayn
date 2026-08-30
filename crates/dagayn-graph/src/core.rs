use crate::helpers::*;
use crate::*;

/// Upper bound for `graph.db-wal` after a checkpoint (256 MB).
///
/// Must match `WAL_SIZE_LIMIT_BYTES` in `dagayn/sqlite_tuning.py`: the Python
/// and Rust backends take turns writing the same WAL file.
const WAL_SIZE_LIMIT_BYTES: i64 = 256 * 1024 * 1024;

impl GraphStore {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref();
        if db_path != Path::new(":memory:")
            && let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|_| rusqlite::Error::InvalidPath(parent.to_path_buf()))?;
        }
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "cache_size", -64000)?;
        // mmap + WAL corrupts sqlite_master when another connection
        // checkpoints (CLI Python store overlapping this backend).
        conn.pragma_update(None, "mmap_size", 0)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        // WAL grows without bound otherwise: auto-checkpoint copies pages back
        // but never shrinks the file, and a long write transaction (full parse
        // of a large monorepo) blocks checkpointing entirely. A 514 MB graph
        // was seen with a 9.1 GB WAL, after which every read paged through it.
        // Keep in sync with WAL_SIZE_LIMIT_BYTES in dagayn/sqlite_tuning.py.
        conn.pragma_update(None, "journal_size_limit", WAL_SIZE_LIMIT_BYTES)?;
        let store = Self {
            conn,
            bulk_load_indexes_suspended: false,
        };
        store.init_schema()?;
        if store.schema_version()? < 1 {
            store.set_metadata("schema_version", "1")?;
        }
        store.run_migrations()?;
        Ok(store)
    }

    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES (?, ?)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_metadata(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM metadata WHERE key = ?", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    /// Absolute path for a stored (repo-relative) `file_path`.
    ///
    /// Returned unchanged when it is already absolute or the graph records no
    /// `repo_root`.
    pub fn resolve_file_path(&self, file_path: &str) -> Result<PathBuf> {
        let path = Path::new(file_path);
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }
        match self.get_metadata("repo_root")? {
            Some(repo_root) => Ok(Path::new(&repo_root).join(path)),
            None => Ok(path.to_path_buf()),
        }
    }

    pub fn schema_version(&self) -> Result<i64> {
        let version = match self.get_metadata("schema_version")? {
            Some(raw) => raw
                .parse::<i64>()
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            None => 1,
        };
        Ok(version)
    }

    pub fn commit(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT").or_else(|err| {
            if matches!(err, rusqlite::Error::SqliteFailure(_, Some(ref message)) if message.contains("no transaction is active"))
            {
                Ok(())
            } else {
                Err(err)
            }
        })?;
        Ok(())
    }

    pub fn rollback(&self) -> Result<()> {
        self.conn.execute_batch("ROLLBACK").or_else(|err| {
            if matches!(err, rusqlite::Error::SqliteFailure(_, Some(ref message)) if message.contains("no transaction is active"))
            {
                Ok(())
            } else {
                Err(err)
            }
        })?;
        Ok(())
    }

    pub fn remove_file_data(&mut self, file_path: &str) -> Result<()> {
        let tx = write_tx(&mut self.conn)?;
        remove_file_data_tx(&tx, file_path)?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_files_data(&mut self, file_paths: &[String]) -> Result<()> {
        let tx = write_tx(&mut self.conn)?;
        remove_files_data_tx(&tx, file_paths)?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("dagayn-{name}-{nonce}.db"))
    }

    /// `journal_size_limit` is per-connection, so it has to be read back on the
    /// same handle that opened the store. Without it a long write transaction
    /// leaves `graph.db-wal` at its peak size forever (9.1 GB was observed
    /// against a 514 MB graph).
    #[test]
    fn open_bounds_the_wal_size() {
        let db_path = temp_db_path("wal-limit");
        let store = GraphStore::open(&db_path).expect("open store");

        let limit: i64 = store
            .conn
            .query_row("PRAGMA journal_size_limit", [], |row| row.get(0))
            .expect("read journal_size_limit");
        assert_eq!(limit, WAL_SIZE_LIMIT_BYTES);

        let mode: String = store
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read journal_mode");
        assert_eq!(mode, "wal");

        drop(store);
        let _ = std::fs::remove_file(&db_path);
    }
}
