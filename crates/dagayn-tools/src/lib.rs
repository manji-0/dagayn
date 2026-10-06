//! MCP tools answered in Rust, before the `dagayn serve` front end hands a
//! call to the Python server.
//!
//! [`call`] returns `None` for anything it does not answer exactly as the
//! Python tool would: an argument fastmcp would coerce or reject, a repository
//! it would auto-detect, a graph it would create, migrate, or refuse. The
//! front end then relays the call, so every error stays Python's.

mod analysis;
mod answerability;
mod apply;
mod arch_tool;
mod architecture;
mod base_symbols;
mod changes;
mod community;
mod context;
mod coverage;
mod dead_code;
mod dead_code_verify;
mod difflib;
mod docs;
mod embedding_arm;
mod ensure;
mod findings;
mod flow;
pub mod hints;
mod large;
pub mod pending;
mod postprocess;
mod pypath;
mod pyrandom;
mod pyunicode;
mod query;
mod questions;
mod refactor;
mod repos;
mod review;
mod review_summary;
mod search;
mod source;
mod stats;
mod suggestions;
mod traverse;
mod units;

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
    /// `get_minimal_context(auto_prepare=True)`, as the MCP tool runs it:
    /// queue a repair for a graph that needs one. `None` never queues.
    pub auto_prepare: Option<AutoPrepare>,
}

/// How `get_minimal_context_tool` queues a repair.
#[derive(Clone, Debug, Default)]
pub struct AutoPrepare {
    /// `sys.executable` of the hosting Python, which runs the queue worker
    /// (`python -P -m dagayn queue run`, with `package_root` on
    /// `PYTHONPATH`); `None` leaves a call that would start one to Python.
    pub python_executable: Option<PathBuf>,
    /// `prepare_budget_seconds`, stored on a queued prepare.
    pub budget_seconds: Option<i64>,
}

impl Context {
    /// Whether the session exposes `tool`.
    pub(crate) fn exposes(&self, tool: &str) -> bool {
        self.allowed_tools
            .as_ref()
            .is_none_or(|allowed| allowed.contains(tool))
    }
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
        "refactor_tool" => refactor::refactor(context, arguments),
        "ensure_graph_tool" => ensure::ensure_graph(context, arguments),
        "find_large_functions_tool" => large::find_large_functions(context, arguments),
        "traverse_graph_tool" => traverse::traverse_graph(context, arguments),
        "get_suggested_questions_tool" => questions::suggested_questions(context, arguments),
        "get_wiki_page_tool" => docs::get_wiki_page(context, arguments),
        "list_repos_tool" => repos::list_repos(context, arguments),
        "apply_refactor_tool" => apply::apply_refactor(context, arguments),
        "run_postprocess_tool" => postprocess::run_postprocess(context, arguments),
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

/// `attach_answerability` on a request a dispatcher rejected: `head`, then
/// `_runtime`, the graph's full answerability and its missingness, and
/// `_repo`, as the sealed error responses carry them.
pub(crate) fn request_error(context: &Context, root: &RepoRoot, head: Ordered) -> Option<Payload> {
    let runtime = context.runtime.clone()?;
    let graph = open_graph(root)?;
    let answerability = graph.answerability()?;
    Some(
        head.put("_runtime", runtime)
            .put("answerability", answerability.full())
            .put("missingness", Value::Array(answerability.missingness()))
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}

/// `_error` in the flow and architecture dispatchers.
pub(crate) fn dispatcher_error(
    context: &Context,
    root: &RepoRoot,
    mode: &str,
    message: &str,
) -> Option<Payload> {
    request_error(
        context,
        root,
        Ordered::default()
            .put("status", "error")
            .put("summary", message)
            .put("error", message)
            .put("mode", mode)
            .put("called_subtool", Value::Null),
    )
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
pub(crate) fn is_placeholder(value: &str) -> bool {
    let value = value.trim();
    value.len() > 3
        && value.starts_with("${")
        && value.ends_with('}')
        && !value[2..value.len() - 1].contains('}')
}

/// The repository a tool call resolved to, and whether it was named (the
/// client's `repo_root` or the server's `--repo`) or auto-detected, as
/// `_repo.source` reports it.
pub(crate) struct RepoRoot {
    pub path: PathBuf,
    pub explicit: bool,
}

impl RepoRoot {
    /// The root as a tool that resolves it first and passes it on as a
    /// string sees it (`session_prepare._resolve_repo`, then
    /// `_get_store(str(root))`): validated like a named root, and reported
    /// as explicit.
    pub(crate) fn into_explicit(self) -> Option<RepoRoot> {
        is_project_root(&self.path).then_some(RepoRoot {
            path: self.path,
            explicit: true,
        })
    }
}

/// `_validate_repo_root`'s check for a project root (`is_project_root`).
fn is_project_root(path: &Path) -> bool {
    path.join(".git").exists()
        || path.join(".svn").exists()
        || dagayn_build::jj::is_jj_workspace(path)
        || path.join(".dagayn").join("graph.db").is_file()
}

impl AsRef<Path> for RepoRoot {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for RepoRoot {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

/// `dagayn.server.main._resolve_repo_root` followed by `_get_store`'s root
/// resolution: the client's `repo_root`, else the pinned one, validated as
/// `_validate_repo_root` does; else `find_project_root()` from the working
/// directory, refused (left to Python, which explains) when that lands on
/// the home directory or the filesystem root. `None` also when the root is
/// not one Python would accept without a message, or only Python resolves it.
pub(crate) fn resolve_repo(context: &Context, requested: Option<&str>) -> Option<RepoRoot> {
    let requested = requested.filter(|value| !value.is_empty() && !is_placeholder(value));
    let pinned = context
        .pinned_repo
        .as_ref()
        .filter(|path| !path.to_str().is_some_and(is_placeholder));
    let candidate = match (requested, pinned) {
        (Some(value), _) => PathBuf::from(value),
        (None, Some(path)) => path.clone(),
        (None, None) => {
            let cwd = std::env::current_dir().ok()?;
            let dagayn_build::ProjectRoot::Found(root) = dagayn_build::find_project_root(&cwd)
            else {
                return None;
            };
            if dagayn_build::unsafe_root_reason(&root).is_some() {
                return None;
            }
            let path = root.canonicalize().ok()?;
            return Some(RepoRoot {
                path,
                explicit: false,
            });
        }
    };
    let resolved = candidate.canonicalize().ok().filter(|path| path.is_dir())?;
    is_project_root(&resolved).then_some(RepoRoot {
        path: resolved,
        explicit: true,
    })
}

/// `get_db_path` for a graph that already exists where Python looks for it
/// (`<root>/.dagayn`, or this checkout's `CRG_DATA_DIR` subdirectory);
/// `None` when Python would create, migrate, or relocate it, a legacy
/// `.dagayn.db` included (it moves or deletes those files).
fn existing_graph(root: &Path) -> Option<PathBuf> {
    let legacy = ["", "-wal", "-shm", "-journal"]
        .iter()
        .any(|suffix| root.join(format!(".dagayn.db{suffix}")).exists());
    if legacy {
        return None;
    }
    dagayn_build::existing_db_path(root)
}

/// The longest [`open_graph`] waits for a writer to finish.
const MAX_READ_LOCK_WAIT: Duration = Duration::from_secs(3);

/// `DAGAYN_READ_LOCK_TIMEOUT` (Python's default 10 s), capped at
/// [`MAX_READ_LOCK_WAIT`].
fn read_lock_wait() -> Duration {
    let seconds = std::env::var("DAGAYN_READ_LOCK_TIMEOUT")
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite())
        .unwrap_or(10.0)
        .max(0.0);
    Duration::from_secs_f64(seconds).min(MAX_READ_LOCK_WAIT)
}

/// An open graph under its lock: the shared read lock, as `_get_store`
/// leaves it, or the exclusive one, as `run_postprocess` holds it.
pub(crate) struct OpenGraph {
    pub root: PathBuf,
    explicit: bool,
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
            "source": if self.explicit { "explicit" } else { "auto" },
        })
    }

    /// `Answerability::recorded` for the graph's current stats.
    pub(crate) fn answerability(&self) -> Option<answerability::Answerability> {
        let stats = self.store.get_stats().ok()?;
        answerability::Answerability::recorded(&self.store, &stats)
    }
}

/// `_get_store` for a resolved root, when it would open an existing graph
/// (in `.dagayn` or under `CRG_DATA_DIR`) that describes this repository;
/// `None` whenever Python would create, migrate, relocate, or refuse it.
pub(crate) fn open_graph(root: &RepoRoot) -> Option<OpenGraph> {
    let db_path = existing_graph(root)?;
    // Wait out an ordinary write (an edit's queued update holds the lock for
    // one to two seconds), but not Python's whole `DAGAYN_READ_LOCK_TIMEOUT`:
    // this runs on the front end's reader, and a graph still busy after this
    // goes to Python, which waits its own timeout and explains a failure.
    let lock = GraphLock::acquire_mode(&db_path, LockMode::Shared, Some(read_lock_wait())).ok()?;
    // Read-only: see `GraphStore::open_read_only` for why this process must
    // not close a writable connection. A graph Python would migrate is its.
    let store = GraphStore::open_read_only(&db_path).ok()?;
    if dagayn_build::graph_repo_mismatch(&store, root).is_some() {
        return None;
    }
    Some(OpenGraph {
        root: root.to_path_buf(),
        explicit: root.explicit,
        db_path,
        store,
        _lock: lock,
    })
}

/// [`open_graph`]'s checks for a writer: the exclusive lock taken without
/// waiting (a busy graph is Python's to wait for), a graph at the current
/// schema (one Python would migrate is its), then a read-write connection.
///
/// Only for a process where Python's own SQLite copy holds no connection to
/// the graph: closing a read-write connection can remove the WAL index under
/// one (see `GraphStore::open_read_only`). The front end calls the tools in
/// [`writes_graph`] only before it boots the Python server.
pub(crate) fn open_graph_for_write(root: &RepoRoot) -> Option<OpenGraph> {
    let db_path = existing_graph(root)?;
    let lock = GraphLock::acquire_mode(&db_path, LockMode::Exclusive, None).ok()?;
    {
        let probe = GraphStore::open_read_only(&db_path).ok()?;
        if dagayn_build::graph_repo_mismatch(&probe, root).is_some() {
            return None;
        }
    }
    let store = GraphStore::open(&db_path).ok()?;
    Some(OpenGraph {
        root: root.to_path_buf(),
        explicit: root.explicit,
        db_path,
        store,
        _lock: lock,
    })
}

/// `get_docs_section_tool`'s answer read from `search_roots`, then the
/// package's reference (`package_root`), as JSON and without `_repo`: what
/// the Python tool reports when it has no graph to resolve the root from.
pub fn docs_section_json(
    search_roots: Vec<PathBuf>,
    package_root: Option<&Path>,
    section: &str,
    max_chars: i64,
) -> Option<String> {
    docs::docs_section(search_roots, package_root, section, max_chars)
        .map(|reply| reply.into_payload().text)
}

/// `dagayn.refactor.dead_code.dead_code_report` as JSON (`dead`,
/// `suppressed`, `verification`): what the native `refactor_tool` reports,
/// for Python callers to share; `None` when the graph cannot be read.
pub fn find_dead_code_json(
    store: &GraphStore,
    kind: Option<&str>,
    file_pattern: Option<&str>,
) -> Option<String> {
    let report = dead_code::dead_code_report(store, kind, file_pattern)?;
    Some(
        json!({
            "dead": report.dead,
            "suppressed": report.suppressed,
            "verification": report.verification.value(),
        })
        .to_string(),
    )
}

/// `dagayn.refactor.suggestions.suggest_refactorings` as JSON: the move,
/// remove, split, and document suggestions before the stability policy the
/// `refactor_tool` reply applies; `None` when the graph cannot be read.
pub fn suggest_refactorings_json(store: &GraphStore) -> Option<String> {
    suggestions::suggest_refactorings(store).map(|all| Value::Array(all).to_string())
}

/// The graph's dead-code candidates before the repository check, as JSON:
/// for testing the graph heuristics only.
pub fn graph_dead_code_candidates_json(
    store: &GraphStore,
    kind: Option<&str>,
    file_pattern: Option<&str>,
) -> Option<String> {
    dead_code::graph_candidate_records(store, kind, file_pattern)
        .map(|records| Value::Array(records).to_string())
}

/// Tools that write the graph database, which the front end may answer only
/// while the Python server (and its SQLite connections) is not running.
pub fn writes_graph(name: &str) -> bool {
    matches!(name, "run_postprocess_tool")
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
