use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

const LATEST_VERSION: i64 = 11;
const SECURITY_KEYWORDS: &[&str] = &[
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
    "query",
    "execute",
    "connect",
    "socket",
    "request",
    "http",
    "sanitize",
    "validate",
    "encrypt",
    "decrypt",
    "hash",
    "sign",
    "verify",
    "admin",
    "privilege",
];

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
    mtime_ns INTEGER DEFAULT 0,
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
CREATE INDEX IF NOT EXISTS idx_nodes_parent_name ON nodes(parent_name, name);
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

pub type EdgeEndpointMap = HashMap<String, Vec<GraphEdge>>;
type ChangedRanges = HashMap<String, Vec<(i64, i64)>>;

struct ChangeRiskInputs<'a> {
    node: &'a GraphNode,
    inbound_edges: &'a [GraphEdge],
    flow_criticalities: &'a [f64],
    flow_count: i64,
    node_community_id: Option<i64>,
    caller_community_ids: &'a HashMap<String, Option<i64>>,
    transitive_test_count: i64,
}

#[derive(Clone, Debug)]
pub struct GraphStats {
    pub total_nodes: i64,
    pub total_edges: i64,
    pub nodes_by_kind: HashMap<String, i64>,
    pub edges_by_kind: HashMap<String, i64>,
    pub languages: Vec<String>,
    pub files_count: i64,
    pub last_updated: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommunityInput {
    pub name: String,
    #[serde(default)]
    pub level: i64,
    #[serde(default)]
    pub cohesion: f64,
    pub size: i64,
    #[serde(default)]
    pub dominant_language: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub members: Vec<String>,
}

pub struct GraphStore {
    conn: Connection,
}

pub type FileBatchItem = (String, Vec<NodeInput>, Vec<EdgeInput>, String, i64);
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
        let tx = self.conn.transaction()?;
        store_file_batch_tx(&tx, batch)?;
        tx.commit()?;
        Ok(())
    }

    pub fn store_file_batch_json(&mut self, batch_json: &str) -> Result<()> {
        let compact: Vec<CompactFileBatchItem> = serde_json::from_str(batch_json)?;
        let batch = compact
            .into_iter()
            .map(|(file_path, nodes, edges, file_hash, mtime_ns)| {
                (
                    file_path,
                    nodes.into_iter().map(NodeInput::from).collect(),
                    edges.into_iter().map(EdgeInput::from).collect(),
                    file_hash,
                    mtime_ns,
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

    pub fn update_file_mtime(&self, file_path: &str, mtime_ns: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET mtime_ns = ? WHERE file_path = ?",
            params![mtime_ns, file_path],
        )?;
        Ok(())
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

    pub fn get_nodes_by_qualified_names(
        &self,
        qualified_names: &[String],
    ) -> Result<HashMap<String, GraphNode>> {
        if qualified_names.is_empty() {
            return Ok(HashMap::new());
        }

        let mut normalized_for = HashMap::new();
        let mut keys = HashSet::new();
        for qualified_name in qualified_names {
            let normalized = self.normalize_qualified_key(qualified_name)?;
            normalized_for.insert(qualified_name.clone(), normalized.clone());
            keys.insert(qualified_name.clone());
            if normalized != *qualified_name {
                keys.insert(normalized);
            }
        }

        let keys = keys.into_iter().collect::<Vec<_>>();
        let mut rows_by_qn = HashMap::new();
        for chunk in keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM nodes WHERE qualified_name IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), node_from_row)?;
            for row in rows {
                let node = row?;
                rows_by_qn
                    .entry(node.qualified_name.clone())
                    .or_insert(node);
            }
        }

        let mut out = HashMap::new();
        for original in qualified_names {
            if let Some(node) = rows_by_qn.get(original) {
                out.insert(original.clone(), node.clone());
                continue;
            }
            if let Some(normalized) = normalized_for.get(original) {
                if let Some(node) = rows_by_qn.get(normalized) {
                    out.insert(original.clone(), node.clone());
                }
            }
        }
        Ok(out)
    }

    pub fn get_nodes_by_ids(&self, node_ids: &[i64]) -> Result<HashMap<i64, GraphNode>> {
        let mut out = HashMap::new();
        if node_ids.is_empty() {
            return Ok(out);
        }

        let mut unique_ids = Vec::new();
        let mut seen = HashSet::new();
        for node_id in node_ids {
            if seen.insert(*node_id) {
                unique_ids.push(*node_id);
            }
        }

        for chunk in unique_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), node_from_row)?;
            for row in rows {
                let node = row?;
                out.insert(node.id, node);
            }
        }
        Ok(out)
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

    pub fn get_all_nodes_filtered(&self, exclude_files: bool) -> Result<Vec<GraphNode>> {
        if !exclude_files {
            return self.get_all_nodes();
        }
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM nodes WHERE kind != 'File'")?;
        let rows = stmt.query_map([], node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_edges(&self) -> Result<Vec<GraphEdge>> {
        let mut stmt = self.conn.prepare("SELECT * FROM edges")?;
        let rows = stmt.query_map([], edge_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_stats(&self) -> Result<GraphStats> {
        let total_nodes = self
            .conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?;
        let total_edges = self
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;

        let mut nodes_by_kind = HashMap::new();
        let mut node_stmt = self
            .conn
            .prepare("SELECT kind, COUNT(*) as cnt FROM nodes GROUP BY kind")?;
        let node_rows = node_stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in node_rows {
            let (kind, count) = row?;
            nodes_by_kind.insert(kind, count);
        }

        let mut edges_by_kind = HashMap::new();
        let mut edge_stmt = self
            .conn
            .prepare("SELECT kind, COUNT(*) as cnt FROM edges GROUP BY kind")?;
        let edge_rows = edge_stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in edge_rows {
            let (kind, count) = row?;
            edges_by_kind.insert(kind, count);
        }

        let mut lang_stmt = self.conn.prepare(
            "SELECT DISTINCT language FROM nodes WHERE language IS NOT NULL AND language != ''",
        )?;
        let lang_rows = lang_stmt.query_map([], |row| row.get::<_, String>(0))?;
        let languages = lang_rows.collect::<std::result::Result<Vec<_>, _>>()?;

        let files_count = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE kind = 'File'",
            [],
            |row| row.get(0),
        )?;
        let last_updated = self.get_metadata("last_updated")?;

        Ok(GraphStats {
            total_nodes,
            total_edges,
            nodes_by_kind,
            edges_by_kind,
            languages,
            files_count,
            last_updated,
        })
    }

    pub fn get_nodes_by_community_id(&self, community_id: i64) -> Result<Vec<GraphNode>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM nodes WHERE community_id = ?")?;
        let rows = stmt.query_map([community_id], node_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_files_matching(&self, pattern: &str) -> Result<Vec<String>> {
        let like = format!("%{pattern}");
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT file_path FROM nodes WHERE file_path LIKE ?")?;
        let rows = stmt.query_map([like], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn count_flow_memberships(&self, node_id: i64) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) as cnt FROM flow_memberships WHERE node_id = ?",
                [node_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn count_flow_memberships_for_nodes(&self, node_ids: &[i64]) -> Result<HashMap<i64, i64>> {
        let mut out = node_ids
            .iter()
            .map(|node_id| (*node_id, 0))
            .collect::<HashMap<_, _>>();
        if node_ids.is_empty() {
            return Ok(out);
        }

        for chunk in node_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT node_id, COUNT(*) FROM flow_memberships \
                 WHERE node_id IN ({placeholders}) GROUP BY node_id"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (node_id, count) = row?;
                out.insert(node_id, count);
            }
        }
        Ok(out)
    }

    pub fn get_flow_criticalities_for_node(&self, node_id: i64) -> Result<Vec<f64>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.criticality FROM flows f \
             JOIN flow_memberships fm ON fm.flow_id = f.id \
             WHERE fm.node_id = ?",
        )?;
        let rows = stmt.query_map([node_id], |row| row.get::<_, f64>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_flow_criticalities_for_nodes(
        &self,
        node_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<f64>>> {
        let mut out = node_ids
            .iter()
            .map(|node_id| (*node_id, Vec::new()))
            .collect::<HashMap<_, _>>();
        if node_ids.is_empty() {
            return Ok(out);
        }

        for chunk in node_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT fm.node_id, f.criticality FROM flows f \
                 JOIN flow_memberships fm ON fm.flow_id = f.id \
                 WHERE fm.node_id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })?;
            for row in rows {
                let (node_id, criticality) = row?;
                out.entry(node_id).or_default().push(criticality);
            }
        }
        Ok(out)
    }

    pub fn get_node_community_id(&self, node_id: i64) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT community_id FROM nodes WHERE id = ?",
                [node_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(|row| row.flatten())
            .map_err(Into::into)
    }

    pub fn get_community_ids_by_node_ids(
        &self,
        node_ids: &[i64],
    ) -> Result<HashMap<i64, Option<i64>>> {
        let mut out = node_ids
            .iter()
            .map(|node_id| (*node_id, None))
            .collect::<HashMap<_, _>>();
        if node_ids.is_empty() {
            return Ok(out);
        }

        for chunk in node_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id, community_id FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
            })?;
            for row in rows {
                let (node_id, community_id) = row?;
                out.insert(node_id, community_id);
            }
        }
        Ok(out)
    }

    pub fn get_community_ids_by_qualified_names(
        &self,
        qns: &[String],
    ) -> Result<HashMap<String, Option<i64>>> {
        let mut out = HashMap::new();
        for chunk in qns.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT qualified_name, community_id FROM nodes \
                 WHERE qualified_name IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })?;
            for row in rows {
                let (qualified_name, community_id) = row?;
                out.insert(qualified_name, community_id);
            }
        }
        Ok(out)
    }

    pub fn get_transitive_tests(&self, qualified_name: &str, max_depth: i64) -> Result<Vec<Value>> {
        let mut seen = HashSet::new();
        let mut results = Vec::new();

        let mut input_qns = vec![qualified_name.to_string()];
        let node_kind = self
            .conn
            .query_row(
                "SELECT kind FROM nodes WHERE qualified_name = ?",
                [qualified_name],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if node_kind.as_deref() == Some("Class") {
            let mut stmt = self.conn.prepare(
                "SELECT target_qualified FROM edges \
                 WHERE source_qualified = ? AND kind = 'CONTAINS'",
            )?;
            let rows = stmt.query_map([qualified_name], |row| row.get::<_, String>(0))?;
            for row in rows {
                input_qns.push(row?);
            }
        }

        for qn in &input_qns {
            for source in self.get_test_sources_for_target(qn)? {
                if seen.insert(source.clone()) {
                    if let Some(test_node) = self.test_node_json(&source, false)? {
                        results.push(test_node);
                    }
                }
            }
        }

        let bare = qualified_name
            .rsplit_once("::")
            .map(|(_, name)| name)
            .unwrap_or(qualified_name);
        for source in self.get_test_sources_for_target(bare)? {
            if seen.insert(source.clone()) {
                if let Some(test_node) = self.test_node_json(&source, false)? {
                    results.push(test_node);
                }
            }
        }

        let mut frontier = input_qns.into_iter().collect::<HashSet<_>>();
        for _ in 0..max_depth {
            let mut next_frontier = HashSet::new();
            for qn in &frontier {
                let mut stmt = self.conn.prepare(
                    "SELECT target_qualified FROM edges \
                     WHERE source_qualified = ? AND kind = 'CALLS'",
                )?;
                let rows = stmt.query_map([qn], |row| row.get::<_, String>(0))?;
                for row in rows {
                    next_frontier.insert(row?);
                }
            }
            for callee in &next_frontier {
                for source in self.get_test_sources_for_target(callee)? {
                    if seen.insert(source.clone()) {
                        if let Some(test_node) = self.test_node_json(&source, true)? {
                            results.push(test_node);
                        }
                    }
                }
            }
            frontier = next_frontier;
        }

        Ok(results)
    }

    fn get_transitive_test_counts(
        &self,
        qualified_names: &[String],
        max_depth: i64,
    ) -> Result<HashMap<String, i64>> {
        let mut seen_tests = qualified_names
            .iter()
            .map(|qualified_name| (qualified_name.clone(), HashSet::new()))
            .collect::<HashMap<_, HashSet<String>>>();
        if qualified_names.is_empty() {
            return Ok(HashMap::new());
        }

        let node_kinds = self.get_node_kinds_by_qualified_names(qualified_names)?;
        let class_qns = qualified_names
            .iter()
            .filter(|qualified_name| {
                node_kinds
                    .get(*qualified_name)
                    .is_some_and(|kind| kind == "Class")
            })
            .cloned()
            .collect::<Vec<_>>();
        let contains_by_class = self.get_contains_targets_by_sources(&class_qns)?;

        let mut direct_target_to_originals: HashMap<String, Vec<String>> = HashMap::new();
        let mut frontier_source_to_originals: HashMap<String, Vec<String>> = HashMap::new();
        for qualified_name in qualified_names {
            direct_target_to_originals
                .entry(qualified_name.clone())
                .or_default()
                .push(qualified_name.clone());
            frontier_source_to_originals
                .entry(qualified_name.clone())
                .or_default()
                .push(qualified_name.clone());

            if let Some(bare) = qualified_name.rsplit_once("::").map(|(_, name)| name) {
                direct_target_to_originals
                    .entry(bare.to_string())
                    .or_default()
                    .push(qualified_name.clone());
            }

            if let Some(contained) = contains_by_class.get(qualified_name) {
                for target in contained {
                    direct_target_to_originals
                        .entry(target.clone())
                        .or_default()
                        .push(qualified_name.clone());
                    frontier_source_to_originals
                        .entry(target.clone())
                        .or_default()
                        .push(qualified_name.clone());
                }
            }
        }

        let direct_targets = direct_target_to_originals
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for (target, source) in self.get_test_sources_by_targets(&direct_targets)? {
            if let Some(originals) = direct_target_to_originals.get(&target) {
                for original in originals {
                    if let Some(seen) = seen_tests.get_mut(original) {
                        seen.insert(source.clone());
                    }
                }
            }
        }

        let mut frontier = frontier_source_to_originals;
        for _ in 0..max_depth {
            if frontier.is_empty() {
                break;
            }
            let sources = frontier.keys().cloned().collect::<Vec<_>>();
            let calls_by_source = self.get_call_targets_by_sources(&sources)?;
            let mut callee_to_originals: HashMap<String, Vec<String>> = HashMap::new();
            for (source, callees) in calls_by_source {
                let Some(originals) = frontier.get(&source) else {
                    continue;
                };
                for callee in callees {
                    callee_to_originals
                        .entry(callee)
                        .or_default()
                        .extend(originals.iter().cloned());
                }
            }

            let callees = callee_to_originals.keys().cloned().collect::<Vec<_>>();
            for (target, source) in self.get_test_sources_by_targets(&callees)? {
                if let Some(originals) = callee_to_originals.get(&target) {
                    for original in originals {
                        if let Some(seen) = seen_tests.get_mut(original) {
                            seen.insert(source.clone());
                        }
                    }
                }
            }
            frontier = callee_to_originals;
        }

        Ok(seen_tests
            .into_iter()
            .map(|(qualified_name, seen)| (qualified_name, seen.len() as i64))
            .collect())
    }

    fn get_node_kinds_by_qualified_names(
        &self,
        qualified_names: &[String],
    ) -> Result<HashMap<String, String>> {
        let mut out = HashMap::new();
        for chunk in qualified_names.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT qualified_name, kind FROM nodes WHERE qualified_name IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (qualified_name, kind) = row?;
                out.insert(qualified_name, kind);
            }
        }
        Ok(out)
    }

    fn get_contains_targets_by_sources(
        &self,
        source_qualified_names: &[String],
    ) -> Result<HashMap<String, Vec<String>>> {
        self.get_edge_targets_by_sources(source_qualified_names, "CONTAINS")
    }

    fn get_call_targets_by_sources(
        &self,
        source_qualified_names: &[String],
    ) -> Result<HashMap<String, Vec<String>>> {
        self.get_edge_targets_by_sources(source_qualified_names, "CALLS")
    }

    fn get_edge_targets_by_sources(
        &self,
        source_qualified_names: &[String],
        kind: &str,
    ) -> Result<HashMap<String, Vec<String>>> {
        let mut out = HashMap::new();
        for chunk in source_qualified_names.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT source_qualified, target_qualified FROM edges \
                 WHERE kind = ? AND source_qualified IN ({placeholders})"
            );
            let mut params = Vec::with_capacity(chunk.len() + 1);
            params.push(kind.to_string());
            params.extend(chunk.iter().cloned());
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (source, target) = row?;
                out.entry(source).or_insert_with(Vec::new).push(target);
            }
        }
        Ok(out)
    }

    fn get_test_sources_by_targets(&self, targets: &[String]) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        for chunk in targets.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT e.target_qualified, e.source_qualified FROM edges e \
                 JOIN nodes n ON n.qualified_name = e.source_qualified \
                 WHERE e.kind = 'TESTED_BY' AND e.target_qualified IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    pub fn count_affected_communities(&self, file_paths: &[String]) -> Result<i64> {
        let mut community_ids = HashSet::new();
        for chunk in file_paths.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT community_id FROM nodes \
                 WHERE community_id IS NOT NULL AND file_path IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, i64>(0)
            })?;
            for row in rows {
                community_ids.insert(row?);
            }
        }
        Ok(community_ids.len() as i64)
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

    pub fn insert_flows_json(&mut self, flows_json: &str) -> Result<i64> {
        let flows: Vec<FlowInput> = serde_json::from_str(flows_json)?;
        let tx = self.conn.transaction()?;
        store_flows_tx(&tx, &flows)?;
        tx.commit()?;
        Ok(flows.len() as i64)
    }

    pub fn get_flows_json(&self, sort_by: &str, limit: i64) -> Result<String> {
        let sort_by = match sort_by {
            "criticality" | "depth" | "node_count" | "file_count" | "name" => sort_by,
            _ => "criticality",
        };
        let order = if matches!(
            sort_by,
            "criticality" | "depth" | "node_count" | "file_count"
        ) {
            "DESC"
        } else {
            "ASC"
        };
        let sql = format!("SELECT * FROM flows ORDER BY {sort_by} {order} LIMIT ?");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([limit], flow_json_from_row)?;
        let flows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        serde_json::to_string(&flows).map_err(Into::into)
    }

    pub fn get_flow_by_id_json(&self, flow_id: i64) -> Result<Option<String>> {
        let flow = self
            .conn
            .query_row("SELECT * FROM flows WHERE id = ?", [flow_id], |row| {
                flow_json_from_row(row)
            })
            .optional()?;
        let Some(mut flow) = flow else {
            return Ok(None);
        };
        let path_ids = flow
            .get("path")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_i64).collect::<Vec<_>>())
            .unwrap_or_default();
        let steps = self.flow_steps_json(&path_ids)?;
        if let Some(obj) = flow.as_object_mut() {
            obj.insert("steps".to_string(), Value::Array(steps));
        }
        serde_json::to_string(&flow).map(Some).map_err(Into::into)
    }

    pub fn get_affected_flows_json(&self, changed_files: &[String]) -> Result<String> {
        if changed_files.is_empty() {
            return Ok("[]".to_string());
        }
        let node_ids = self.get_node_ids_by_files(changed_files)?;
        if node_ids.is_empty() {
            return Ok("[]".to_string());
        }
        let flow_ids = self.get_flow_ids_by_node_ids(&node_ids)?;
        if flow_ids.is_empty() {
            return Ok("[]".to_string());
        }
        let mut flows = Vec::new();
        for flow_id in flow_ids {
            if let Some(raw) = self.get_flow_by_id_json(flow_id)? {
                flows.push(serde_json::from_str::<Value>(&raw)?);
            }
        }
        flows.sort_by(|left, right| {
            let left = left
                .get("criticality")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let right = right
                .get("criticality")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            right
                .partial_cmp(&left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        serde_json::to_string(&flows).map_err(Into::into)
    }

    pub fn analyze_changes_json(
        &self,
        changed_files: &[String],
        changed_ranges_json: Option<&str>,
    ) -> Result<String> {
        let changed_ranges = match changed_ranges_json {
            Some(raw) if !raw.is_empty() => serde_json::from_str::<ChangedRanges>(raw)?,
            _ => HashMap::new(),
        };
        let changed_nodes = if changed_ranges.is_empty() {
            self.changed_nodes_by_files(changed_files)?
        } else {
            self.changed_nodes_by_ranges(&changed_ranges)?
        };
        let changed_funcs = changed_nodes
            .into_iter()
            .filter(|node| matches!(node.kind.as_str(), "Function" | "Test" | "Class"))
            .collect::<Vec<_>>();

        let func_ids = changed_funcs.iter().map(|node| node.id).collect::<Vec<_>>();
        let func_qns = changed_funcs
            .iter()
            .map(|node| node.qualified_name.clone())
            .collect::<Vec<_>>();

        let flow_crit_map = self.get_flow_criticalities_for_nodes(&func_ids)?;
        let nodes_needing_count = flow_crit_map
            .iter()
            .filter_map(|(node_id, values)| {
                if values.is_empty() {
                    Some(*node_id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let flow_count_map = if nodes_needing_count.is_empty() {
            HashMap::new()
        } else {
            self.count_flow_memberships_for_nodes(&nodes_needing_count)?
        };
        let node_cid_map = self.get_community_ids_by_node_ids(&func_ids)?;
        let (_, inbound_map) = self.get_edges_by_endpoints(&func_qns)?;

        let mut caller_qns = HashSet::new();
        for edges in inbound_map.values() {
            for edge in edges {
                if edge.kind == "CALLS" {
                    caller_qns.insert(edge.source_qualified.clone());
                }
            }
        }
        let caller_qns = caller_qns.into_iter().collect::<Vec<_>>();
        let caller_cid_map = if caller_qns.is_empty() {
            HashMap::new()
        } else {
            self.get_community_ids_by_qualified_names(&caller_qns)?
        };
        let transitive_test_counts = self.get_transitive_test_counts(&func_qns, 1)?;

        let mut node_risks = Vec::new();
        for node in &changed_funcs {
            let inbound_edges = inbound_map
                .get(&node.qualified_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let flow_criticalities = flow_crit_map
                .get(&node.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let flow_count = *flow_count_map.get(&node.id).unwrap_or(&0);
            let risk = self.compute_change_risk_score(ChangeRiskInputs {
                node,
                inbound_edges,
                flow_criticalities,
                flow_count,
                node_community_id: node_cid_map.get(&node.id).copied().flatten(),
                caller_community_ids: &caller_cid_map,
                transitive_test_count: *transitive_test_counts
                    .get(&node.qualified_name)
                    .unwrap_or(&0),
            })?;
            let mut value = node_to_value(node);
            if let Some(obj) = value.as_object_mut() {
                obj.insert("risk_score".to_string(), json!(risk));
            }
            node_risks.push(value);
        }

        let overall_risk = node_risks
            .iter()
            .filter_map(|value| value.get("risk_score").and_then(Value::as_f64))
            .fold(0.0, f64::max);
        let affected_flows =
            serde_json::from_str::<Vec<Value>>(&self.get_affected_flows_json(changed_files)?)?;

        let mut test_gaps = Vec::new();
        for node in &changed_funcs {
            if node.is_test {
                continue;
            }
            let tested = inbound_map
                .get(&node.qualified_name)
                .map(|edges| edges.iter().any(|edge| edge.kind == "TESTED_BY"))
                .unwrap_or(false);
            if !tested {
                test_gaps.push(json!({
                    "name": sanitize_name(&node.name),
                    "qualified_name": sanitize_name(&node.qualified_name),
                    "file": node.file_path,
                    "line_start": node.line_start,
                    "line_end": node.line_end,
                }));
            }
        }

        let mut review_priorities = node_risks.clone();
        review_priorities.sort_by(|left, right| {
            let left = left
                .get("risk_score")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let right = right
                .get("risk_score")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            right
                .partial_cmp(&left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        review_priorities.truncate(10);

        let mut summary_parts = vec![
            format!("Analyzed {} changed file(s):", changed_files.len()),
            format!("  - {} changed function(s)/class(es)", changed_funcs.len()),
            format!("  - {} affected flow(s)", affected_flows.len()),
            format!("  - {} test gap(s)", test_gaps.len()),
            format!("  - Overall risk score: {overall_risk:.2}"),
        ];
        if !test_gaps.is_empty() {
            let gap_names = test_gaps
                .iter()
                .take(5)
                .filter_map(|gap| gap.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ");
            summary_parts.push(format!("  - Untested: {gap_names}"));
        }

        serde_json::to_string(&json!({
            "summary": summary_parts.join("\n"),
            "risk_score": overall_risk,
            "changed_functions": node_risks,
            "affected_flows": affected_flows,
            "test_gaps": test_gaps,
            "review_priorities": review_priorities,
        }))
        .map_err(Into::into)
    }

    pub fn delete_affected_flows(&mut self, changed_files: &[String]) -> Result<Vec<i64>> {
        if changed_files.is_empty() {
            return Ok(Vec::new());
        }
        let node_ids = self.get_node_ids_by_files(changed_files)?;
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let flow_ids = self.get_flow_ids_by_node_ids(&node_ids)?;
        if flow_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut entry_point_ids = Vec::new();
        let mut seen_entry_points = HashSet::new();
        for chunk in flow_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT entry_point_id FROM flows WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, i64>(0)
            })?;
            for row in rows {
                let entry_point_id = row?;
                if seen_entry_points.insert(entry_point_id) {
                    entry_point_ids.push(entry_point_id);
                }
            }
        }

        let tx = self.conn.transaction()?;
        {
            let mut delete_membership =
                tx.prepare("DELETE FROM flow_memberships WHERE flow_id = ?")?;
            let mut delete_flow = tx.prepare("DELETE FROM flows WHERE id = ?")?;
            for flow_id in flow_ids {
                delete_membership.execute([flow_id])?;
                delete_flow.execute([flow_id])?;
            }
        }
        tx.commit()?;
        Ok(entry_point_ids)
    }

    pub fn get_node_kind_by_id(&self, node_id: i64) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT kind FROM nodes WHERE id = ?", [node_id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    pub fn store_communities_json(&mut self, communities_json: &str) -> Result<i64> {
        let communities: Vec<CommunityInput> = serde_json::from_str(communities_json)?;
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM community_summaries", [])?;
        tx.execute("DELETE FROM communities", [])?;
        tx.execute("UPDATE nodes SET community_id = NULL", [])?;
        let mut insert = tx.prepare(
            "INSERT INTO communities \
             (name, level, cohesion, size, dominant_language, description) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )?;
        for community in &communities {
            insert.execute(params![
                community.name,
                community.level,
                community.cohesion,
                community.size,
                community.dominant_language,
                community.description
            ])?;
            let community_id = tx.last_insert_rowid();
            for chunk in community.members.chunks(450) {
                if chunk.is_empty() {
                    continue;
                }
                let placeholders = std::iter::repeat_n("?", chunk.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!(
                    "UPDATE nodes SET community_id = ? WHERE qualified_name IN ({placeholders})"
                );
                let mut params = Vec::with_capacity(chunk.len() + 1);
                params.push(rusqlite::types::Value::Integer(community_id));
                params.extend(chunk.iter().cloned().map(rusqlite::types::Value::Text));
                tx.execute(&sql, rusqlite::params_from_iter(params))?;
            }
        }
        drop(insert);
        tx.commit()?;
        Ok(communities.len() as i64)
    }

    pub fn get_communities_json(&self, sort_by: &str, min_size: i64) -> Result<String> {
        let sort_by = match sort_by {
            "size" | "cohesion" | "name" => sort_by,
            _ => "size",
        };
        let order = if matches!(sort_by, "size" | "cohesion") {
            "DESC"
        } else {
            "ASC"
        };
        let sql = format!("SELECT * FROM communities WHERE size >= ? ORDER BY {sort_by} {order}");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([min_size], community_json_from_row)?;
        let mut communities = Vec::new();
        for row in rows {
            let mut community = row?;
            let id = community.get("id").and_then(Value::as_i64).unwrap_or(0);
            let members = self.get_community_member_qns(id)?;
            if let Some(obj) = community.as_object_mut() {
                obj.insert(
                    "members".to_string(),
                    Value::Array(
                        members
                            .into_iter()
                            .map(|member| Value::String(sanitize_name(&member)))
                            .collect(),
                    ),
                );
            }
            communities.push(community);
        }
        serde_json::to_string(&communities).map_err(Into::into)
    }

    fn get_community_member_qns(&self, community_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT qualified_name FROM nodes WHERE community_id = ?")?;
        let rows = stmt.query_map([community_id], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_all_community_member_qns(&self) -> Result<HashMap<i64, Vec<String>>> {
        let mut out = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT community_id, qualified_name FROM nodes \
             WHERE community_id IS NOT NULL ORDER BY community_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (community_id, qualified_name) = row?;
            out.entry(community_id)
                .or_insert_with(Vec::new)
                .push(qualified_name);
        }
        Ok(out)
    }

    fn get_test_sources_for_target(&self, target_qualified: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_qualified FROM edges \
             WHERE target_qualified = ? AND kind = 'TESTED_BY'",
        )?;
        let rows = stmt.query_map([target_qualified], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn test_node_json(&self, qualified_name: &str, indirect: bool) -> Result<Option<Value>> {
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

    fn get_node_ids_by_files(&self, file_paths: &[String]) -> Result<HashSet<i64>> {
        let mut out = HashSet::new();
        let file_keys = self.expand_file_keys(file_paths)?;
        for chunk in file_keys.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id FROM nodes WHERE file_path IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| row.get(0))?;
            for row in rows {
                out.insert(row?);
            }
        }
        Ok(out)
    }

    fn expand_file_keys(&self, file_paths: &[String]) -> Result<Vec<String>> {
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

    fn changed_nodes_by_files(&self, changed_files: &[String]) -> Result<Vec<GraphNode>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for file_path in changed_files {
            for node in self.get_nodes_by_file(file_path)? {
                if seen.insert(node.qualified_name.clone()) {
                    out.push(node);
                }
            }
        }
        Ok(out)
    }

    fn changed_nodes_by_ranges(&self, changed_ranges: &ChangedRanges) -> Result<Vec<GraphNode>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for (file_path, ranges) in changed_ranges {
            let mut nodes = self.get_nodes_by_file(file_path)?;
            if nodes.is_empty() {
                for matched_path in self.get_files_matching(file_path)? {
                    nodes.extend(self.get_nodes_by_file(&matched_path)?);
                }
            }
            for node in nodes {
                if seen.contains(&node.qualified_name) {
                    continue;
                }
                if ranges
                    .iter()
                    .any(|(start, end)| node.line_start <= *end && node.line_end >= *start)
                {
                    seen.insert(node.qualified_name.clone());
                    out.push(node);
                }
            }
        }
        Ok(out)
    }

    fn compute_change_risk_score(&self, inputs: ChangeRiskInputs<'_>) -> Result<f64> {
        let mut score = 0.0_f64;

        if inputs.flow_criticalities.is_empty() {
            score += (inputs.flow_count as f64 * 0.05).min(0.25);
        } else {
            score += inputs.flow_criticalities.iter().sum::<f64>().min(0.25);
        }

        let caller_edges = inputs
            .inbound_edges
            .iter()
            .filter(|edge| edge.kind == "CALLS")
            .collect::<Vec<_>>();
        if let Some(node_cid) = inputs.node_community_id {
            let cross_community = caller_edges
                .iter()
                .filter(|edge| {
                    inputs
                        .caller_community_ids
                        .get(&edge.source_qualified)
                        .and_then(|cid| *cid)
                        .is_some_and(|cid| cid != node_cid)
                })
                .count();
            score += (cross_community as f64 * 0.05).min(0.15);
        }

        score += 0.30 - ((inputs.transitive_test_count as f64 / 5.0).min(1.0) * 0.25);

        let name_lower = inputs.node.name.to_lowercase();
        let qn_lower = inputs.node.qualified_name.to_lowercase();
        if SECURITY_KEYWORDS
            .iter()
            .any(|keyword| name_lower.contains(keyword) || qn_lower.contains(keyword))
        {
            score += 0.20;
        }

        score += (caller_edges.len() as f64 / 20.0).min(0.10);
        Ok((score.clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0)
    }

    fn get_flow_ids_by_node_ids(&self, node_ids: &HashSet<i64>) -> Result<Vec<i64>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let node_ids = node_ids.iter().copied().collect::<Vec<_>>();
        for chunk in node_ids.chunks(450) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT flow_id FROM flow_memberships WHERE node_id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| row.get(0))?;
            for row in rows {
                let flow_id = row?;
                if seen.insert(flow_id) {
                    out.push(flow_id);
                }
            }
        }
        Ok(out)
    }

    fn flow_steps_json(&self, path_ids: &[i64]) -> Result<Vec<Value>> {
        if path_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut nodes_by_id = HashMap::new();
        for chunk in path_ids.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM nodes WHERE id IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), node_from_row)?;
            for row in rows {
                let node = row?;
                nodes_by_id.insert(node.id, node);
            }
        }
        let mut steps = Vec::new();
        for node_id in path_ids {
            if let Some(node) = nodes_by_id.get(node_id) {
                steps.push(json!({
                    "node_id": node.id,
                    "name": sanitize_name(&node.name),
                    "kind": node.kind,
                    "file": node.file_path,
                    "line_start": node.line_start,
                    "line_end": node.line_end,
                    "qualified_name": sanitize_name(&node.qualified_name),
                }));
            }
        }
        Ok(steps)
    }

    pub fn get_edges_by_source(&self, qualified_name: &str) -> Result<Vec<GraphEdge>> {
        self.get_edges_by_endpoint("source_qualified", qualified_name)
    }

    pub fn get_edges_by_target(&self, qualified_name: &str) -> Result<Vec<GraphEdge>> {
        self.get_edges_by_endpoint("target_qualified", qualified_name)
    }

    pub fn get_edges_by_endpoints(
        &self,
        qualified_names: &[String],
    ) -> Result<(EdgeEndpointMap, EdgeEndpointMap)> {
        let mut outgoing = qualified_names
            .iter()
            .map(|qn| (qn.clone(), Vec::new()))
            .collect::<HashMap<_, _>>();
        let mut incoming = qualified_names
            .iter()
            .map(|qn| (qn.clone(), Vec::new()))
            .collect::<HashMap<_, _>>();
        if qualified_names.is_empty() {
            return Ok((outgoing, incoming));
        }

        let mut keys = HashSet::new();
        let mut normalized_to_originals: HashMap<String, Vec<String>> = HashMap::new();
        for qn in qualified_names {
            keys.insert(qn.clone());
            let normalized = self.normalize_qualified_key(qn)?;
            keys.insert(normalized.clone());
            normalized_to_originals
                .entry(normalized)
                .or_default()
                .push(qn.clone());
        }

        let mut seen_out = qualified_names
            .iter()
            .map(|qn| (qn.clone(), HashSet::new()))
            .collect::<HashMap<_, _>>();
        let mut seen_in = qualified_names
            .iter()
            .map(|qn| (qn.clone(), HashSet::new()))
            .collect::<HashMap<_, _>>();
        let keys = keys.into_iter().collect::<Vec<_>>();
        for chunk in keys.chunks(225) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT * FROM edges \
                 WHERE source_qualified IN ({placeholders}) \
                 OR target_qualified IN ({placeholders})"
            );
            let mut params = Vec::with_capacity(chunk.len() * 2);
            params.extend(chunk.iter().cloned());
            params.extend(chunk.iter().cloned());
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), edge_from_row)?;
            for row in rows {
                let edge = row?;
                let source_originals = normalized_to_originals
                    .get(&edge.source_qualified)
                    .cloned()
                    .unwrap_or_else(|| {
                        if outgoing.contains_key(&edge.source_qualified) {
                            vec![edge.source_qualified.clone()]
                        } else {
                            Vec::new()
                        }
                    });
                let target_originals = normalized_to_originals
                    .get(&edge.target_qualified)
                    .cloned()
                    .unwrap_or_else(|| {
                        if incoming.contains_key(&edge.target_qualified) {
                            vec![edge.target_qualified.clone()]
                        } else {
                            Vec::new()
                        }
                    });
                for original in source_originals {
                    if let Some(seen) = seen_out.get_mut(&original) {
                        if seen.insert(edge.id) {
                            outgoing.entry(original).or_default().push(edge.clone());
                        }
                    }
                }
                for original in target_originals {
                    if let Some(seen) = seen_in.get_mut(&original) {
                        if seen.insert(edge.id) {
                            incoming.entry(original).or_default().push(edge.clone());
                        }
                    }
                }
            }
        }
        Ok((outgoing, incoming))
    }

    pub fn get_direct_dependents(&self, file_paths: &[String]) -> Result<Vec<String>> {
        if file_paths.is_empty() {
            return Ok(Vec::new());
        }

        let mut dependents = HashSet::new();
        let mut fp_keys = Vec::new();
        let mut seen_keys = HashSet::new();
        for file_path in file_paths {
            for key in self.qualified_key_candidates(file_path)? {
                if seen_keys.insert(key.clone()) {
                    fp_keys.push(key);
                }
            }
        }

        for chunk in fp_keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT file_path FROM edges \
                 WHERE target_qualified IN ({placeholders}) AND kind = 'IMPORTS_FROM'"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                dependents.insert(row?);
            }
        }

        let file_keys = self.expand_file_keys(file_paths)?;
        let mut node_qns = Vec::new();
        for chunk in file_keys.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("SELECT qualified_name FROM nodes WHERE file_path IN ({placeholders})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows {
                node_qns.push(row?);
            }
        }

        if !node_qns.is_empty() {
            let (_, incoming) = self.get_edges_by_endpoints(&node_qns)?;
            for edges in incoming.values() {
                for edge in edges {
                    if matches!(
                        edge.kind.as_str(),
                        "CALLS" | "IMPORTS_FROM" | "INHERITS" | "IMPLEMENTS"
                    ) {
                        dependents.insert(edge.file_path.clone());
                    }
                }
            }
        }

        for file_path in file_paths {
            dependents.remove(file_path);
        }
        let mut out = dependents.into_iter().collect::<Vec<_>>();
        out.sort();
        Ok(out)
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
                10 => self.migrate_v10()?,
                11 => self.migrate_v11()?,
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

    fn migrate_v10(&self) -> Result<()> {
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_parent_name ON nodes(parent_name, name)",
            [],
        )?;
        Ok(())
    }

    fn migrate_v11(&self) -> Result<()> {
        if !has_column(&self.conn, "nodes", "mtime_ns")? {
            self.conn.execute(
                "ALTER TABLE nodes ADD COLUMN mtime_ns INTEGER DEFAULT 0",
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

fn flow_json_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let path_json: String = row.get("path_json")?;
    let path = serde_json::from_str::<Vec<i64>>(&path_json).unwrap_or_default();
    let name: String = row.get("name")?;
    Ok(json!({
        "id": row.get::<_, i64>("id")?,
        "name": sanitize_name(&name),
        "entry_point_id": row.get::<_, i64>("entry_point_id")?,
        "depth": row.get::<_, i64>("depth")?,
        "node_count": row.get::<_, i64>("node_count")?,
        "file_count": row.get::<_, i64>("file_count")?,
        "criticality": row.get::<_, f64>("criticality")?,
        "path": path,
        "created_at": row.get::<_, String>("created_at")?,
        "updated_at": row.get::<_, String>("updated_at")?,
    }))
}

fn community_json_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let name: String = row.get("name")?;
    let description = row
        .get::<_, Option<String>>("description")?
        .unwrap_or_default();
    Ok(json!({
        "id": row.get::<_, i64>("id")?,
        "name": sanitize_name(&name),
        "level": row.get::<_, i64>("level")?,
        "cohesion": row.get::<_, f64>("cohesion")?,
        "size": row.get::<_, i64>("size")?,
        "dominant_language": row.get::<_, Option<String>>("dominant_language")?.unwrap_or_default(),
        "description": sanitize_name(&description),
        "members": [],
    }))
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
             file_hash, mtime_ns, extra, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(qualified_name) DO UPDATE SET
            kind=excluded.kind, name=excluded.name,
            file_path=excluded.file_path, line_start=excluded.line_start,
            line_end=excluded.line_end, language=excluded.language,
            parent_name=excluded.parent_name, params=excluded.params,
            return_type=excluded.return_type, modifiers=excluded.modifiers,
            is_test=excluded.is_test, file_hash=excluded.file_hash,
            mtime_ns=excluded.mtime_ns, extra=excluded.extra, updated_at=excluded.updated_at
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

    for (file_path, nodes, edges, file_hash, mtime_ns) in batch {
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
                mtime_ns,
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

fn node_to_value(node: &GraphNode) -> Value {
    json!({
        "id": node.id,
        "kind": node.kind,
        "name": sanitize_name(&node.name),
        "qualified_name": sanitize_name(&node.qualified_name),
        "file_path": node.file_path,
        "line_start": node.line_start,
        "line_end": node.line_end,
        "language": node.language,
        "parent_name": node.parent_name.as_deref().map(sanitize_name),
        "is_test": node.is_test,
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

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| *ch == '\t' || *ch == '\n' || (*ch as u32) >= 0x20)
        .take(256)
        .collect()
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
            .store_file_nodes_edges("app.py", &[file, func], &[], "hash1", 0)
            .unwrap();
        store
            .store_file_nodes_edges("app.py", &[], &[], "hash2", 0)
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
                    0,
                ),
                (
                    "b.py".to_string(),
                    vec![file_b],
                    vec![],
                    "hash-b".to_string(),
                    0,
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
                        "hash",
                        123
                    ]
                ]"#,
            )
            .unwrap();

        assert_eq!(
            store.get_file_hashes(&["app.py".to_string()]).unwrap()["app.py"],
            "hash"
        );
        assert_eq!(
            store.get_file_meta_map().unwrap()["app.py"],
            ("hash".to_string(), 123)
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
            .store_file_nodes_edges("app.py", &[file, func], &[], "hash", 0)
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
            .store_file_nodes_edges("app.py", &[class, func], &[], "hash", 0)
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
                0,
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
                0,
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
        let test_callee = NodeInput {
            kind: "Test".to_string(),
            name: "test_callee".to_string(),
            file_path: "test_app.py".to_string(),
            line_start: 1,
            line_end: 5,
            language: "python".to_string(),
            parent_name: None,
            params: None,
            return_type: None,
            modifiers: None,
            is_test: true,
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
        let tested_by = EdgeInput {
            kind: "TESTED_BY".to_string(),
            source: "test_app.py::test_callee".to_string(),
            target: "app.py::callee".to_string(),
            file_path: "test_app.py".to_string(),
            line: 2,
            extra: Value::Object(Default::default()),
        };
        store
            .store_file_batch(&[(
                "app.py".to_string(),
                vec![entry, callee, test_callee],
                vec![edge, tested_by],
                "hash".to_string(),
                0,
            )])
            .unwrap();

        assert_eq!(
            store.get_files_matching("test_app.py").unwrap(),
            vec!["test_app.py"]
        );
        let targets = store.get_all_call_targets(false).unwrap();
        assert_eq!(targets, HashSet::from(["app.py::callee".to_string()]));
        let nodes = store
            .get_nodes_by_kind(&["Function".to_string()], None)
            .unwrap();
        assert_eq!(nodes.len(), 2);
        let stats = store.get_stats().unwrap();
        assert_eq!(stats.total_nodes, 3);
        assert_eq!(stats.total_edges, 2);
        assert_eq!(stats.nodes_by_kind["Function"], 2);
        assert_eq!(stats.nodes_by_kind["Test"], 1);
        assert_eq!(stats.edges_by_kind["CALLS"], 1);
        assert_eq!(stats.edges_by_kind["TESTED_BY"], 1);
        assert_eq!(stats.files_count, 0);
        assert_eq!(stats.languages, vec!["python".to_string()]);
        let (calls_out, tested_by) = store.get_flow_edge_data().unwrap();
        assert_eq!(calls_out["app.py::entry"], vec!["app.py::callee"]);
        assert_eq!(tested_by, HashSet::from(["app.py::callee".to_string()]));

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
        assert_eq!(store.count_flow_memberships(callee_id).unwrap(), 1);
        let nodes_by_id = store.get_nodes_by_ids(&[entry_id, callee_id]).unwrap();
        assert_eq!(nodes_by_id[&entry_id].qualified_name, "app.py::entry");
        assert_eq!(nodes_by_id[&callee_id].qualified_name, "app.py::callee");
        let nodes_by_qn = store
            .get_nodes_by_qualified_names(&[
                "app.py::entry".to_string(),
                "app.py::callee".to_string(),
                "missing.py::none".to_string(),
            ])
            .unwrap();
        assert_eq!(nodes_by_qn["app.py::entry"].id, entry_id);
        assert_eq!(nodes_by_qn["app.py::callee"].id, callee_id);
        assert!(!nodes_by_qn.contains_key("missing.py::none"));
        let membership_counts = store
            .count_flow_memberships_for_nodes(&[entry_id, callee_id])
            .unwrap();
        assert_eq!(membership_counts[&entry_id], 1);
        assert_eq!(membership_counts[&callee_id], 1);
        assert_eq!(
            store.get_flow_criticalities_for_node(callee_id).unwrap(),
            vec![0.25]
        );
        let flow_criticalities = store
            .get_flow_criticalities_for_nodes(&[entry_id, callee_id])
            .unwrap();
        assert_eq!(flow_criticalities[&entry_id], vec![0.25]);
        assert_eq!(flow_criticalities[&callee_id], vec![0.25]);
        assert_eq!(store.get_node_community_id(callee_id).unwrap(), None);
        let community_ids = store
            .get_community_ids_by_node_ids(&[entry_id, callee_id])
            .unwrap();
        assert_eq!(community_ids[&entry_id], None);
        assert_eq!(community_ids[&callee_id], None);
        let direct_tests = store.get_transitive_tests("app.py::callee", 1).unwrap();
        assert_eq!(direct_tests.len(), 1);
        assert_eq!(direct_tests[0]["name"], "test_callee");
        assert_eq!(direct_tests[0]["indirect"], false);
        let indirect_tests = store.get_transitive_tests("app.py::entry", 1).unwrap();
        assert_eq!(indirect_tests.len(), 1);
        assert_eq!(indirect_tests[0]["name"], "test_callee");
        assert_eq!(indirect_tests[0]["indirect"], true);
        assert_eq!(store.store_flows(&[]).unwrap(), 0);
        assert_eq!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM flows", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );

        assert_eq!(store.store_flows(&flows).unwrap(), 1);
        let flows_json: Vec<Value> =
            serde_json::from_str(&store.get_flows_json("criticality", 50).unwrap()).unwrap();
        assert_eq!(flows_json.len(), 1);
        assert_eq!(flows_json[0]["name"], "entry");
        let flow_id = flows_json[0]["id"].as_i64().unwrap();
        let flow_json: Value = serde_json::from_str(
            &store
                .get_flow_by_id_json(flow_id)
                .unwrap()
                .expect("flow exists"),
        )
        .unwrap();
        assert_eq!(flow_json["steps"].as_array().unwrap().len(), 2);
        let affected: Vec<Value> = serde_json::from_str(
            &store
                .get_affected_flows_json(&["app.py".to_string()])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(affected.len(), 1);
        let analysis: Value = serde_json::from_str(
            &store
                .analyze_changes_json(&["app.py".to_string()], None)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(analysis["risk_score"], json!(0.55));
        assert_eq!(analysis["changed_functions"].as_array().unwrap().len(), 2);
        assert_eq!(analysis["affected_flows"].as_array().unwrap().len(), 1);
        assert_eq!(analysis["test_gaps"].as_array().unwrap().len(), 1);
        let deleted_entry_points = store
            .delete_affected_flows(&["app.py".to_string()])
            .unwrap();
        assert_eq!(deleted_entry_points, vec![entry_id]);
        assert_eq!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM flows", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .insert_flows_json(&serde_json::to_string(&flows).unwrap())
                .unwrap(),
            1
        );
        assert!(store
            .delete_affected_flows(&["missing.py".to_string()])
            .unwrap()
            .is_empty());
        assert_eq!(
            store.get_node_kind_by_id(entry_id).unwrap().as_deref(),
            Some("Function")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stores_and_reads_communities() {
        let path = temp_db("communities");
        let mut store = GraphStore::open(&path).expect("open graph store");
        let node = NodeInput {
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
        store
            .store_file_batch(&[(
                "auth.py".to_string(),
                vec![node],
                vec![],
                "hash".to_string(),
                0,
            )])
            .unwrap();
        let payload = serde_json::to_string(&vec![CommunityInput {
            name: "auth-cluster".to_string(),
            level: 0,
            cohesion: 0.75,
            size: 1,
            dominant_language: "python".to_string(),
            description: "Auth functions".to_string(),
            members: vec!["auth.py::login".to_string()],
        }])
        .unwrap();

        assert_eq!(store.store_communities_json(&payload).unwrap(), 1);
        let communities: Vec<Value> =
            serde_json::from_str(&store.get_communities_json("size", 0).unwrap()).unwrap();
        assert_eq!(communities.len(), 1);
        assert_eq!(communities[0]["name"], "auth-cluster");
        assert_eq!(communities[0]["members"], json!(["auth.py::login"]));
        let community_id = communities[0]["id"].as_i64().unwrap();
        let members = store.get_nodes_by_community_id(community_id).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].qualified_name, "auth.py::login");
        let all_member_qns = store.get_all_community_member_qns().unwrap();
        assert_eq!(
            all_member_qns.get(&community_id),
            Some(&vec!["auth.py::login".to_string()])
        );
        let community_ids = store
            .get_community_ids_by_qualified_names(&[
                "auth.py::login".to_string(),
                "missing.py::none".to_string(),
            ])
            .unwrap();
        assert_eq!(
            community_ids.get("auth.py::login").copied().flatten(),
            Some(community_id)
        );
        assert!(!community_ids.contains_key("missing.py::none"));
        assert_eq!(
            store
                .count_affected_communities(&["auth.py".to_string()])
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .count_affected_communities(&["missing.py".to_string()])
                .unwrap(),
            0
        );
        let all_nodes = store.get_all_nodes_filtered(true).unwrap();
        assert_eq!(all_nodes.len(), 1);
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
                    0,
                ),
                (
                    "src/app.py".to_string(),
                    vec![target],
                    vec![edge.clone(), edge],
                    "hash-app".to_string(),
                    0,
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
