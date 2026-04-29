use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

const LATEST_VERSION: i64 = 9;

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    qualified_name TEXT NOT NULL UNIQUE,
    file_path TEXT NOT NULL,
    line_start INTEGER,
    line_end INTEGER,
    language TEXT,
    parent_name TEXT,
    params TEXT,
    return_type TEXT,
    modifiers TEXT,
    is_test INTEGER DEFAULT 0,
    file_hash TEXT,
    extra TEXT DEFAULT '{}',
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    source_qualified TEXT NOT NULL,
    target_qualified TEXT NOT NULL,
    file_path TEXT NOT NULL,
    line INTEGER DEFAULT 0,
    extra TEXT DEFAULT '{}',
    confidence REAL DEFAULT 1.0,
    confidence_tier TEXT DEFAULT 'EXTRACTED',
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(file_path);
CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_qualified ON nodes(qualified_name);
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_qualified);
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_qualified);
CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind);
CREATE INDEX IF NOT EXISTS idx_edges_target_kind ON edges(target_qualified, kind);
CREATE INDEX IF NOT EXISTS idx_edges_source_kind ON edges(source_qualified, kind);
CREATE INDEX IF NOT EXISTS idx_edges_file ON edges(file_path);
"#;

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("system clock error")]
    Clock,
}

pub type Result<T> = std::result::Result<T, GraphError>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodeInput {
    pub kind: String,
    pub name: String,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub parent_name: Option<String>,
    #[serde(default)]
    pub params: Option<String>,
    #[serde(default)]
    pub return_type: Option<String>,
    #[serde(default)]
    pub modifiers: Option<String>,
    #[serde(default)]
    pub is_test: bool,
    #[serde(default)]
    pub extra: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EdgeInput {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub file_path: String,
    #[serde(default)]
    pub line: i64,
    #[serde(default)]
    pub extra: Value,
}

#[derive(Clone, Debug)]
pub struct GraphNode {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub language: String,
    pub parent_name: Option<String>,
    pub params: Option<String>,
    pub return_type: Option<String>,
    pub is_test: bool,
    pub file_hash: Option<String>,
    pub extra: Value,
}

#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub id: i64,
    pub kind: String,
    pub source_qualified: String,
    pub target_qualified: String,
    pub file_path: String,
    pub line: i64,
    pub extra: Value,
    pub confidence: f64,
    pub confidence_tier: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FlowInput {
    pub name: String,
    pub entry_point_id: i64,
    pub depth: i64,
    pub node_count: i64,
    pub file_count: i64,
    pub criticality: f64,
    #[serde(default)]
    pub path: Vec<i64>,
}

pub struct GraphStore {
    conn: Connection,
}

pub type FileBatchItem = (String, Vec<NodeInput>, Vec<EdgeInput>, String);
pub type FlowEdgeData = (HashMap<String, Vec<String>>, HashSet<String>);

#[derive(Debug, Deserialize)]
struct CompactNodeInput(
    String,
    String,
    String,
    i64,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    Value,
);

#[derive(Debug, Deserialize)]
struct CompactEdgeInput(String, String, String, String, i64, Value);

type CompactFileBatchItem = (
    String,
    Vec<CompactNodeInput>,
    Vec<CompactEdgeInput>,
    String,
    i64,
);

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
        let store = Self { conn };
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
        let tx = self.conn.transaction()?;
        remove_file_data_tx(&tx, file_path)?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_files_data(&mut self, file_paths: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for file_path in file_paths {
            remove_file_data_tx(&tx, file_path)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn rebuild_fts_index(&mut self) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            r#"
            DROP TABLE IF EXISTS nodes_fts;
            CREATE VIRTUAL TABLE nodes_fts USING fts5(
                name, qualified_name, file_path, signature,
                content='nodes', content_rowid='rowid',
                tokenize='porter unicode61'
            );
            INSERT INTO nodes_fts(nodes_fts) VALUES('rebuild');
            "#,
        )?;
        let count = tx.query_row("SELECT count(*) FROM nodes_fts", [], |row| row.get(0))?;
        tx.commit()?;
        Ok(count)
    }

    pub fn compute_missing_signatures(&mut self) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT id, name, kind, params, return_type FROM nodes WHERE signature IS NULL",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        {
            let mut update = tx.prepare("UPDATE nodes SET signature = ? WHERE id = ?")?;
            for (node_id, name, kind, params, return_type) in &rows {
                let mut signature = if kind == "Function" || kind == "Test" {
                    let mut sig = format!("def {}({})", name, params.as_deref().unwrap_or(""));
                    if let Some(return_type) = return_type {
                        sig.push_str(" -> ");
                        sig.push_str(return_type);
                    }
                    sig
                } else if kind == "Class" {
                    format!("class {name}")
                } else {
                    name.clone()
                };
                if signature.chars().count() > 512 {
                    signature = signature.chars().take(512).collect();
                }
                update.execute(params![signature, node_id])?;
            }
        }

        let count = rows.len() as i64;
        tx.commit()?;
        Ok(count)
    }

    pub fn resolve_markdown_artifact_refs(&mut self) -> Result<(i64, i64)> {
        let tx = self.conn.transaction()?;
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT id, extra FROM edges \
                 WHERE kind='CROSS_ARTIFACT' \
                   AND extra LIKE '%unresolved_target_name%'",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        let mut resolved = 0_i64;
        let mut dropped = 0_i64;
        for (edge_id, raw_extra) in rows {
            let Ok(mut extra) = serde_json::from_str::<Value>(&raw_extra) else {
                continue;
            };
            let Some(extra_obj) = extra.as_object_mut() else {
                continue;
            };
            let Some(sym) = extra_obj
                .get("unresolved_target_name")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                continue;
            };

            let matches = {
                let mut stmt = tx.prepare(
                    "SELECT qualified_name, language \
                     FROM nodes \
                     WHERE name = ? AND language != 'markdown' \
                     LIMIT 2",
                )?;
                let matches = stmt
                    .query_map([sym], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                matches
            };

            if matches.len() == 1 {
                let (target, language) = &matches[0];
                extra_obj.remove("unresolved_target_name");
                extra_obj.insert(
                    "target_language".to_string(),
                    Value::String(language.clone().unwrap_or_else(|| "unknown".to_string())),
                );
                extra_obj.insert("confidence".to_string(), Value::from(0.8));
                extra_obj.insert(
                    "confidence_tier".to_string(),
                    Value::String("HIGH".to_string()),
                );
                tx.execute(
                    "UPDATE edges \
                     SET target_qualified = ?, extra = ?, confidence = 0.8, confidence_tier = 'HIGH' \
                     WHERE id = ?",
                    params![target, serde_json::to_string(&extra)?, edge_id],
                )?;
                resolved += 1;
            } else {
                tx.execute("DELETE FROM edges WHERE id = ?", [edge_id])?;
                dropped += 1;
            }
        }

        tx.commit()?;
        Ok((resolved, dropped))
    }

    pub fn compute_summaries(&mut self) -> Result<()> {
        match self.compute_community_summaries() {
            Ok(()) | Err(GraphError::Sqlite(_)) => {}
            Err(err) => return Err(err),
        }
        match self.compute_flow_snapshots() {
            Ok(()) | Err(GraphError::Sqlite(_)) => {}
            Err(err) => return Err(err),
        }
        match self.compute_risk_index() {
            Ok(()) | Err(GraphError::Sqlite(_)) => {}
            Err(err) => return Err(err),
        }
        Ok(())
    }

    fn compute_community_summaries(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
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

    fn compute_flow_snapshots(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM flow_snapshots", [])?;

        let flow_rows = {
            let mut stmt = tx.prepare(
                "SELECT id, name, entry_point_id, criticality, node_count, \
                 file_count, path_json FROM flows",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut needed_ids: HashSet<i64> = HashSet::new();
        let mut parsed_paths = Vec::with_capacity(flow_rows.len());
        for (_, _, entry_point_id, _, _, _, path_json) in &flow_rows {
            needed_ids.insert(*entry_point_id);
            let path_ids = if path_json.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str::<Vec<i64>>(path_json)?
            };
            for node_id in path_ids.iter().skip(1).take(3) {
                needed_ids.insert(*node_id);
            }
            if let Some(last) = path_ids.last() {
                needed_ids.insert(*last);
            }
            parsed_paths.push(path_ids);
        }

        let mut id_to_name: HashMap<i64, String> = HashMap::new();
        let id_list = needed_ids.into_iter().collect::<Vec<_>>();
        for chunk in id_list.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id, qualified_name FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = tx.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (node_id, qualified_name) = row?;
                id_to_name.insert(node_id, qualified_name);
            }
        }

        let mut insert = tx.prepare(
            "INSERT OR REPLACE INTO flow_snapshots \
             (flow_id, name, entry_point, critical_path, criticality, node_count, file_count) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )?;
        for ((flow_id, name, entry_point_id, criticality, node_count, file_count, _), path_ids) in
            flow_rows.into_iter().zip(parsed_paths)
        {
            let entry_point = id_to_name
                .get(&entry_point_id)
                .cloned()
                .unwrap_or_else(|| entry_point_id.to_string());
            let mut critical_path = Vec::new();
            if !path_ids.is_empty() {
                critical_path.push(entry_point.clone());
                if path_ids.len() > 2 {
                    for node_id in path_ids.iter().skip(1).take(3) {
                        if let Some(name) = id_to_name.get(node_id) {
                            critical_path.push(name.clone());
                        }
                    }
                }
                if path_ids.len() > 1 {
                    if let Some(last) = path_ids.last().and_then(|node_id| id_to_name.get(node_id))
                    {
                        if !critical_path.contains(last) {
                            critical_path.push(last.clone());
                        }
                    }
                }
            }
            insert.execute(params![
                flow_id,
                name,
                entry_point,
                serde_json::to_string(&critical_path)?,
                criticality,
                node_count,
                file_count
            ])?;
        }
        drop(insert);
        tx.commit()?;
        Ok(())
    }

    fn compute_risk_index(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM risk_index", [])?;

        let mut caller_counts: HashMap<String, i64> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT target_qualified, COUNT(*) FROM edges \
                 WHERE kind = 'CALLS' GROUP BY target_qualified",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (qualified_name, count) = row?;
                caller_counts.insert(qualified_name, count);
            }
        }

        let mut tested_counts: HashMap<String, i64> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT source_qualified, COUNT(*) FROM edges \
                 WHERE kind = 'TESTED_BY' GROUP BY source_qualified",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (qualified_name, count) = row?;
                tested_counts.insert(qualified_name, count);
            }
        }

        let risk_nodes = {
            let mut stmt = tx.prepare(
                "SELECT id, qualified_name, name FROM nodes \
                 WHERE kind IN ('Function', 'Class', 'Test')",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let security_keywords = [
            "auth",
            "login",
            "password",
            "token",
            "session",
            "crypt",
            "secret",
            "credential",
            "permission",
            "sql",
            "execute",
        ];
        let mut insert = tx.prepare(
            "INSERT OR REPLACE INTO risk_index \
             (node_id, qualified_name, risk_score, caller_count, test_coverage, \
              security_relevant, last_computed) \
             VALUES (?, ?, ?, ?, ?, ?, datetime('now'))",
        )?;
        for (node_id, qualified_name, name) in risk_nodes {
            let caller_count = *caller_counts.get(&qualified_name).unwrap_or(&0);
            let tested = *tested_counts.get(&qualified_name).unwrap_or(&0);
            let coverage = if tested > 0 { "tested" } else { "untested" };
            let name_lower = name.to_lowercase();
            let security_relevant = security_keywords
                .iter()
                .any(|keyword| name_lower.contains(keyword));
            let mut risk = 0.0_f64;
            if caller_count > 10 {
                risk += 0.3;
            } else if caller_count > 3 {
                risk += 0.15;
            }
            if coverage == "untested" {
                risk += 0.3;
            }
            if security_relevant {
                risk += 0.4;
            }
            insert.execute(params![
                node_id,
                qualified_name,
                risk.min(1.0),
                caller_count,
                coverage,
                if security_relevant { 1 } else { 0 }
            ])?;
        }
        drop(insert);
        tx.commit()?;
        Ok(())
    }

    pub fn store_file_nodes_edges(
        &mut self,
        file_path: &str,
        nodes: &[NodeInput],
        edges: &[EdgeInput],
        file_hash: &str,
    ) -> Result<()> {
        self.store_file_batch(&[(
            file_path.to_string(),
            nodes.to_vec(),
            edges.to_vec(),
            file_hash.to_string(),
        )])
    }

    pub fn store_file_batch(&mut self, batch: &[FileBatchItem]) -> Result<()> {
        let tx = self.conn.transaction()?;
        store_file_batch_tx(&tx, batch)?;
        tx.commit()?;
        Ok(())
    }

    pub fn store_file_batch_json(&mut self, batch_json: &str) -> Result<()> {
        let compact: Vec<CompactFileBatchItem> = serde_json::from_str(batch_json)?;
        let batch = compact
            .into_iter()
            .map(|(file_path, nodes, edges, file_hash, _mtime_ns)| {
                (
                    file_path,
                    nodes.into_iter().map(NodeInput::from).collect(),
                    edges.into_iter().map(EdgeInput::from).collect(),
                    file_hash,
                )
            })
            .collect::<Vec<_>>();
        self.store_file_batch(&batch)
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

    pub fn get_node(&self, qualified_name: &str) -> Result<Option<GraphNode>> {
        for key in self.qualified_key_candidates(qualified_name)? {
            let row = self
                .conn
                .query_row(
                    "SELECT * FROM nodes WHERE qualified_name = ?",
                    [key],
                    node_from_row,
                )
                .optional()?;
            if row.is_some() {
                return Ok(row);
            }
        }
        Ok(None)
    }

    pub fn get_nodes_by_file(&self, file_path: &str) -> Result<Vec<GraphNode>> {
        let mut seen = std::collections::HashSet::<i64>::new();
        let mut nodes = Vec::new();
        for key in self.file_key_candidates(file_path)? {
            let mut stmt = self
                .conn
                .prepare("SELECT * FROM nodes WHERE file_path = ?")?;
            let rows = stmt.query_map([key], node_from_row)?;
            for row in rows {
                let node = row?;
                if seen.insert(node.id) {
                    nodes.push(node);
                }
            }
        }
        Ok(nodes)
    }

    pub fn get_nodes_by_kind(
        &self,
        kinds: &[String],
        file_pattern: Option<&str>,
    ) -> Result<Vec<GraphNode>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", kinds.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = if file_pattern.is_some() {
            format!("SELECT * FROM nodes WHERE kind IN ({placeholders}) AND file_path LIKE ?")
        } else {
            format!("SELECT * FROM nodes WHERE kind IN ({placeholders})")
        };
        let mut params = kinds.iter().map(String::as_str).collect::<Vec<_>>();
        let pattern;
        if let Some(file_pattern) = file_pattern {
            pattern = format!("%{file_pattern}%");
            params.push(&pattern);
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_call_targets(&self, include_file_sources: bool) -> Result<HashSet<String>> {
        if include_file_sources {
            let mut stmt = self
                .conn
                .prepare("SELECT DISTINCT target_qualified FROM edges WHERE kind = 'CALLS'")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            return rows
                .collect::<std::result::Result<HashSet<_>, _>>()
                .map_err(Into::into);
        }
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT e.target_qualified FROM edges e \
             LEFT JOIN nodes n ON n.qualified_name = e.source_qualified \
             WHERE e.kind = 'CALLS' \
             AND (n.kind IS NULL OR n.kind != 'File')",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<HashSet<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_nodes(&self) -> Result<Vec<GraphNode>> {
        let mut stmt = self.conn.prepare("SELECT * FROM nodes")?;
        let rows = stmt.query_map([], node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_flow_edge_data(&self) -> Result<FlowEdgeData> {
        let mut calls_out: HashMap<String, Vec<String>> = HashMap::new();
        let mut has_tested_by: HashSet<String> = HashSet::new();
        let mut stmt = self.conn.prepare(
            "SELECT kind, source_qualified, target_qualified FROM edges \
             WHERE kind IN ('CALLS', 'TESTED_BY')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (kind, source, target) = row?;
            if kind == "CALLS" {
                calls_out.entry(source).or_default().push(target);
            } else {
                has_tested_by.insert(target);
            }
        }
        Ok((calls_out, has_tested_by))
    }

    pub fn store_flows(&mut self, flows: &[FlowInput]) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM flow_snapshots", [])?;
        tx.execute("DELETE FROM flow_memberships", [])?;
        tx.execute("DELETE FROM flows", [])?;
        store_flows_tx(&tx, flows)?;
        tx.commit()?;
        Ok(flows.len() as i64)
    }

    pub fn store_flows_json(&mut self, flows_json: &str) -> Result<i64> {
        let flows: Vec<FlowInput> = serde_json::from_str(flows_json)?;
        self.store_flows(&flows)
    }

    pub fn get_edges_by_source(&self, qualified_name: &str) -> Result<Vec<GraphEdge>> {
        self.get_edges_by_endpoint("source_qualified", qualified_name)
    }

    pub fn get_edges_by_target(&self, qualified_name: &str) -> Result<Vec<GraphEdge>> {
        self.get_edges_by_endpoint("target_qualified", qualified_name)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        Ok(())
    }

    fn run_migrations(&self) -> Result<()> {
        let current = self.schema_version()?;
        if current >= LATEST_VERSION {
            return Ok(());
        }
        for version in (current + 1)..=LATEST_VERSION {
            match version {
                2 => self.migrate_v2()?,
                3 => self.migrate_v3()?,
                4 => self.migrate_v4()?,
                5 => self.migrate_v5()?,
                6 => self.migrate_v6()?,
                7 => self.migrate_v7()?,
                8 => self.migrate_v8()?,
                9 => self.migrate_v9()?,
                _ => {}
            }
            self.set_metadata("schema_version", &version.to_string())?;
        }
        Ok(())
    }

    fn migrate_v2(&self) -> Result<()> {
        if !has_column(&self.conn, "nodes", "signature")? {
            self.conn
                .execute("ALTER TABLE nodes ADD COLUMN signature TEXT", [])?;
        }
        Ok(())
    }

    fn migrate_v3(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS flows (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                entry_point_id INTEGER NOT NULL,
                depth INTEGER NOT NULL,
                node_count INTEGER NOT NULL,
                file_count INTEGER NOT NULL,
                criticality REAL NOT NULL DEFAULT 0.0,
                path_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS flow_memberships (
                flow_id INTEGER NOT NULL,
                node_id INTEGER NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (flow_id, node_id)
            );
            CREATE INDEX IF NOT EXISTS idx_flows_criticality ON flows(criticality DESC);
            CREATE INDEX IF NOT EXISTS idx_flows_entry ON flows(entry_point_id);
            CREATE INDEX IF NOT EXISTS idx_flow_memberships_node ON flow_memberships(node_id);
            "#,
        )?;
        Ok(())
    }

    fn migrate_v4(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS communities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                level INTEGER NOT NULL DEFAULT 0,
                parent_id INTEGER,
                cohesion REAL NOT NULL DEFAULT 0.0,
                size INTEGER NOT NULL DEFAULT 0,
                dominant_language TEXT,
                description TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "#,
        )?;
        if !has_column(&self.conn, "nodes", "community_id")? {
            self.conn
                .execute("ALTER TABLE nodes ADD COLUMN community_id INTEGER", [])?;
        }
        self.conn.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_nodes_community ON nodes(community_id);
            CREATE INDEX IF NOT EXISTS idx_communities_parent ON communities(parent_id);
            CREATE INDEX IF NOT EXISTS idx_communities_cohesion ON communities(cohesion DESC);
            "#,
        )?;
        Ok(())
    }

    fn migrate_v5(&self) -> Result<()> {
        if !table_exists(&self.conn, "nodes_fts")? {
            self.conn.execute(
                r#"
                CREATE VIRTUAL TABLE nodes_fts USING fts5(
                    name, qualified_name, file_path, signature,
                    content='nodes', content_rowid='rowid',
                    tokenize='porter unicode61'
                )
                "#,
                [],
            )?;
        }
        Ok(())
    }

    fn migrate_v6(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS community_summaries (
                community_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                purpose TEXT DEFAULT '',
                key_symbols TEXT DEFAULT '[]',
                risk TEXT DEFAULT 'unknown',
                size INTEGER DEFAULT 0,
                dominant_language TEXT DEFAULT '',
                FOREIGN KEY (community_id) REFERENCES communities(id)
            );
            CREATE TABLE IF NOT EXISTS flow_snapshots (
                flow_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                entry_point TEXT NOT NULL,
                critical_path TEXT DEFAULT '[]',
                criticality REAL DEFAULT 0.0,
                node_count INTEGER DEFAULT 0,
                file_count INTEGER DEFAULT 0,
                FOREIGN KEY (flow_id) REFERENCES flows(id)
            );
            CREATE TABLE IF NOT EXISTS risk_index (
                node_id INTEGER PRIMARY KEY,
                qualified_name TEXT NOT NULL,
                risk_score REAL DEFAULT 0.0,
                caller_count INTEGER DEFAULT 0,
                test_coverage TEXT DEFAULT 'unknown',
                security_relevant INTEGER DEFAULT 0,
                last_computed TEXT DEFAULT '',
                FOREIGN KEY (node_id) REFERENCES nodes(id)
            );
            CREATE INDEX IF NOT EXISTS idx_risk_index_score ON risk_index(risk_score DESC);
            "#,
        )?;
        Ok(())
    }

    fn migrate_v7(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_edges_target_kind ON edges(target_qualified, kind);
            CREATE INDEX IF NOT EXISTS idx_edges_source_kind ON edges(source_qualified, kind);
            "#,
        )?;
        Ok(())
    }

    fn migrate_v8(&self) -> Result<()> {
        self.conn.execute(
            r#"
            CREATE INDEX IF NOT EXISTS idx_edges_composite
            ON edges(kind, source_qualified, target_qualified, file_path, line)
            "#,
            [],
        )?;
        Ok(())
    }

    fn migrate_v9(&self) -> Result<()> {
        if !has_column(&self.conn, "edges", "confidence")? {
            self.conn.execute(
                "ALTER TABLE edges ADD COLUMN confidence REAL DEFAULT 1.0",
                [],
            )?;
        }
        if !has_column(&self.conn, "edges", "confidence_tier")? {
            self.conn.execute(
                "ALTER TABLE edges ADD COLUMN confidence_tier TEXT DEFAULT 'EXTRACTED'",
                [],
            )?;
        }
        Ok(())
    }

    fn get_edges_by_endpoint(&self, column: &str, qualified_name: &str) -> Result<Vec<GraphEdge>> {
        let mut seen = std::collections::HashSet::<i64>::new();
        let mut edges = Vec::new();
        let sql = format!("SELECT * FROM edges WHERE {column} = ?");
        for key in self.qualified_key_candidates(qualified_name)? {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([key], edge_from_row)?;
            for row in rows {
                let edge = row?;
                if seen.insert(edge.id) {
                    edges.push(edge);
                }
            }
        }
        Ok(edges)
    }

    fn file_key_candidates(&self, file_path: &str) -> Result<Vec<String>> {
        let normalized = self.normalize_file_path_key(file_path)?;
        if normalized == file_path {
            Ok(vec![file_path.to_string()])
        } else {
            Ok(vec![file_path.to_string(), normalized])
        }
    }

    fn qualified_key_candidates(&self, qualified_name: &str) -> Result<Vec<String>> {
        let normalized = self.normalize_qualified_key(qualified_name)?;
        if normalized == qualified_name {
            Ok(vec![qualified_name.to_string()])
        } else {
            Ok(vec![qualified_name.to_string(), normalized])
        }
    }

    fn normalize_qualified_key(&self, qualified_name: &str) -> Result<String> {
        if let Some((file_path, rest)) = qualified_name.split_once("::") {
            Ok(format!(
                "{}::{rest}",
                self.normalize_file_path_key(file_path)?
            ))
        } else {
            self.normalize_file_path_key(qualified_name)
        }
    }

    fn normalize_file_path_key(&self, file_path: &str) -> Result<String> {
        let path = Path::new(file_path);
        if !path.is_absolute() {
            return Ok(file_path.to_string());
        }
        let Some(repo_root) = self.get_metadata("repo_root")? else {
            return Ok(file_path.to_string());
        };
        let repo_root = Path::new(&repo_root);
        if let Ok(rel) = path.strip_prefix(repo_root) {
            return Ok(rel.to_string_lossy().to_string());
        }
        if let (Ok(path), Ok(repo_root)) = (path.canonicalize(), repo_root.canonicalize()) {
            if let Ok(rel) = path.strip_prefix(repo_root) {
                return Ok(rel.to_string_lossy().to_string());
            }
        }
        Ok(file_path.to_string())
    }
}

fn now_seconds() -> Result<f64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| GraphError::Clock)?;
    Ok(duration.as_secs_f64())
}

impl From<CompactNodeInput> for NodeInput {
    fn from(value: CompactNodeInput) -> Self {
        Self {
            kind: value.0,
            name: value.1,
            file_path: value.2,
            line_start: value.3,
            line_end: value.4,
            language: value.5,
            parent_name: value.6,
            params: value.7,
            return_type: value.8,
            modifiers: value.9,
            is_test: value.10,
            extra: value.11,
        }
    }
}

impl From<CompactEdgeInput> for EdgeInput {
    fn from(value: CompactEdgeInput) -> Self {
        Self {
            kind: value.0,
            source: value.1,
            target: value.2,
            file_path: value.3,
            line: value.4,
            extra: value.5,
        }
    }
}

fn make_qualified(node: &NodeInput) -> String {
    if node.kind == "File" {
        node.file_path.clone()
    } else if let Some(parent) = &node.parent_name {
        format!("{}::{}.{}", node.file_path, parent, node.name)
    } else {
        format!("{}::{}", node.file_path, node.name)
    }
}

fn remove_file_data_tx(tx: &Transaction<'_>, file_path: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM risk_index WHERE node_id IN (SELECT id FROM nodes WHERE file_path = ?)",
        [file_path],
    )?;
    tx.execute("DELETE FROM edges WHERE file_path = ?", [file_path])?;
    tx.execute("DELETE FROM nodes WHERE file_path = ?", [file_path])?;
    Ok(())
}

fn store_flows_tx(tx: &Transaction<'_>, flows: &[FlowInput]) -> Result<()> {
    let mut insert_flow = tx.prepare(
        "INSERT INTO flows \
         (name, entry_point_id, depth, node_count, file_count, criticality, path_json) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )?;
    let mut insert_membership = tx.prepare(
        "INSERT OR IGNORE INTO flow_memberships (flow_id, node_id, position) \
         VALUES (?, ?, ?)",
    )?;
    for flow in flows {
        insert_flow.execute(params![
            flow.name,
            flow.entry_point_id,
            flow.depth,
            flow.node_count,
            flow.file_count,
            flow.criticality,
            serde_json::to_string(&flow.path)?,
        ])?;
        let flow_id = tx.last_insert_rowid();
        for (position, node_id) in flow.path.iter().enumerate() {
            insert_membership.execute(params![flow_id, node_id, position as i64])?;
        }
    }
    Ok(())
}

fn store_file_batch_tx(tx: &Transaction<'_>, batch: &[FileBatchItem]) -> Result<()> {
    let now = now_seconds()?;
    let mut delete_risk = tx.prepare(
        "DELETE FROM risk_index WHERE node_id IN (SELECT id FROM nodes WHERE file_path = ?)",
    )?;
    let mut delete_edges = tx.prepare("DELETE FROM edges WHERE file_path = ?")?;
    let mut delete_nodes = tx.prepare("DELETE FROM nodes WHERE file_path = ?")?;
    let mut insert_node = tx.prepare(
        r#"
        INSERT INTO nodes
            (kind, name, qualified_name, file_path, line_start, line_end,
             language, parent_name, params, return_type, modifiers, is_test,
             file_hash, extra, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(qualified_name) DO UPDATE SET
            kind=excluded.kind, name=excluded.name,
            file_path=excluded.file_path, line_start=excluded.line_start,
            line_end=excluded.line_end, language=excluded.language,
            parent_name=excluded.parent_name, params=excluded.params,
            return_type=excluded.return_type, modifiers=excluded.modifiers,
            is_test=excluded.is_test, file_hash=excluded.file_hash,
            extra=excluded.extra, updated_at=excluded.updated_at
        "#,
    )?;
    let mut insert_edge = tx.prepare(
        r#"
        INSERT INTO edges
            (kind, source_qualified, target_qualified, file_path, line, extra,
             confidence, confidence_tier, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )?;
    let mut seen_edges = HashSet::new();

    for (file_path, nodes, edges, file_hash) in batch {
        delete_risk.execute([file_path])?;
        delete_edges.execute([file_path])?;
        delete_nodes.execute([file_path])?;

        for node in nodes {
            let qualified = make_qualified(node);
            let extra = if node.extra.is_null() {
                "{}".to_string()
            } else {
                serde_json::to_string(&node.extra)?
            };
            insert_node.execute(params![
                node.kind,
                node.name,
                qualified,
                node.file_path,
                node.line_start,
                node.line_end,
                node.language,
                node.parent_name,
                node.params,
                node.return_type,
                node.modifiers,
                i64::from(node.is_test),
                file_hash,
                extra,
                now,
            ])?;
        }

        for edge in edges {
            let key = (
                edge.kind.as_str(),
                edge.source.as_str(),
                edge.target.as_str(),
                edge.file_path.as_str(),
                edge.line,
            );
            if !seen_edges.insert(key) {
                continue;
            }
            let extra = if edge.extra.is_null() {
                Value::Object(Default::default())
            } else {
                edge.extra.clone()
            };
            let confidence = extra
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let confidence_tier = extra
                .get("confidence_tier")
                .and_then(Value::as_str)
                .unwrap_or("EXTRACTED");
            let extra_json = serde_json::to_string(&extra)?;
            insert_edge.execute(params![
                edge.kind,
                edge.source,
                edge.target,
                edge.file_path,
                edge.line,
                extra_json,
                confidence,
                confidence_tier,
                now,
            ])?;
        }
    }
    Ok(())
}

fn node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphNode> {
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
    })
}

fn edge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphEdge> {
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
        confidence_tier: row
            .get::<_, Option<String>>("confidence_tier")?
            .unwrap_or_else(|| "EXTRACTED".to_string()),
    })
}

fn parse_json_column(raw: Option<String>) -> serde_json::Result<Value> {
    match raw {
        Some(raw) if !raw.is_empty() => serde_json::from_str(&raw),
        _ => Ok(Value::Object(Default::default())),
    }
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?",
        [table],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn common_prefix(values: &[String]) -> String {
    let Some((first, rest)) = values.split_first() else {
        return String::new();
    };
    let mut prefix = first.clone();
    for value in rest {
        while !value.starts_with(&prefix) {
            if prefix.pop().is_none() {
                return String::new();
            }
        }
    }
    prefix
}

fn community_purpose(paths: &[String]) -> String {
    let prefix = common_prefix(paths);
    if !prefix.contains('/') {
        return String::new();
    }
    prefix
        .rsplit_once('/')
        .map(|(before_last, _)| before_last.rsplit('/').next().unwrap_or(""))
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn temp_db(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("dagayn-rust-{}-{}.db", name, std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn creates_current_schema() {
        let path = temp_db("schema");
        let store = GraphStore::open(&path).expect("open graph store");
        assert_eq!(store.schema_version().unwrap(), LATEST_VERSION);
        assert!(table_exists(&store.conn, "nodes_fts").unwrap());
        assert!(has_column(&store.conn, "edges", "confidence_tier").unwrap());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn atomically_replaces_file_data() {
        let path = temp_db("replace");
        let mut store = GraphStore::open(&path).expect("open graph store");
        let file = NodeInput {
            kind: "File".to_string(),
            name: "app.py".to_string(),
            file_path: "app.py".to_string(),
            line_start: 1,
            line_end: 1,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let func = NodeInput {
            kind: "Function".to_string(),
            name: "main".to_string(),
            file_path: "app.py".to_string(),
            line_start: 1,
            line_end: 3,
            language: "python".to_string(),
            parent_name: None,
            params: Some("()".to_string()),
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        store
            .store_file_nodes_edges("app.py", &[file, func], &[], "hash1")
            .unwrap();
        store
            .store_file_nodes_edges("app.py", &[], &[], "hash2")
            .unwrap();
        assert!(store.get_all_files().unwrap().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stores_file_batch_in_one_transaction() {
        let path = temp_db("batch");
        let mut store = GraphStore::open(&path).expect("open graph store");
        let file_a = NodeInput {
            kind: "File".to_string(),
            name: "a.py".to_string(),
            file_path: "a.py".to_string(),
            line_start: 1,
            line_end: 1,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let file_b = NodeInput {
            kind: "File".to_string(),
            name: "b.py".to_string(),
            file_path: "b.py".to_string(),
            line_start: 1,
            line_end: 1,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };

        store
            .store_file_batch(&[
                (
                    "a.py".to_string(),
                    vec![file_a],
                    vec![],
                    "hash-a".to_string(),
                ),
                (
                    "b.py".to_string(),
                    vec![file_b],
                    vec![],
                    "hash-b".to_string(),
                ),
            ])
            .unwrap();

        let mut files = store.get_all_files().unwrap();
        files.sort();
        assert_eq!(files, vec!["a.py", "b.py"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stores_compact_json_batch() {
        let path = temp_db("json-batch");
        let mut store = GraphStore::open(&path).expect("open graph store");
        store
            .store_file_batch_json(
                r#"[
                    [
                        "app.py",
                        [["File","app.py","app.py",1,1,"python",null,null,null,null,false,{}]],
                        [],
                        "hash"
                    ]
                ]"#,
            )
            .unwrap();

        assert_eq!(
            store.get_file_hashes(&["app.py".to_string()]).unwrap()["app.py"],
            "hash"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rebuilds_fts_index() {
        let path = temp_db("fts");
        let mut store = GraphStore::open(&path).expect("open graph store");
        let file = NodeInput {
            kind: "File".to_string(),
            name: "app.py".to_string(),
            file_path: "app.py".to_string(),
            line_start: 1,
            line_end: 1,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let func = NodeInput {
            kind: "Function".to_string(),
            name: "calculate_total".to_string(),
            file_path: "app.py".to_string(),
            line_start: 3,
            line_end: 5,
            language: "python".to_string(),
            parent_name: None,
            params: Some("()".to_string()),
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };

        store
            .store_file_nodes_edges("app.py", &[file, func], &[], "hash")
            .unwrap();
        store
            .conn
            .execute("DROP TABLE IF EXISTS nodes_fts", [])
            .unwrap();

        assert_eq!(store.rebuild_fts_index().unwrap(), 2);
        let hit: String = store
            .conn
            .query_row(
                "SELECT name FROM nodes_fts WHERE name MATCH 'calculate*'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hit, "calculate_total");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn computes_missing_signatures() {
        let path = temp_db("signatures");
        let mut store = GraphStore::open(&path).expect("open graph store");
        let class = NodeInput {
            kind: "Class".to_string(),
            name: "Service".to_string(),
            file_path: "app.py".to_string(),
            line_start: 1,
            line_end: 10,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let func = NodeInput {
            kind: "Function".to_string(),
            name: "handle".to_string(),
            file_path: "app.py".to_string(),
            line_start: 3,
            line_end: 5,
            language: "python".to_string(),
            parent_name: Some("Service".to_string()),
            params: Some("request".to_string()),
            return_type: Some("Response".to_string()),
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };

        store
            .store_file_nodes_edges("app.py", &[class, func], &[], "hash")
            .unwrap();

        assert_eq!(store.compute_missing_signatures().unwrap(), 2);
        assert_eq!(store.compute_missing_signatures().unwrap(), 0);
        let signatures = store
            .conn
            .prepare("SELECT qualified_name, signature FROM nodes ORDER BY qualified_name")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            signatures,
            vec![
                ("app.py::Service".to_string(), "class Service".to_string()),
                (
                    "app.py::Service.handle".to_string(),
                    "def handle(request) -> Response".to_string(),
                ),
            ]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn resolves_markdown_artifact_refs() {
        let path = temp_db("markdown-refs");
        let mut store = GraphStore::open(&path).expect("open graph store");
        let target = NodeInput {
            kind: "Class".to_string(),
            name: "BridgePattern".to_string(),
            file_path: "parser.py".to_string(),
            line_start: 1,
            line_end: 10,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let edge = EdgeInput {
            kind: "CROSS_ARTIFACT".to_string(),
            source: "docs/spec.md::section".to_string(),
            target: "<unresolved:BridgePattern>".to_string(),
            file_path: "docs/spec.md".to_string(),
            line: 5,
            extra: json!({
                "relationship_role": "describes_symbol",
                "target_language": "unknown",
                "confidence": 0.2,
                "confidence_tier": "LOW",
                "unresolved_target_name": "BridgePattern",
            }),
        };

        store
            .store_file_batch(&[(
                "parser.py".to_string(),
                vec![target],
                vec![edge],
                "hash".to_string(),
            )])
            .unwrap();

        assert_eq!(store.resolve_markdown_artifact_refs().unwrap(), (1, 0));
        let row = store
            .conn
            .query_row(
                "SELECT target_qualified, confidence, confidence_tier, extra \
                 FROM edges WHERE kind = 'CROSS_ARTIFACT'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, "parser.py::BridgePattern");
        assert_eq!(row.1, 0.8);
        assert_eq!(row.2, "HIGH");
        let extra: Value = serde_json::from_str(&row.3).unwrap();
        assert!(extra.get("unresolved_target_name").is_none());
        assert_eq!(extra["target_language"], "python");
        assert_eq!(extra["confidence"], 0.8);
        assert_eq!(store.resolve_markdown_artifact_refs().unwrap(), (0, 0));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn computes_summary_tables() {
        let path = temp_db("summaries");
        let mut store = GraphStore::open(&path).expect("open graph store");
        let file = NodeInput {
            kind: "File".to_string(),
            name: "auth.py".to_string(),
            file_path: "auth.py".to_string(),
            line_start: 1,
            line_end: 20,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let login = NodeInput {
            kind: "Function".to_string(),
            name: "login".to_string(),
            file_path: "auth.py".to_string(),
            line_start: 1,
            line_end: 5,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let check_token = NodeInput {
            kind: "Function".to_string(),
            name: "check_token".to_string(),
            file_path: "auth.py".to_string(),
            line_start: 6,
            line_end: 10,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let test_login = NodeInput {
            kind: "Test".to_string(),
            name: "test_login".to_string(),
            file_path: "auth.py".to_string(),
            line_start: 12,
            line_end: 15,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: true,
            extra: Value::Object(Default::default()),
        };
        let calls = EdgeInput {
            kind: "CALLS".to_string(),
            source: "auth.py::login".to_string(),
            target: "auth.py::check_token".to_string(),
            file_path: "auth.py".to_string(),
            line: 2,
            extra: Value::Object(Default::default()),
        };
        let tested_by = EdgeInput {
            kind: "TESTED_BY".to_string(),
            source: "auth.py::login".to_string(),
            target: "auth.py::test_login".to_string(),
            file_path: "auth.py".to_string(),
            line: 13,
            extra: Value::Object(Default::default()),
        };
        store
            .store_file_batch(&[(
                "auth.py".to_string(),
                vec![file, login, check_token, test_login],
                vec![calls, tested_by],
                "hash".to_string(),
            )])
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO communities (name, level, cohesion, size, dominant_language) \
                 VALUES ('auth-cluster', 0, 1.0, 3, 'python')",
                [],
            )
            .unwrap();
        let community_id: i64 = store
            .conn
            .query_row(
                "SELECT id FROM communities WHERE name = 'auth-cluster'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        store
            .conn
            .execute("UPDATE nodes SET community_id = ?", [community_id])
            .unwrap();
        let login_id: i64 = store
            .conn
            .query_row(
                "SELECT id FROM nodes WHERE qualified_name = 'auth.py::login'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let token_id: i64 = store
            .conn
            .query_row(
                "SELECT id FROM nodes WHERE qualified_name = 'auth.py::check_token'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO flows \
                 (name, entry_point_id, depth, node_count, file_count, criticality, path_json) \
                 VALUES ('auth flow', ?, 2, 2, 1, 0.5, ?)",
                params![
                    login_id,
                    serde_json::to_string(&vec![login_id, token_id]).unwrap()
                ],
            )
            .unwrap();

        store.compute_summaries().unwrap();

        let community_row: (String, i64, String) = store
            .conn
            .query_row(
                "SELECT name, size, key_symbols FROM community_summaries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(community_row.0, "auth-cluster");
        assert_eq!(community_row.1, 3);
        let key_symbols: Vec<String> = serde_json::from_str(&community_row.2).unwrap();
        assert_eq!(key_symbols[0], "login");

        let flow_path: String = store
            .conn
            .query_row("SELECT critical_path FROM flow_snapshots", [], |row| {
                row.get(0)
            })
            .unwrap();
        let flow_path: Vec<String> = serde_json::from_str(&flow_path).unwrap();
        assert_eq!(flow_path, vec!["auth.py::login", "auth.py::check_token"]);

        let risk_row: (String, i64, String, i64, f64) = store
            .conn
            .query_row(
                "SELECT qualified_name, caller_count, test_coverage, security_relevant, risk_score \
                 FROM risk_index WHERE qualified_name = 'auth.py::check_token'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(risk_row.0, "auth.py::check_token");
        assert_eq!(risk_row.1, 1);
        assert_eq!(risk_row.2, "untested");
        assert_eq!(risk_row.3, 1);
        assert_eq!(risk_row.4, 0.7);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stores_flows_and_reads_flow_inputs() {
        let path = temp_db("flows");
        let mut store = GraphStore::open(&path).expect("open graph store");
        let entry = NodeInput {
            kind: "Function".to_string(),
            name: "entry".to_string(),
            file_path: "app.py".to_string(),
            line_start: 1,
            line_end: 5,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let callee = NodeInput {
            kind: "Function".to_string(),
            name: "callee".to_string(),
            file_path: "app.py".to_string(),
            line_start: 7,
            line_end: 10,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let edge = EdgeInput {
            kind: "CALLS".to_string(),
            source: "app.py::entry".to_string(),
            target: "app.py::callee".to_string(),
            file_path: "app.py".to_string(),
            line: 2,
            extra: Value::Object(Default::default()),
        };
        store
            .store_file_batch(&[(
                "app.py".to_string(),
                vec![entry, callee],
                vec![edge],
                "hash".to_string(),
            )])
            .unwrap();

        let targets = store.get_all_call_targets(false).unwrap();
        assert_eq!(targets, HashSet::from(["app.py::callee".to_string()]));
        let nodes = store
            .get_nodes_by_kind(&["Function".to_string()], None)
            .unwrap();
        assert_eq!(nodes.len(), 2);
        let (calls_out, tested_by) = store.get_flow_edge_data().unwrap();
        assert_eq!(calls_out["app.py::entry"], vec!["app.py::callee"]);
        assert!(tested_by.is_empty());

        let entry_id = store.get_node("app.py::entry").unwrap().unwrap().id;
        let callee_id = store.get_node("app.py::callee").unwrap().unwrap().id;
        let flows = vec![FlowInput {
            name: "entry".to_string(),
            entry_point_id: entry_id,
            depth: 1,
            node_count: 2,
            file_count: 1,
            criticality: 0.25,
            path: vec![entry_id, callee_id],
        }];
        assert_eq!(store.store_flows(&flows).unwrap(), 1);
        assert_eq!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM flows", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM flow_memberships", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
        assert_eq!(store.store_flows(&[]).unwrap(), 0);
        assert_eq!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM flows", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reads_nodes_and_edges_for_incremental_dependents() {
        let path = temp_db("read-api");
        let mut store = GraphStore::open(&path).expect("open graph store");
        let source = NodeInput {
            kind: "File".to_string(),
            name: "src/lib.py".to_string(),
            file_path: "src/lib.py".to_string(),
            line_start: 1,
            line_end: 1,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let function = NodeInput {
            kind: "Function".to_string(),
            name: "build".to_string(),
            file_path: "src/lib.py".to_string(),
            line_start: 3,
            line_end: 5,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: json!({"role": "entry"}),
        };
        let target = NodeInput {
            kind: "File".to_string(),
            name: "src/app.py".to_string(),
            file_path: "src/app.py".to_string(),
            line_start: 1,
            line_end: 1,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: false,
            extra: Value::Object(Default::default()),
        };
        let edge = EdgeInput {
            kind: "CALLS".to_string(),
            source: "src/app.py::main".to_string(),
            target: "src/lib.py::build".to_string(),
            file_path: "src/app.py".to_string(),
            line: 8,
            extra: json!({"confidence": 0.75, "confidence_tier": "HEURISTIC"}),
        };

        store
            .store_file_batch(&[
                (
                    "src/lib.py".to_string(),
                    vec![source, function],
                    vec![],
                    "hash-lib".to_string(),
                ),
                (
                    "src/app.py".to_string(),
                    vec![target],
                    vec![edge.clone(), edge],
                    "hash-app".to_string(),
                ),
            ])
            .unwrap();

        let nodes = store.get_nodes_by_file("src/lib.py").unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(
            store.get_node("src/lib.py::build").unwrap().unwrap().extra["role"],
            "entry"
        );

        let incoming = store.get_edges_by_target("src/lib.py::build").unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].file_path, "src/app.py");
        assert_eq!(incoming[0].confidence_tier, "HEURISTIC");

        let outgoing = store.get_edges_by_source("src/app.py::main").unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].confidence, 0.75);
        let _ = std::fs::remove_file(path);
    }
}
