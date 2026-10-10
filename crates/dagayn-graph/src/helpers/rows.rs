//! Row mapping, JSON columns, schema probes, and SQL placeholders.

use super::*;

pub(crate) fn extra_json(value: &Value) -> Result<String> {
    if value.is_null() || value.as_object().is_some_and(|object| object.is_empty()) {
        Ok("{}".to_string())
    } else {
        Ok(serde_json::to_string(value)?)
    }
}

pub(crate) fn push_text(params: &mut Vec<SqlValue>, value: &str) {
    params.push(SqlValue::Text(value.to_string()));
}

pub(crate) fn push_optional_text(params: &mut Vec<SqlValue>, value: Option<&str>) {
    match value {
        Some(value) => params.push(SqlValue::Text(value.to_string())),
        None => params.push(SqlValue::Null),
    }
}

/// `"?,?,?"` for an `IN (...)` clause of `count` bound parameters.
pub(crate) fn placeholder_list(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn value_placeholders(width: usize, rows: usize) -> String {
    let row = format!(
        "({})",
        std::iter::repeat_n("?", width)
            .collect::<Vec<_>>()
            .join(",")
    );
    std::iter::repeat_n(row, rows).collect::<Vec<_>>().join(",")
}

pub(crate) fn node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphNode> {
    let extra: Option<String> = row.get("extra")?;
    Ok(GraphNode {
        id: row.get("id")?,
        kind: row.get("kind")?,
        name: row.get("name")?,
        qualified_name: row.get("qualified_name")?,
        file_path: row.get("file_path")?,
        line_start: row.get("line_start")?,
        line_end: row.get("line_end")?,
        language: row
            .get::<_, Option<String>>("language")?
            .unwrap_or_default(),
        parent_name: row.get("parent_name")?,
        params: row.get("params")?,
        return_type: row.get("return_type")?,
        is_test: row.get::<_, i64>("is_test")? != 0,
        file_hash: row.get("file_hash")?,
        extra: parse_json_column(extra).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?,
        // Projections that do not select `signature` (and pre-v?? schemas that
        // lack the column) must not fail the whole row, so an unavailable
        // column reads as `None` -- the same guard Python's `_row_to_node` has.
        signature: row.get::<_, Option<String>>("signature").unwrap_or(None),
    })
}

pub(crate) fn edge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphEdge> {
    let extra: Option<String> = row.get("extra")?;
    Ok(GraphEdge {
        id: row.get("id")?,
        kind: row.get("kind")?,
        source_qualified: row.get("source_qualified")?,
        target_qualified: row.get("target_qualified")?,
        file_path: row.get("file_path")?,
        line: row.get("line")?,
        extra: parse_json_column(extra).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?,
        confidence: row.get::<_, Option<f64>>("confidence")?.unwrap_or(1.0),
        confidence_tier: ConfidenceTier::from_raw(
            row.get::<_, Option<String>>("confidence_tier")?.as_deref(),
        ),
    })
}

pub(crate) fn parse_json_column(raw: Option<String>) -> serde_json::Result<Value> {
    match raw {
        Some(raw) if !raw.is_empty() => serde_json::from_str(&raw),
        _ => Ok(Value::Object(Default::default())),
    }
}

pub(crate) fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?",
        [table],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}
