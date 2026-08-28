//! Node-side lookups that mirror the Python `GraphStore` query surface.

use crate::helpers::*;
use crate::*;

/// `(id, name, kind, params, return_type)` for nodes missing a signature.
pub type NodeSignatureRow = (i64, String, String, Option<String>, Option<String>);

impl GraphStore {
    pub fn get_node_by_id(&self, node_id: i64) -> Result<Option<GraphNode>> {
        self.conn
            .query_row("SELECT * FROM nodes WHERE id = ?", [node_id], node_from_row)
            .optional()
            .map_err(Into::into)
    }

    /// Nodes whose line span falls inside `[min_lines, max_lines]`, largest first.
    pub fn get_nodes_by_size(
        &self,
        min_lines: i64,
        max_lines: Option<i64>,
        kind: Option<&str>,
        file_path_pattern: Option<&str>,
        limit: i64,
    ) -> Result<Vec<GraphNode>> {
        let mut conditions = vec![
            "line_start IS NOT NULL".to_string(),
            "line_end IS NOT NULL".to_string(),
            "(line_end - line_start + 1) >= ?".to_string(),
        ];
        let mut params: Vec<SqlValue> = vec![SqlValue::Integer(min_lines)];

        if let Some(max_lines) = max_lines {
            conditions.push("(line_end - line_start + 1) <= ?".to_string());
            params.push(SqlValue::Integer(max_lines));
        }
        if let Some(kind) = kind.filter(|kind| !kind.is_empty()) {
            conditions.push("kind = ?".to_string());
            params.push(SqlValue::Text(kind.to_string()));
        }
        if let Some(pattern) = file_path_pattern.filter(|pattern| !pattern.is_empty()) {
            conditions.push("file_path LIKE ?".to_string());
            params.push(SqlValue::Text(format!("%{pattern}%")));
        }
        params.push(SqlValue::Integer(limit));

        let where_clause = conditions.join(" AND ");
        let sql = format!(
            "SELECT * FROM nodes WHERE {where_clause} \
             ORDER BY (line_end - line_start + 1) DESC LIMIT ?"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// `name -> definition count` over the requested kinds.
    ///
    /// Dead-code analysis uses this to tell an ambiguous bare name (many
    /// same-named definitions) from a unique one.
    pub fn count_nodes_by_name(
        &self,
        kinds: &[String],
        include_tests: bool,
    ) -> Result<HashMap<String, i64>> {
        let mut out = HashMap::new();
        if kinds.is_empty() {
            return Ok(out);
        }
        let placeholders = placeholder_list(kinds.len());
        let test_filter = if include_tests {
            ""
        } else {
            "AND is_test = 0 "
        };
        let sql = format!(
            "SELECT name, COUNT(*) FROM nodes \
             WHERE kind IN ({placeholders}) {test_filter}GROUP BY name"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(kinds), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (name, count) = row?;
            out.insert(name, count);
        }
        Ok(out)
    }

    /// Nodes declared inside `parent_name` under `name`, restricted to `kinds`.
    ///
    /// Used to resolve a method against its declared base class without
    /// guessing at qualified-name shapes.
    pub fn get_nodes_by_parent_and_name(
        &self,
        parent_name: &str,
        name: &str,
        kinds: &[String],
    ) -> Result<Vec<GraphNode>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = placeholder_list(kinds.len());
        let sql = format!(
            "SELECT * FROM nodes WHERE parent_name = ? AND name = ? AND kind IN ({placeholders})"
        );
        let mut params: Vec<&str> = vec![parent_name, name];
        params.extend(kinds.iter().map(String::as_str));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_nodes_without_signature(&self) -> Result<Vec<NodeSignatureRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, kind, params, return_type FROM nodes WHERE signature IS NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn update_node_signature(&self, node_id: i64, signature: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET signature = ? WHERE id = ?",
            params![signature, node_id],
        )?;
        Ok(())
    }

    /// Batch node fetch by exact qualified name, without key normalization.
    ///
    /// The subgraph and impact paths already hold graph-native qualified names,
    /// so they skip the normalization `get_nodes_by_qualified_names` performs.
    pub fn batch_get_nodes(&self, qualified_names: &[String]) -> Result<Vec<GraphNode>> {
        let mut out = Vec::new();
        if qualified_names.is_empty() {
            return Ok(out);
        }
        for chunk in qualified_names.chunks(450) {
            let placeholders = placeholder_list(chunk.len());
            let sql = format!("SELECT * FROM nodes WHERE qualified_name IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), node_from_row)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }
}
