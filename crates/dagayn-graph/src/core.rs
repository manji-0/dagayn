use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref();
        if db_path != Path::new(":memory:") {
            if let Some(parent) = db_path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .map_err(|_| rusqlite::Error::InvalidPath(parent.to_path_buf()))?;
                }
            }
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
