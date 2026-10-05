//! `get_minimal_context_tool` (`dagayn.tools.context.get_minimal_context`
//! with the MCP wrapper's `auto_prepare=True`).
//!
//! Answered here only when Python would neither queue a repair (an `unbuilt`
//! or `commit_drift` graph, missing local embeddings) nor run the risk
//! analysis (`changed_files`), and when the assessment would not have to
//! write (a freshly seeded worktree). Everything else is Python's.

use dagayn_build::{Vcs, detect_vcs};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo};

const SEEDED_NEEDS_VERIFY_KEY: &str = "seeded_needs_content_verify";

const REVIEW: &[&str] = &[
    "review",
    "pr",
    "merge",
    "diff",
    "レビュー",
    "差分",
    "プルリク",
    "マージ",
];
const DEBUG: &[&str] = &[
    "debug",
    "bug",
    "error",
    "fix",
    "デバッグ",
    "バグ",
    "不具合",
    "エラー",
    "修正",
];
const FEATURE: &[&str] = &[
    "feature",
    "add",
    "implement",
    "機能追加",
    "新規機能",
    "実装",
    "追加",
];
const REFACTOR: &[&str] = &[
    "refactor",
    "rename",
    "dead",
    "clean",
    "リファクタ",
    "リファクタリング",
    "名称変更",
    "改名",
    "デッドコード",
    "整理",
];
const EXPLORE: &[&str] = &[
    "onboard",
    "understand",
    "explore",
    "arch",
    "探索",
    "理解",
    "アーキテクチャ",
    "構造",
    "オンボーディング",
];

/// `(workflow, suggested tools, recommended_action, why, confidence)`.
struct Workflow {
    name: &'static str,
    tools: [&'static str; 3],
    recommended_action: &'static str,
    why: &'static str,
    confidence: &'static str,
}

const WORKFLOWS: [Workflow; 6] = [
    Workflow {
        name: "review",
        tools: ["review_tool", "flow_tool", "query_graph_tool"],
        recommended_action: "Run review_tool mode=changes first, then drill into context only when needed.",
        why: "The task mentions reviewing a diff or PR, so risk and changed-node ranking are the fastest entry point.",
        confidence: "high",
    },
    Workflow {
        name: "debug",
        tools: [
            "semantic_search_nodes_tool",
            "query_graph_tool",
            "flow_tool",
        ],
        recommended_action: "Search for the failing concept, then trace callers and callees around the matching node.",
        why: "The task mentions a bug or failure, so locating the relevant symbol before graph traversal reduces noise.",
        confidence: "high",
    },
    Workflow {
        name: "refactor",
        tools: [
            "refactor_tool",
            "query_graph_tool",
            "architecture_analysis_tool",
        ],
        recommended_action: "Get graph-backed refactor suggestions, then verify impact before editing.",
        why: "The task mentions cleanup or refactoring, so candidate ranking and safety checks should precede file edits.",
        confidence: "high",
    },
    Workflow {
        name: "explore",
        tools: [
            "architecture_analysis_tool",
            "flow_tool",
            "query_graph_tool",
        ],
        recommended_action: "Start with architecture_analysis_tool mode=overview, then drill into communities or flow_tool mode=list.",
        why: "The task asks to understand structure, so a broad graph summary is cheaper than reading files first.",
        confidence: "high",
    },
    Workflow {
        name: "feature",
        tools: [
            "semantic_search_nodes_tool",
            "query_graph_tool",
            "review_tool",
        ],
        recommended_action: "Search for related symbols, trace dependencies, then run change review after implementation.",
        why: "The task mentions adding behavior, so finding extension points should come before editing.",
        confidence: "medium",
    },
    Workflow {
        name: "general",
        tools: [
            "review_tool",
            "semantic_search_nodes_tool",
            "architecture_analysis_tool",
        ],
        recommended_action: "Use minimal change review, semantic search, or architecture overview based on the first concrete finding.",
        why: "No specific workflow keyword was detected, so the default keeps broad options available.",
        confidence: "low",
    },
];

/// `_workflow_for_task`, in its keyword order.
fn workflow_for_task(task: &str) -> &'static Workflow {
    let folded = task.to_lowercase();
    let mentions = |keywords: &[&str]| keywords.iter().any(|keyword| folded.contains(keyword));
    let name = if mentions(REVIEW) {
        "review"
    } else if mentions(DEBUG) {
        "debug"
    } else if mentions(REFACTOR) {
        "refactor"
    } else if mentions(EXPLORE) {
        "explore"
    } else if mentions(FEATURE) {
        "feature"
    } else {
        "general"
    };
    WORKFLOWS
        .iter()
        .find(|workflow| workflow.name == name)
        .unwrap_or(&WORKFLOWS[5])
}

/// Whether `to_lowercase` folds `task` as Python's `casefold` does: ASCII,
/// CJK, and kana, and any character that has no case at all. A letter with
/// special case folding (`ß`, ligatures, final sigma) goes to Python.
fn folds_like_python(task: &str) -> bool {
    task.chars().all(|c| {
        c.is_ascii()
            || !c.is_alphabetic()
            || matches!(c as u32,
                0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF66..=0xFF9F)
    })
}

/// Up to `limit` non-empty `name`s of `items` (`_names_from_items`).
fn names(items: &[Value], limit: usize) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .take(limit)
        .map(str::to_string)
        .collect()
}

fn json_rows(raw: Result<String, dagayn_graph::GraphError>) -> Vec<Value> {
    raw.ok()
        .and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok())
        .unwrap_or_default()
}

pub(crate) fn get_minimal_context(
    context: &Context,
    arguments: &Map<String, Value>,
) -> Option<Payload> {
    let args = Args::new(arguments, &["task", "changed_files", "repo_root", "base"])?;
    let task = match arguments.get("task") {
        None => "",
        Some(Value::String(task)) => task,
        Some(_) => return None,
    };
    match arguments.get("changed_files") {
        None | Some(Value::Null) => {}
        // The risk analysis is Python's.
        Some(Value::Array(files)) if files.is_empty() => {}
        Some(_) => return None,
    }
    if !matches!(arguments.get("base"), None | Some(Value::String(_))) {
        return None;
    }
    let embedding_requested = context
        .local_embedding
        .as_deref()
        .is_some_and(|mode| !mode.trim().is_empty() && !mode.trim().eq_ignore_ascii_case("none"));
    if !folds_like_python(task) {
        return None;
    }
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    let (git, vcs) = match detect_vcs(&root) {
        Vcs::Git => (true, "git"),
        Vcs::None => (false, "none"),
        Vcs::Jj | Vcs::Svn => return None,
    };
    let graph = open_graph(&root)?;
    // A seeded worktree is verified (and its flag cleared) by Python.
    if graph
        .store
        .get_metadata(SEEDED_NEEDS_VERIFY_KEY)
        .ok()?
        .is_some_and(|value| value == "1")
    {
        return None;
    }
    // With a local embedding mode, Python first decides whether to refresh
    // the vectors (and queues it); only "nothing to do" is answered here.
    if embedding_requested && !dagayn_build::embedding_refresh_skips(&graph.store).ok()? {
        return None;
    }
    let sync = dagayn_build::assess_graph_sync(&graph.store, &root).ok()?;
    let legacy_status = match sync.state {
        "commit_synced" => "synced",
        "worktree_behind" | "worktree_ahead" => "dirty_worktree",
        // `unbuilt` and `commit_drift` queue a prepare in Python.
        _ => return None,
    };
    let stats = graph.store.get_stats().ok()?;
    // `commit_tier_from_sync`: at HEAD, so only a dirty tree can add a code.
    let freshness =
        sync.current_head_sha
            .clone()
            .filter(|_| git)
            .map(|head| dagayn_build::CommitFreshness {
                state: "commit_synced",
                git_head_sha: Some(head.clone()),
                current_head_sha: head,
                worktree_dirty: sync.worktree_dirty,
                extractor_drift: Vec::new(),
            });
    let health = Answerability::compute(&graph.store, &stats, freshness.as_ref()).without_counts();

    let workflow = workflow_for_task(task);
    let suggestions: Vec<&str> = workflow
        .tools
        .iter()
        .copied()
        .filter(|tool| context.exposes(tool))
        .collect();
    // `get_communities(store, sort_by="size")[:3]`, then their names.
    let community_rows = json_rows(graph.store.get_communities_json("size", 0));
    let communities = names(&community_rows[..community_rows.len().min(3)], 3);
    let flows = names(&json_rows(graph.store.get_flows_json("criticality", 3)), 3);

    let summary = format!(
        "{} nodes, {} edges across {} files.",
        stats.total_nodes, stats.total_edges, stats.files_count
    );
    let mut response = Ordered::default()
        .put("status", "ok")
        .put("summary", summary);
    if !communities.is_empty() {
        response = response.put("communities", json!(communities));
    }
    if !flows.is_empty() {
        response = response.put("top_flows", json!(flows));
    }
    if !suggestions.is_empty() {
        response = response.put("next_tool_suggestions", json!(suggestions));
    }
    Some(
        response
            .put("workflow", workflow.name)
            .put("recommended_action", workflow.recommended_action)
            .put("why", workflow.why)
            .put("confidence", workflow.confidence)
            .put("graph_health", health)
            .put(
                "sync",
                json!({"state": sync.state, "status": legacy_status, "vcs": vcs}),
            )
            .put("_repo", graph.repo_context())
            .into_payload(),
    )
}
