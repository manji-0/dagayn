//! Edge lookups that mirror the Python `GraphStore` query surface.
//!
//! These replace the raw `store._conn` SQL the Python refactor and analysis
//! modules used to issue directly, so both backends answer the same questions
//! through the same method names.

use crate::helpers::*;
use crate::*;

impl GraphStore {
    /// All edges of one `kind`.
    ///
    /// With `unresolved_target_only`, restricts to edges whose target is an
    /// `<unresolved:...>` placeholder -- the shape entry-point bridges take
    /// before a later pass resolves them.
    pub fn get_edges_by_kind(
        &self,
        kind: &str,
        unresolved_target_only: bool,
    ) -> Result<Vec<GraphEdge>> {
        let sql = if unresolved_target_only {
            "SELECT * FROM edges WHERE kind = ? AND target_qualified LIKE '<unresolved:%'"
        } else {
            "SELECT * FROM edges WHERE kind = ?"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([kind], edge_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Edges grouped by `source_qualified`, optionally filtered to `kinds`.
    pub fn get_edges_by_sources(
        &self,
        source_qns: &[String],
        kinds: &[String],
    ) -> Result<EdgeEndpointMap> {
        self.edges_by_endpoint_column("source_qualified", source_qns, kinds)
    }

    /// Edges grouped by `target_qualified`, optionally filtered to `kinds`.
    pub fn get_edges_by_targets(
        &self,
        target_qns: &[String],
        kinds: &[String],
    ) -> Result<EdgeEndpointMap> {
        self.edges_by_endpoint_column("target_qualified", target_qns, kinds)
    }

    fn edges_by_endpoint_column(
        &self,
        column: &str,
        qns: &[String],
        kinds: &[String],
    ) -> Result<EdgeEndpointMap> {
        let mut out: EdgeEndpointMap = HashMap::new();
        if qns.is_empty() {
            return Ok(out);
        }
        let mut unique = Vec::new();
        let mut seen_qn = HashSet::new();
        for qn in qns {
            if seen_qn.insert(qn.as_str()) {
                unique.push(qn.clone());
            }
        }
        for chunk in unique.chunks(450) {
            let placeholders = placeholder_list(chunk.len());
            let kind_filter = if kinds.is_empty() {
                String::new()
            } else {
                format!(" AND kind IN ({})", placeholder_list(kinds.len()))
            };
            let sql =
                format!("SELECT * FROM edges WHERE {column} IN ({placeholders}){kind_filter}");
            let mut params = chunk.iter().map(String::as_str).collect::<Vec<_>>();
            params.extend(kinds.iter().map(String::as_str));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok((row.get::<_, String>(column)?, edge_from_row(row)?))
            })?;
            for row in rows {
                let (key, edge) = row?;
                out.entry(key).or_default().push(edge);
            }
        }
        Ok(out)
    }

    /// Edges grouped by the normalized `target_name` column.
    ///
    /// With `qualified_only`, drops rows whose `target_qualified` is just the
    /// bare name -- those are the unqualified call edges the caller already
    /// handles separately.
    pub fn get_edges_by_target_names(
        &self,
        names: &[String],
        kind: &str,
        qualified_only: bool,
    ) -> Result<EdgeEndpointMap> {
        let mut out: EdgeEndpointMap = HashMap::new();
        if names.is_empty() {
            return Ok(out);
        }
        for chunk in names.chunks(450) {
            let placeholders = placeholder_list(chunk.len());
            let qualified_filter = if qualified_only {
                " AND target_qualified != target_name"
            } else {
                ""
            };
            let sql = format!(
                "SELECT * FROM edges \
                 WHERE target_name IN ({placeholders}) AND kind = ?{qualified_filter}"
            );
            let mut params = chunk.iter().map(String::as_str).collect::<Vec<_>>();
            params.push(kind);
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok((row.get::<_, String>("target_name")?, edge_from_row(row)?))
            })?;
            for row in rows {
                let (key, edge) = row?;
                out.entry(key).or_default().push(edge);
            }
        }
        Ok(out)
    }

    pub fn count_edges_by_target_name_prefix(&self, prefix: &str, kind: &str) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE kind = ? AND target_name LIKE ?",
                params![kind, format!("{prefix}%")],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Whether any `kind` edge points at `target_qualified`.
    pub fn has_edge_to_target(&self, target_qualified: &str, kind: &str) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM edges WHERE target_qualified = ? AND kind = ? LIMIT 1",
                params![target_qualified, kind],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Edges whose `target_name` matches an unqualified symbol name exactly.
    ///
    /// CALLS edges often store the bare callee name, so reverse call tracing
    /// has to look here when qualified-name lookup comes back empty.
    pub fn search_edges_by_target_name(&self, name: &str, kind: &str) -> Result<Vec<GraphEdge>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM edges WHERE target_name = ? AND kind = ?")?;
        let rows = stmt.query_map(params![name, kind], edge_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// `IMPORTS_FROM` edges resolving to `defining_file`.
    ///
    /// `IMPORTS_FROM.target_qualified` holds the resolved module file path, not
    /// a symbol qualified name, so the symbol is not part of the lookup.
    pub fn search_import_edges_for_symbol(&self, defining_file: &str) -> Result<Vec<GraphEdge>> {
        let keys = self.file_key_candidates(defining_file)?;
        let placeholders = placeholder_list(keys.len());
        let sql = format!(
            "SELECT * FROM edges \
             WHERE kind = 'IMPORTS_FROM' AND target_qualified IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&keys), edge_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// `target_qualified` of every edge sourced from `source_qns` (with repeats).
    pub fn get_outgoing_targets(&self, source_qns: &[String]) -> Result<Vec<String>> {
        self.endpoint_column_values("source_qualified", "target_qualified", source_qns)
    }

    /// `source_qualified` of every edge targeting `target_qns` (with repeats).
    pub fn get_incoming_sources(&self, target_qns: &[String]) -> Result<Vec<String>> {
        self.endpoint_column_values("target_qualified", "source_qualified", target_qns)
    }

    fn endpoint_column_values(
        &self,
        filter_column: &str,
        select_column: &str,
        qns: &[String],
    ) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if qns.is_empty() {
            return Ok(out);
        }
        for chunk in qns.chunks(450) {
            let placeholders = placeholder_list(chunk.len());
            let sql = format!(
                "SELECT {select_column} FROM edges WHERE {filter_column} IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// Edges whose source *and* target are both inside `qualified_names`.
    pub fn get_edges_among(&self, qualified_names: &HashSet<String>) -> Result<Vec<GraphEdge>> {
        if qualified_names.is_empty() {
            return Ok(Vec::new());
        }
        let qns = qualified_names.iter().cloned().collect::<Vec<_>>();
        let mut out = Vec::new();
        for chunk in qns.chunks(450) {
            let placeholders = placeholder_list(chunk.len());
            let sql = format!("SELECT * FROM edges WHERE source_qualified IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), edge_from_row)?;
            for row in rows {
                let edge = row?;
                if qualified_names.contains(&edge.target_qualified) {
                    out.push(edge);
                }
            }
        }
        Ok(out)
    }
}
