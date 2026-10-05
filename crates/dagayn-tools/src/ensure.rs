//! `ensure_graph_tool` (`dagayn.tools.ensure.ensure_graph`, which is
//! `session_prepare` with the MCP wrapper's arguments).
//!
//! Answered here only when the prepare has nothing to do: a git checkout (not
//! a linked worktree, which may be seeded) whose graph already describes HEAD
//! with every indexed file verified, nothing to embed, no `force`, and no hook
//! skip-when-busy contract. Building, updating, seeding, and embedding are
//! Python's.

use std::time::Instant;

use dagayn_build::{SyncAssessment, Vcs, detect_vcs, is_linked_worktree};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo, suggestions};

/// The budget the MCP wrapper passes.
const BUDGET_SECONDS: i64 = 300;
const SEEDED_NEEDS_VERIFY_KEY: &str = "seeded_needs_content_verify";
const HOOK_UPDATE_ENV: &str = "DAGAYN_HOOK_UPDATE";
const NEXT_TOOL_SUGGESTIONS: [&str; 3] = [
    "get_minimal_context_tool",
    "review_tool",
    "query_graph_tool",
];

/// `seal_graph_sync_state(...)` for a synced or ahead assessment, in the
/// contract model's field order.
fn sync_payload(root: &str, sync: &SyncAssessment, total_nodes: i64, files_count: i64) -> Value {
    let mut out = Ordered::default()
        .put("repo_root", root)
        .put("status", "synced")
        .put("vcs", "git")
        .put("git_head_sha", json!(sync.git_head_sha))
        .put("current_head_sha", json!(sync.current_head_sha))
        .put("current_branch", json!(sync.current_branch))
        .put("last_updated", json!(sync.last_updated))
        .put("total_nodes", total_nodes)
        .put("files_count", files_count)
        .put("content_verified", sync.content_verified)
        .put("unverified_file_count", sync.unverified_file_count)
        .put("state", sync.state)
        .put("worktree_dirty", sync.worktree_dirty);
    if sync.state == "worktree_ahead" {
        out = out
            .replace("status", json!("dirty_worktree"))
            .put("indexed_files", json!(sync.indexed_files));
    }
    out.value()
}

pub(crate) fn ensure_graph(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let started = Instant::now();
    let args = Args::new(arguments, &["repo_root", "force"])?;
    // `force` refreshes even a synced graph.
    if !matches!(arguments.get("force"), None | Some(Value::Bool(false))) {
        return None;
    }
    // A hook-started server skips when the write lock is busy.
    if std::env::var(HOOK_UPDATE_ENV).is_ok_and(|value| !value.is_empty() && value != "0") {
        return None;
    }
    // `session_prepare._resolve_repo` resolves the root before opening the
    // store, so even an auto-detected one is validated and reported explicit.
    let root = resolve_repo(context, args.optional_string("repo_root")?)?.into_explicit()?;
    if detect_vcs(&root) != Vcs::Git || is_linked_worktree(&root) {
        return None;
    }
    let graph = open_graph(&root)?;
    // Verifying a seeded graph clears its flag, a write.
    if graph
        .store
        .get_metadata(SEEDED_NEEDS_VERIFY_KEY)
        .ok()?
        .is_some_and(|value| value == "1")
    {
        return None;
    }
    // `_resolve_local_embedding(None) or "none"`, echoed as given.
    let local_embedding = context
        .local_embedding
        .as_deref()
        .filter(|mode| !mode.is_empty())
        .unwrap_or("none");
    let embedding_requested =
        !matches!(local_embedding.trim().to_lowercase().as_str(), "" | "none");
    if embedding_requested && !dagayn_build::embedding_refresh_skips(&graph.store).ok()? {
        return None;
    }
    // Python's first assessment hashes without a cap; a capped one that gave
    // up cannot say the structure phase would be skipped.
    let sync = dagayn_build::assess_graph_sync(&graph.store, &root).ok()?;
    if !matches!(sync.state, "commit_synced" | "worktree_ahead") || !sync.content_verified {
        return None;
    }
    let stats = graph.store.get_stats().ok()?;
    let health = Answerability::recorded(&graph.store, &stats)?.full();

    let reason = if sync.state == "worktree_ahead" || sync.worktree_dirty {
        "graph_ready_worktree_dirty"
    } else {
        "graph_ready"
    };
    let embedding = if embedding_requested {
        "done"
    } else {
        "not_requested"
    };
    let summary = format!(
        "session prepare (noop); sync={}; structure=noop; embedding={embedding}; {} nodes / {} files",
        sync.state, stats.total_nodes, stats.files_count
    );
    let root_text = graph.root.to_string_lossy().into_owned();
    let (hints, kept) = suggestions(context, &NEXT_TOOL_SUGGESTIONS);
    let elapsed = (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0;
    Some(
        Ordered::default()
            .put("status", "ok")
            .put("summary", summary)
            .put("action", "noop")
            .put("reason", reason)
            .put("total_nodes", stats.total_nodes)
            .put("total_edges", stats.total_edges)
            .put("files_count", stats.files_count)
            .put("last_updated", json!(stats.last_updated))
            .put("graph_health", health)
            .put(
                "sync",
                sync_payload(&root_text, &sync, stats.total_nodes, stats.files_count),
            )
            .put(
                "phases",
                json!({"structure": "noop", "embedding": embedding}),
            )
            .put("budget_seconds", BUDGET_SECONDS)
            .put("elapsed_seconds", elapsed)
            .put("local_embedding", local_embedding)
            .put("embedding_policy", "auto")
            .put("repo_root", root_text)
            .put("_hints", hints)
            .put("next_tool_suggestions", kept)
            .put(
                "worktree_seed",
                json!({"seeded": false, "skipped": true, "reason": "not_linked_worktree"}),
            )
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}
