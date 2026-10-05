//! `dagayn.task_queue`'s producer side: [`enqueue`] (`TaskQueue.enqueue`,
//! coalescing into a pending twin) and [`ensure_worker`], which starts the
//! Python queue worker (`python -P -m dagayn queue run`) when none holds its
//! lock. The worker itself stays Python's.
//!
//! The queue database is a file of its own next to `graph.db`. Inside a
//! Python process, Python's `sqlite3` may hold it open too, and the two
//! SQLite copies cannot see each other's locks; this connection therefore
//! never checkpoints or deletes the WAL when it closes.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use rusqlite::config::DbConfig;
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::local_time::local_isoformat;

/// `QUEUE_DB_NAME`, in the graph's data directory.
pub const QUEUE_DB_NAME: &str = "task_queue.db";
/// `WORKER_LOCK_NAME`, in the graph's data directory.
pub const WORKER_LOCK_NAME: &str = "queue_worker.lock";
/// `str(DEFAULT_IDLE_SECONDS)`.
const DEFAULT_IDLE_SECONDS: &str = "60.0";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    payload TEXT NOT NULL DEFAULT '{}',
    state TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_error TEXT,
    not_before REAL NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS task_log (
    id INTEGER PRIMARY KEY,
    task_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    note TEXT,
    at TEXT NOT NULL
);
";

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("unknown task kind: {0}")]
    UnknownKind(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// A pending twin's payload this port cannot merge as Python would.
    #[error("cannot merge the pending payload: {0}")]
    Payload(String),
}

/// `DEFAULT_PRIORITIES`.
fn default_priority(kind: &str) -> Result<i64, QueueError> {
    match kind {
        "update" | "prepare" => Ok(10),
        "embed" | "postprocess" => Ok(0),
        _ => Err(QueueError::UnknownKind(kind.to_string())),
    }
}

/// A task payload: its keys in order, each with its `json.dumps` text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskPayload(Vec<(String, String)>);

impl TaskPayload {
    fn set(&mut self, key: &str, text: String) {
        match self.0.iter_mut().find(|(existing, _)| existing == key) {
            Some(slot) => slot.1 = text,
            None => self.0.push((key.to_string(), text)),
        }
    }

    fn remove(&mut self, key: &str) {
        self.0.retain(|(existing, _)| existing != key);
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, text)| text.as_str())
    }

    pub fn put_str(mut self, key: &str, value: &str) -> Self {
        self.set(key, py_string(value));
        self
    }

    pub fn put_bool(mut self, key: &str, value: bool) -> Self {
        self.set(key, if value { "true" } else { "false" }.to_string());
        self
    }

    /// An `int`, or `None` (`null`).
    pub fn put_int(mut self, key: &str, value: Option<i64>) -> Self {
        self.set(key, value.map_or("null".to_string(), |v| v.to_string()));
        self
    }

    /// `json.dumps(payload)`.
    pub fn dumps(&self) -> String {
        let fields: Vec<String> = self
            .0
            .iter()
            .map(|(key, text)| format!("{}: {text}", py_string(key)))
            .collect();
        format!("{{{}}}", fields.join(", "))
    }

    /// `json.loads(text)` of a stored payload, values kept as written.
    fn loads(text: &str) -> Result<Self, QueueError> {
        let ordered: Ordered =
            serde_json::from_str(text).map_err(|err| QueueError::Payload(err.to_string()))?;
        Ok(Self(
            ordered
                .0
                .into_iter()
                .map(|(key, raw)| (key, raw.get().to_string()))
                .collect(),
        ))
    }
}

/// A JSON object's entries in document order; a repeated key keeps its first
/// position and its last value, as a `dict` does.
struct Ordered(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;
        impl<'de> Visitor<'de> for Entries {
            type Value = Ordered;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut out: Vec<(String, Box<RawValue>)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    match out.iter_mut().find(|(existing, _)| *existing == key) {
                        Some(slot) => slot.1 = value,
                        None => out.push((key, value)),
                    }
                }
                Ok(Ordered(out))
            }
        }
        deserializer.deserialize_map(Entries)
    }
}

/// `json.dumps(text)` (`ensure_ascii=True`).
fn py_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut units = [0_u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Python's truthiness of a stored JSON value (absent is false).
fn truthy(text: Option<&str>) -> Result<bool, QueueError> {
    let Some(text) = text else {
        return Ok(false);
    };
    let value: Value =
        serde_json::from_str(text).map_err(|err| QueueError::Payload(err.to_string()))?;
    Ok(match value {
        Value::Null => false,
        Value::Bool(flag) => flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    })
}

/// The stored value as a list of `str(path)`, when it is a list.
fn path_list(text: Option<&str>) -> Result<Option<Vec<String>>, QueueError> {
    let Some(text) = text else {
        return Ok(None);
    };
    let value: Value =
        serde_json::from_str(text).map_err(|err| QueueError::Payload(err.to_string()))?;
    let Value::Array(items) = value else {
        return Ok(None);
    };
    items
        .into_iter()
        .map(|item| match item {
            Value::String(text) => Ok(text),
            Value::Bool(flag) => Ok(if flag { "True" } else { "False" }.to_string()),
            Value::Null => Ok("None".to_string()),
            Value::Number(number) if number.is_i64() || number.is_u64() => Ok(number.to_string()),
            other => Err(QueueError::Payload(format!("str() of {other}"))),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// `_merge_payloads`: newer keys win; for `embed`, `skip_structure` survives
/// only when both carry it, and `files` only when both are lists (unioned).
fn merge_payloads(
    kind: &str,
    old: &TaskPayload,
    new: &TaskPayload,
) -> Result<TaskPayload, QueueError> {
    let mut merged = old.clone();
    for (key, text) in &new.0 {
        merged.set(key, text.clone());
    }
    if kind != "embed" {
        return Ok(merged);
    }
    if !(truthy(old.get("skip_structure"))? && truthy(new.get("skip_structure"))?) {
        merged.remove("skip_structure");
    }
    match (path_list(old.get("files"))?, path_list(new.get("files"))?) {
        (Some(old_files), Some(new_files)) => {
            let mut files: Vec<String> = old_files.into_iter().chain(new_files).collect();
            files.sort();
            files.dedup();
            let items: Vec<String> = files.iter().map(|path| py_string(path)).collect();
            merged.set("files", format!("[{}]", items.join(", ")));
        }
        _ => merged.remove("files"),
    }
    Ok(merged)
}

/// `TaskQueue(db_path)`: the directory, WAL, the schema, and `not_before`.
fn open_queue(db_path: &Path) -> Result<Connection, QueueError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    conn.execute_batch(SCHEMA)?;
    let has_not_before = conn
        .prepare("PRAGMA table_info(tasks)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == "not_before");
    if !has_not_before {
        conn.execute(
            "ALTER TABLE tasks ADD COLUMN not_before REAL NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(conn)
}

/// `TaskQueue(db_path).enqueue(kind, payload)`: `("added" | "coalesced",
/// task id)`, under one `BEGIN IMMEDIATE` so a worker cannot claim the twin
/// between the lookup and the write.
pub fn enqueue(
    db_path: &Path,
    kind: &str,
    payload: &TaskPayload,
) -> Result<(&'static str, i64), QueueError> {
    let priority = default_priority(kind)?;
    let conn = open_queue(db_path)?;
    let now = local_isoformat();
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<(&'static str, i64), QueueError> {
        let twin: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, payload, priority FROM tasks WHERE kind = ? AND state = 'pending'",
                [kind],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((id, stored)) = twin {
            let merged = merge_payloads(kind, &TaskPayload::loads(&stored)?, payload)?;
            conn.execute(
                "UPDATE tasks SET payload = ?, priority = MAX(priority, ?), updated_at = ? \
                 WHERE id = ?",
                params![merged.dumps(), priority, now, id],
            )?;
            conn.execute_batch("COMMIT")?;
            return Ok(("coalesced", id));
        }
        conn.execute(
            "INSERT INTO tasks (kind, priority, payload, state, created_at, updated_at) \
             VALUES (?, ?, ?, 'pending', ?, ?)",
            params![kind, priority, payload.dumps(), now, now],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute_batch("COMMIT")?;
        Ok(("added", id))
    })();
    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    result
}

/// `ensure_worker(repo_root)`: start `python -P -m dagayn queue run` in a
/// session of its own, with `pythonpath` (the parent of the `dagayn`
/// package) first on `PYTHONPATH`, unless a live worker holds the lock in
/// `data_dir`. True when one was started.
pub fn ensure_worker(data_dir: &Path, repo_root: &Path, python: &Path, pythonpath: &Path) -> bool {
    // `WorkerLock(...).acquire()`, then `release()`: the probe records this
    // process's pid in the lock file, as Python's does.
    if std::fs::create_dir_all(data_dir).is_err() {
        return false;
    }
    let Ok(mut lock) = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(data_dir.join(WORKER_LOCK_NAME))
    else {
        return false;
    };
    if lock.try_lock().is_err() {
        return false;
    }
    let _ = lock
        .set_len(0)
        .and_then(|()| writeln!(lock, "{}", std::process::id()))
        .and_then(|()| lock.flush());
    let _ = lock.unlock();
    drop(lock);

    let mut path = pythonpath.as_os_str().to_os_string();
    if let Some(existing) = std::env::var_os("PYTHONPATH").filter(|value| !value.is_empty()) {
        path.push(":");
        path.push(existing);
    }
    let mut command = Command::new(python);
    command
        .args(["-P", "-m", "dagayn", "queue", "run", "--repo"])
        .arg(repo_root)
        .args(["--idle-seconds", DEFAULT_IDLE_SECONDS])
        .env("PYTHONPATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // `close_fds=True`: the worker outlives this call, so it must not
        // keep a descriptor this process inherited or made without
        // `CLOEXEC` (the MCP session's stdio, say) open.
        // SAFETY: `sysconf` only reads a limit.
        let open_max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
        let last_fd = libc::c_int::try_from(open_max.clamp(3, 65_536)).unwrap_or(65_536);
        // SAFETY: `setsid`, `fcntl`, and `close` are async-signal-safe and
        // touch nothing but the child's own session and descriptors. Those
        // already `CLOEXEC` (std's exec-error pipe among them) close on exec.
        unsafe {
            command.pre_exec(move || {
                libc::setsid();
                for fd in 3..last_fd {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                        libc::close(fd);
                    }
                }
                Ok(())
            });
        }
    }
    match command.spawn() {
        Ok(mut child) => {
            // Reap it whenever it exits, as `subprocess` does for a dropped
            // `Popen`.
            std::thread::spawn(move || child.wait());
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "dagayn-queue-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn payloads_dump_as_python_does() {
        let payload = TaskPayload::default()
            .put_str("local_embedding", "bge-m3")
            .put_bool("keep_local_embedding_server", true)
            .put_int("budget_seconds", None);
        assert_eq!(
            payload.dumps(),
            r#"{"local_embedding": "bge-m3", "keep_local_embedding_server": true, "budget_seconds": null}"#
        );
        assert_eq!(TaskPayload::default().dumps(), "{}");
        assert_eq!(
            py_string("\u{e9}\"\u{1}\u{1f600}"),
            r#""\u00e9\"\u0001\ud83d\ude00""#
        );
    }

    #[test]
    fn merging_keeps_old_order_and_unions_embed_files() {
        let old = TaskPayload::loads(r#"{"files": ["b", "a"], "skip_structure": true, "x": 1}"#)
            .expect("old");
        let new = TaskPayload::default().put_str("x", "y");
        let merged = merge_payloads("prepare", &old, &new).expect("merge");
        assert_eq!(
            merged.dumps(),
            r#"{"files": ["b", "a"], "skip_structure": true, "x": "y"}"#
        );
        let new = TaskPayload::loads(r#"{"files": ["c", "a"], "skip_structure": 1}"#).expect("new");
        let merged = merge_payloads("embed", &old, &new).expect("merge");
        assert_eq!(
            merged.dumps(),
            r#"{"files": ["a", "b", "c"], "skip_structure": 1, "x": 1}"#
        );
        let whole = TaskPayload::default().put_bool("keep_local_embedding_server", true);
        let merged = merge_payloads("embed", &old, &whole).expect("merge");
        assert_eq!(
            merged.dumps(),
            r#"{"x": 1, "keep_local_embedding_server": true}"#
        );
    }

    #[test]
    fn enqueue_adds_then_coalesces() {
        let dir = temp_dir("enqueue");
        let db = dir.join(".dagayn").join(QUEUE_DB_NAME);
        let payload = TaskPayload::default().put_str("local_embedding", "none");
        assert_eq!(
            enqueue(&db, "prepare", &payload).expect("add"),
            ("added", 1)
        );
        assert_eq!(
            enqueue(&db, "prepare", &payload).expect("coalesce"),
            ("coalesced", 1)
        );
        assert_eq!(enqueue(&db, "embed", &payload).expect("add"), ("added", 2));
        assert!(matches!(
            enqueue(&db, "nope", &payload),
            Err(QueueError::UnknownKind(_))
        ));
        let conn = Connection::open(&db).expect("open");
        let (priority, stored): (i64, String) = conn
            .query_row(
                "SELECT priority, payload FROM tasks WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("row");
        assert_eq!(priority, 10);
        assert_eq!(stored, r#"{"local_embedding": "none"}"#);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_held_lock_means_a_live_worker() {
        let dir = temp_dir("lock");
        let held = OpenOptions::new()
            .append(true)
            .create(true)
            .open(dir.join(WORKER_LOCK_NAME))
            .expect("lock file");
        held.lock().expect("lock");
        assert!(!ensure_worker(
            &dir,
            &dir,
            Path::new("/nonexistent/python"),
            &dir
        ));
        drop(held);
        // Free, the probe takes the lock; the interpreter then fails to start.
        assert!(!ensure_worker(
            &dir,
            &dir,
            Path::new("/nonexistent/python"),
            &dir
        ));
        let pid = std::fs::read_to_string(dir.join(WORKER_LOCK_NAME)).expect("pid");
        assert_eq!(pid, format!("{}\n", std::process::id()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
