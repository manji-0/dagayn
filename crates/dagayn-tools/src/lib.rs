//! MCP tools answered in Rust, before the `dagayn serve` front end hands a
//! call to the Python server.
//!
//! [`call`] returns `None` for anything it does not answer exactly as the
//! Python tool would: an argument fastmcp would coerce or reject, a repository
//! it would auto-detect, a graph it would create, migrate, or refuse. The
//! front end then relays the call, so every error stays Python's.

mod answerability;
mod arch_tool;
mod architecture;
mod changes;
mod context;
mod coverage;
mod docs;
mod flow;
pub mod hints;
mod pyrandom;
mod query;
mod review;
mod review_summary;
mod search;
mod source;
mod stats;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dagayn_build::{GraphLock, LockMode};
use dagayn_graph::GraphStore;
use serde_json::{Map, Value, json};

/// What the session knows beyond a call's arguments.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// `dagayn serve --repo`, already resolved.
    pub pinned_repo: Option<PathBuf>,
    /// Tool names the session exposes; `None` exposes every tool.
    pub allowed_tools: Option<HashSet<String>>,
    /// The directory holding the packaged `docs/` (the parent of `dagayn/`).
    pub package_root: Option<PathBuf>,
    /// `dagayn serve`'s local embedding default (`--local-embedding`, or the
    /// one inferred from the graph); `None` or `"none"` when off.
    pub local_embedding: Option<String>,
    /// `dagayn serve`'s default embedding provider and model for search
    /// (`--remote-embedding`, or a local sidecar's), when it has one.
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    /// `tool_runtime_summary()` of the hosting Python process, which tools
    /// that attach `_runtime` need.
    pub runtime: Option<Value>,
}

/// A tool result: its JSON text with the Python tool's top-level key order,
/// and the same value for `structuredContent`.
#[derive(Debug, PartialEq)]
pub struct Payload {
    pub text: String,
    pub value: Value,
}

/// Answer `name(arguments)`, or `None` to leave it to the Python server.
pub fn call(context: &Context, name: &str, arguments: &Value) -> Option<Payload> {
    let arguments = arguments.as_object()?;
    match name {
        "list_graph_stats_tool" => stats::list_graph_stats(context, arguments),
        "get_docs_section_tool" => docs::get_docs_section(context, arguments),
        "get_minimal_context_tool" => context::get_minimal_context(context, arguments),
        "query_graph_tool" => query::query_graph(context, arguments),
        "semantic_search_nodes_tool" => search::semantic_search(context, arguments),
        "review_tool" => review::review(context, arguments),
        "flow_tool" => flow::flow(context, arguments),
        "architecture_analysis_tool" => arch_tool::architecture(context, arguments),
        _ => None,
    }
}

/// `_with_dispatch_metadata` and `seal_dispatcher_ok` for a mode-based tool
/// whose subtool answered `out`: `mode`, `called_subtool`, and `_runtime`
/// added, the dispatcher's hints built (and recorded) even where the
/// subtool's stay, the envelope's fields first; then the server's `_repo`.
/// `trailing` is what `attach_answerability` appends after `_runtime` for a
/// subtool that reported no answerability of its own.
pub(crate) struct Dispatch<'a> {
    pub mode: &'a str,
    pub subtool: &'a str,
    /// The `generate_hints` tool name the dispatcher reports.
    pub hints_tool: &'a str,
    pub runtime: Value,
    pub trailing: Vec<(&'a str, Value)>,
    /// `_repo`.
    pub repo: Value,
}

pub(crate) fn seal_dispatch(
    out: Ordered,
    dispatch: Dispatch,
    exposed: &dyn Fn(&str) -> bool,
) -> Payload {
    let Dispatch {
        mode,
        subtool,
        hints_tool,
        runtime,
        trailing,
        repo,
    } = dispatch;
    let mut seen = out.value();
    if let Some(object) = seen.as_object_mut() {
        object.insert("mode".to_string(), json!(mode));
        object.insert("called_subtool".to_string(), json!(subtool));
        object.insert("_runtime".to_string(), runtime.clone());
        for (key, value) in &trailing {
            object.insert(key.to_string(), value.clone());
        }
    }
    let dispatcher_hints = hints::generate_hints(hints_tool, &seen, &mut hints::session(), exposed);
    let has_hints = out.get("_hints").is_some();
    let field = |key: &str| seen.get(key).cloned().unwrap_or(Value::Null);
    let mut sealed = Ordered::default()
        .put("status", field("status"))
        .put("mode", mode)
        .put("called_subtool", subtool)
        .put("summary", field("summary"));
    for (key, value) in out.into_entries() {
        if !matches!(key.as_str(), "status" | "summary") {
            sealed = sealed.put(&key, value);
        }
    }
    sealed = sealed.put("_runtime", runtime);
    for (key, value) in trailing {
        sealed = sealed.put(key, value);
    }
    if !has_hints {
        sealed = sealed.put("_hints", dispatcher_hints);
    }
    sealed.put("_repo", repo).into_payload()
}

/// A JSON object that keeps its keys in insertion order, as Python's dicts
/// do; `serde_json`'s map sorts them.
#[derive(Default)]
pub(crate) struct Ordered(Vec<(String, Value)>);

impl Ordered {
    pub(crate) fn put(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.0.push((key.to_string(), value.into()));
        self
    }

    /// Replace a key's value in place, keeping its position.
    pub(crate) fn replace(mut self, key: &str, value: Value) -> Self {
        if let Some(slot) = self.0.iter_mut().find(|(existing, _)| existing == key) {
            slot.1 = value;
        }
        self
    }

    /// `apply_output_budget`: halve the listed lists, lowest priority (last)
    /// first, until `json.dumps` of the object is at most `budget_tokens`
    /// tokens (four characters each).
    pub(crate) fn apply_output_budget(mut self, budget_tokens: usize, priorities: &[&str]) -> Self {
        let over =
            |object: &Self| crate::query::python_dumps_len(&object.value()) / 4 > budget_tokens;
        if !over(&self) {
            return self;
        }
        let mut truncation = Vec::new();
        for field in priorities.iter().rev() {
            // `_get_path`: `parent.key` names a list inside a top-level object.
            let (top, nested) = match field.split_once('.') {
                Some((top, key)) => (top, Some(key)),
                None => (*field, None),
            };
            let Some(index) = self.0.iter().position(|(key, _)| key == top) else {
                continue;
            };
            let Some(total) = list_at(&mut self.0[index].1, nested).map(|items| items.len()) else {
                continue;
            };
            let mut kept = total;
            while kept > 1 && over(&self) {
                kept /= 2;
                if let Some(items) = list_at(&mut self.0[index].1, nested) {
                    items.truncate(kept);
                }
            }
            if kept < total {
                truncation.push((field.to_string(), json!({"kept": kept, "total": total})));
                self = self.set("truncated", json!(true));
            }
            if !over(&self) {
                break;
            }
        }
        if !truncation.is_empty() {
            let record: Map<String, Value> = truncation.into_iter().collect();
            self = self.set("_truncation", Value::Object(record));
        } else if over(&self) {
            self = self.set("truncated", json!(true));
        }
        self
    }

    /// Python's `object[key] = value`: in place, or appended.
    pub(crate) fn set(self, key: &str, value: Value) -> Self {
        if self.0.iter().any(|(existing, _)| existing == key) {
            self.replace(key, value)
        } else {
            self.put(key, value)
        }
    }

    /// A key's value, if it has one.
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The keys and values, in order.
    pub(crate) fn into_entries(self) -> Vec<(String, Value)> {
        self.0
    }

    /// The object so far (key order aside).
    pub(crate) fn value(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(key, item)| (key.clone(), item.clone()))
                .collect(),
        )
    }

    pub(crate) fn into_payload(self) -> Payload {
        let mut text = String::from("{");
        let mut value = Map::new();
        for (index, (key, item)) in self.0.into_iter().enumerate() {
            if index > 0 {
                text.push(',');
            }
            text.push_str(&json!(key).to_string());
            text.push(':');
            text.push_str(&item.to_string());
            value.insert(key, item);
        }
        text.push('}');
        Payload {
            text,
            value: Value::Object(value),
        }
    }
}

/// The list at `entry` (or at its `nested` key), as `_get_path` finds it.
fn list_at<'a>(entry: &'a mut Value, nested: Option<&str>) -> Option<&'a mut Vec<Value>> {
    let target = match nested {
        Some(key) => entry.as_object_mut()?.get_mut(key)?,
        None => entry,
    };
    target.as_array_mut()
}

/// The arguments of a call when they are exactly the declared ones, each a
/// plain JSON value of its declared type; fastmcp handles anything it would
/// coerce or reject.
pub(crate) struct Args<'a>(&'a Map<String, Value>);

impl<'a> Args<'a> {
    pub(crate) fn new(arguments: &'a Map<String, Value>, declared: &[&str]) -> Option<Self> {
        arguments
            .keys()
            .all(|key| declared.contains(&key.as_str()))
            .then_some(Self(arguments))
    }

    /// `str`, required.
    pub(crate) fn string(&self, key: &str) -> Option<&'a str> {
        self.0.get(key)?.as_str()
    }

    /// `Optional[str]`: `Some(None)` when absent or null.
    pub(crate) fn optional_string(&self, key: &str) -> Option<Option<&'a str>> {
        match self.0.get(key) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(value)) => Some(Some(value)),
            Some(_) => None,
        }
    }

    /// `int` with a default; a JSON integer only.
    pub(crate) fn integer(&self, key: &str, default: i64) -> Option<i64> {
        match self.0.get(key) {
            None => Some(default),
            Some(value) if value.is_i64() => value.as_i64(),
            Some(_) => None,
        }
    }
}

/// `dagayn.incremental_files.is_unresolved_path_placeholder`.
fn is_placeholder(value: &str) -> bool {
    let value = value.trim();
    value.len() > 3
        && value.starts_with("${")
        && value.ends_with('}')
        && !value[2..value.len() - 1].contains('}')
}

/// `dagayn.server.main._resolve_repo_root` followed by `_validate_repo_root`:
/// the client's `repo_root`, else the pinned one, resolved. `None` when
/// neither is given (Python auto-detects) or the root is not one Python would
/// accept without a message.
pub(crate) fn explicit_repo(context: &Context, requested: Option<&str>) -> Option<PathBuf> {
    let requested = requested.filter(|value| !value.is_empty() && !is_placeholder(value));
    let candidate = match requested {
        Some(value) => PathBuf::from(value),
        None => context.pinned_repo.clone()?,
    };
    let resolved = candidate.canonicalize().ok().filter(|path| path.is_dir())?;
    let is_project_root = resolved.join(".git").exists()
        || resolved.join(".svn").exists()
        || resolved.join(".dagayn").join("graph.db").is_file();
    is_project_root.then_some(resolved)
}

/// An open graph under the shared read lock, as `_get_store` leaves it.
pub(crate) struct OpenGraph {
    pub root: PathBuf,
    pub db_path: PathBuf,
    pub store: GraphStore,
    _lock: GraphLock,
}

impl OpenGraph {
    /// `_repo`, as `attach_repo_context` adds it for an explicit root.
    pub(crate) fn repo_context(&self) -> Value {
        json!({
            "repo_root": self.root.to_string_lossy(),
            "db_path": self.db_path.to_string_lossy(),
            "source": "explicit",
        })
    }
}

/// `_get_store` for an explicit root, when it would open an existing graph in
/// the default location that describes this repository; `None` whenever
/// Python would create, migrate, relocate, or refuse it.
pub(crate) fn open_graph(root: &Path) -> Option<OpenGraph> {
    if std::env::var_os("CRG_DATA_DIR").is_some_and(|value| !value.is_empty()) {
        return None;
    }
    let legacy = ["", "-wal", "-shm", "-journal"]
        .iter()
        .any(|suffix| root.join(format!(".dagayn.db{suffix}")).exists());
    if legacy {
        return None;
    }
    if !root.join(".dagayn").join("graph.db").is_file() {
        return None;
    }
    // `get_db_path`: the same path, and the inner `.gitignore` written if
    // it went missing.
    let db_path = dagayn_build::db_path_for_build(root).ok()?;
    // No wait: this runs on the front end's reader, and a graph being written
    // is Python's to wait for (`DAGAYN_READ_LOCK_TIMEOUT`) while pings and
    // listings stay answered.
    let lock = GraphLock::acquire_mode(&db_path, LockMode::Shared, Some(Duration::ZERO)).ok()?;
    // Read-only: see `GraphStore::open_read_only` for why this process must
    // not close a writable connection. A graph Python would migrate is its.
    let store = GraphStore::open_read_only(&db_path).ok()?;
    if dagayn_build::graph_repo_mismatch(&store, root).is_some() {
        return None;
    }
    Some(OpenGraph {
        root: root.to_path_buf(),
        db_path,
        store,
        _lock: lock,
    })
}

/// `dagayn.tool_surface.suggestion_is_callable`.
fn suggestion_is_callable(context: &Context, suggestion: &str) -> bool {
    let Some(allowed) = &context.allowed_tools else {
        return true;
    };
    let mut stripped = suggestion.trim();
    if let Some(rest) = stripped.strip_prefix("Run:") {
        stripped = rest.trim();
    }
    let tool = suggestion_tool(stripped);
    tool == "dagayn" || !tool.ends_with("_tool") || allowed.contains(tool)
}

fn suggestion_tool(suggestion: &str) -> &str {
    let head = suggestion.split(" -- ").next().unwrap_or(suggestion);
    let head = head.split(' ').next().unwrap_or(head);
    head.split('(').next().unwrap_or(head)
}

/// `_hints` and `next_tool_suggestions` as `make_response` builds them from
/// plain suggestions, filtered to the session's tools.
pub(crate) fn suggestions(context: &Context, all: &[&str]) -> (Value, Value) {
    let kept: Vec<&str> = all
        .iter()
        .copied()
        .filter(|suggestion| suggestion_is_callable(context, suggestion))
        .take(3)
        .collect();
    let next_steps: Vec<Value> = kept
        .iter()
        .map(|suggestion| json!({"tool": suggestion_tool(suggestion), "suggestion": suggestion}))
        .collect();
    (
        json!({"next_steps": next_steps, "related": [], "warnings": []}),
        json!(kept),
    )
}

#[cfg(test)]
mod tests;
