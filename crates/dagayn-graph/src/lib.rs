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

pub struct GraphStore {
    conn: Connection,
}

pub type FileBatchItem = (String, Vec<NodeInput>, Vec<EdgeInput>, String);

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
