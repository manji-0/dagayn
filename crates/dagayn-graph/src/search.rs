use crate::helpers::*;
use crate::*;

impl GraphStore {
    pub fn rebuild_fts_index(&mut self) -> Result<i64> {
        let repo_root = self.get_metadata("repo_root")?.map(PathBuf::from);
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            r#"
            DROP TABLE IF EXISTS nodes_fts;
            CREATE VIRTUAL TABLE nodes_fts USING fts5(
                name, qualified_name, file_path, signature, identifier_tokens, doc_text,
                tokenize='porter unicode61'
            );
            "#,
        )?;
        let fts_rows = {
            let mut stmt = tx.prepare(
                "SELECT rowid AS node_rowid, kind, name, qualified_name, file_path, line_start, line_end, \
                 signature, extra FROM nodes",
            )?;
            let mapped = stmt.query_map([], |row| {
                let rowid: i64 = row.get("node_rowid")?;
                let kind: String = row.get("kind")?;
                let name: String = row.get("name")?;
                let qualified_name: String = row.get("qualified_name")?;
                let file_path: String = row.get("file_path")?;
                let line_start: Option<i64> = row.get("line_start")?;
                let line_end: Option<i64> = row.get("line_end")?;
                let signature: Option<String> = row.get("signature")?;
                let extra_raw: Option<String> = row.get("extra")?;
                Ok((
                    rowid,
                    kind,
                    name,
                    qualified_name,
                    file_path,
                    line_start,
                    line_end,
                    signature,
                    extra_raw,
                ))
            })?;
            let mut collected = Vec::new();
            for row in mapped {
                collected.push(row?);
            }
            collected
        };
        {
            let mut insert = tx.prepare(
                "INSERT INTO nodes_fts(rowid, name, qualified_name, file_path, signature, \
                 identifier_tokens, doc_text) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;
            for (
                rowid,
                kind,
                name,
                qualified_name,
                file_path,
                line_start,
                line_end,
                signature,
                extra_raw,
            ) in fts_rows
            {
                let extra = parse_json_column(extra_raw)?;
                let display_name = extra
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let identifier_tokens =
                    identifier_search_text([&name, &qualified_name, &file_path, display_name]);
                let source_excerpt = read_node_source_excerpt(
                    repo_root.as_deref(),
                    &kind,
                    &file_path,
                    line_start,
                    line_end,
                );
                let doc_text = [display_name, source_excerpt.as_str()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                let doc_text = segment_japanese_fts_text(&doc_text);
                insert.execute(params![
                    rowid,
                    name,
                    qualified_name,
                    file_path,
                    signature.unwrap_or_default(),
                    identifier_tokens,
                    doc_text
                ])?;
            }
        }
        let count = tx.query_row("SELECT count(*) FROM nodes_fts", [], |row| row.get(0))?;
        tx.commit()?;
        Ok(count)
    }

    pub fn compute_missing_signatures(&mut self) -> Result<i64> {
        let tx = self.conn.transaction()?;
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
