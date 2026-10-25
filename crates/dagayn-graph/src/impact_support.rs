use crate::*;

impl GraphStore {
    pub(crate) fn test_node_json(
        &self,
        qualified_name: &str,
        indirect: bool,
    ) -> Result<Option<Value>> {
        self.conn
            .query_row(
                "SELECT name, qualified_name, file_path, kind FROM nodes \
                 WHERE qualified_name = ?",
                [qualified_name],
                |row| {
                    Ok(json!({
                        "name": row.get::<_, String>(0)?,
                        "qualified_name": row.get::<_, String>(1)?,
                        "file_path": row.get::<_, String>(2)?,
                        "kind": row.get::<_, String>(3)?,
                        "indirect": indirect,
                    }))
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn expand_file_keys(&self, file_paths: &[String]) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut seen = HashSet::new();
        for file_path in file_paths {
            for key in self.file_key_candidates(file_path)? {
                if seen.insert(key.clone()) {
                    keys.push(key);
                }
            }
        }
        Ok(keys)
    }
}
