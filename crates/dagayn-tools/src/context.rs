//! `get_minimal_context_tool` (`dagayn.tools.context.get_minimal_context`
//! with the MCP wrapper's `auto_prepare=True`).
//!
//! Answered here only when Python would neither queue a repair (an `unbuilt`
//! or `commit_drift` graph, missing local embeddings) nor run the risk
//! analysis (`changed_files`), and when the assessment would not have to
//! write (a freshly seeded worktree). Everything else is Python's.

use dagayn_build::{Vcs, detect_vcs};
use dagayn_graph::GraphStats;
use serde_json::{Map, Value, json};

use crate::{Args, Context, OpenGraph, Ordered, Payload, explicit_repo, open_graph};

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

/// Python's `round(value, 4)` (correctly rounded, ties to even on the exact
/// binary value, as Rust's formatting rounds).
fn round4(value: f64) -> f64 {
    format!("{value:.4}").parse().unwrap_or(value)
}

/// `graph_answerability_summary` without its `counts`, as
/// `_graph_answerability` returns it, for a graph at HEAD (the only state
/// answered here).
fn graph_health(graph: &OpenGraph, stats: &GraphStats, worktree_dirty: bool, git: bool) -> Value {
    let counts = graph.store.answerability_counts();
    let edge_kind = |kind: &str| stats.edges_by_kind.get(kind).copied().unwrap_or(0);
    let test_edges = edge_kind("TESTED_BY");
    let cross_artifact = edge_kind("CROSS_ARTIFACT");
    let reportable_cross = (cross_artifact - counts.unresolved_markdown_code_spans).max(0);
    let reportable_unresolved =
        (counts.unresolved_cross_artifact_edges - counts.unresolved_markdown_code_spans).max(0);
    let unresolved_ratio = if reportable_cross != 0 {
        reportable_unresolved as f64 / reportable_cross as f64
    } else {
        0.0
    };
    let last_updated = stats
        .last_updated
        .as_deref()
        .is_some_and(|value| !value.is_empty());

    let mut reason_codes: Vec<&str> = Vec::new();
    let mut score = 1.0_f64;
    if !counts.failures.is_empty() {
        for failure in &counts.failures {
            if !reason_codes.contains(failure) {
                reason_codes.push(failure);
            }
        }
        score -= 0.2;
    }
    if stats.total_nodes == 0 || stats.files_count == 0 {
        reason_codes.push("empty_graph");
        score = 0.0;
    }
    if counts.flows == 0 {
        reason_codes.push("missing_flows");
        score -= 0.15;
    }
    if counts.communities == 0 {
        reason_codes.push("missing_communities");
        score -= 0.15;
    }
    if test_edges == 0 {
        reason_codes.push("missing_test_edges");
        score -= 0.1;
    }
    if cross_artifact != 0 && unresolved_ratio > 0.35 {
        reason_codes.push("many_unresolved_cross_artifact_edges");
        score -= 0.15;
    }
    if !last_updated {
        reason_codes.push("missing_last_updated");
        score -= 0.1;
    }
    if counts.stale_flow_memberships > 0 || counts.unassigned_nodes > 0 {
        reason_codes.push("stale_derived_structures");
        score -= 0.15;
    }
    // `_freshness_reason_codes` for a graph at HEAD with no extractor drift:
    // only a dirty working tree is worth a code, and only under git.
    if git && worktree_dirty {
        reason_codes.push("uncommitted_changes_may_be_unindexed");
        score -= 0.1;
    }
    let score = round4(score).max(0.0);
    let status = if score >= 0.75 {
        "ok"
    } else if score > 0.0 {
        "degraded"
    } else {
        "empty"
    };
    let mut health = Map::new();
    health.insert("status".into(), json!(status));
    health.insert("score".into(), json!(score));
    health.insert("reason_codes".into(), json!(reason_codes));
    health.insert(
        "parse".into(),
        json!([stats.files_count, stats.languages.len(), last_updated]),
    );
    health.insert(
        "answerability".into(),
        json!([
            counts.flows,
            counts.communities,
            test_edges,
            reportable_cross,
            round4(unresolved_ratio)
        ]),
    );
    if reportable_unresolved != 0 {
        health.insert("unresolved_edges".into(), json!(reportable_unresolved));
    }
    Value::Object(health)
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
    let root = explicit_repo(context, args.optional_string("repo_root")?)?;
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
    let health = graph_health(&graph, &stats, sync.worktree_dirty, git);

    let workflow = workflow_for_task(task);
    let suggestions: Vec<&str> = workflow
        .tools
        .iter()
        .copied()
        .filter(|tool| {
            context
                .allowed_tools
                .as_ref()
                .is_none_or(|allowed| allowed.contains(*tool))
        })
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
