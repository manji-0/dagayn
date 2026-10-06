//! `get_minimal_context_tool` (`dagayn.tools.context.get_minimal_context`;
//! the MCP tool runs it with `auto_prepare=True`, which [`Context::auto_prepare`]
//! carries).
//!
//! The graph's sync state, health, top communities and flows, a workflow
//! routed from the task, and with `changed_files` a review-priority risk.
//! With `auto_prepare`, an `unbuilt` or `commit_drift` graph (or a local
//! embedding index too far behind) queues a background `prepare`, and a
//! smaller embedding gap an `embed`, in `.dagayn/task_queue.db`, starting the
//! Python queue worker when none runs; the call never waits for either.
//! Verifying a freshly seeded worktree's content clears its marker, as the
//! Python assessment does.

use std::collections::HashSet;
use std::path::Path;

use dagayn_build::task_queue::{self, TaskPayload};
use dagayn_build::{EmbeddingRefresh, SyncAssessment, Vcs, detect_vcs};
use serde_json::{Map, Value, json};

use crate::answerability::Answerability;
use crate::changes::{DiffParse, analyze_changes_with, parse_diff};
use crate::pyunicode::casefold;
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

/// `_workflow_for_task`, in its keyword order (`task.casefold()`; the
/// keywords fold to themselves).
fn workflow_for_task(task: &str) -> &'static Workflow {
    let folded = casefold(task);
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

/// `LEGACY_STATUS_BY_STATE`.
fn legacy_status(state: &str) -> &'static str {
    match state {
        "unbuilt" => "empty",
        "commit_drift" => "git_drift",
        "commit_synced" => "synced",
        _ => "dirty_worktree",
    }
}

/// `commit_tier_from_sync`: `None` outside `GIT_BACKED_VCS` or without a
/// HEAD.
fn commit_tier_from_sync(
    sync: &SyncAssessment,
    git: bool,
) -> Option<dagayn_build::CommitFreshness> {
    if !git {
        return None;
    }
    let current = sync.current_head_sha.clone()?;
    let drift = !sync.extractor_drift.is_empty();
    let state = if drift || sync.git_head_sha.as_deref() != Some(current.as_str()) {
        "commit_drift"
    } else {
        "commit_synced"
    };
    Some(dagayn_build::CommitFreshness {
        state,
        git_head_sha: sync.git_head_sha.clone(),
        current_head_sha: current,
        worktree_dirty: sync.worktree_dirty,
        extractor_drift: sync.extractor_drift.clone(),
    })
}

/// `_local_embedding_requested`.
fn local_embedding_requested(mode: &str) -> bool {
    // `str.strip()` also strips the ASCII separators `\x1c`-`\x1f`.
    let mode = mode.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c));
    !mode.is_empty() && !mode.eq_ignore_ascii_case("none")
}

/// `_MAX_RISK_FILES`: `int(DAGAYN_MINIMAL_CONTEXT_MAX_RISK_FILES)`, default
/// 100; `None` for a value `int()` rejects (Python fails importing it).
fn max_risk_files() -> Option<usize> {
    let Ok(raw) = std::env::var("DAGAYN_MINIMAL_CONTEXT_MAX_RISK_FILES") else {
        return Some(100);
    };
    let text = raw.trim();
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    let bytes = digits.as_bytes();
    let well_formed = !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_digit)
        && bytes.last().is_some_and(u8::is_ascii_digit)
        && bytes.iter().all(|b| b.is_ascii_digit() || *b == b'_')
        && !digits.contains("__");
    if !well_formed {
        return None;
    }
    let value: i64 = text.replace('_', "").parse().ok()?;
    // `len(files) > value` for a negative cap skips every non-empty list.
    Some(usize::try_from(value).unwrap_or(0))
}

/// The risk fields of the reply (step 3 of the Python body).
struct Risk {
    /// `unknown` (not analysed, or the analysis failed), `skipped`, `low`,
    /// `medium`, or `high`.
    level: &'static str,
    score: f64,
    top_affected: Vec<String>,
    affected_flows: Vec<String>,
    test_gap_count: usize,
    skipped_count: usize,
}

impl Risk {
    fn unknown() -> Self {
        Self {
            level: "unknown",
            score: 0.0,
            top_affected: Vec::new(),
            affected_flows: Vec::new(),
            test_gap_count: 0,
            skipped_count: 0,
        }
    }

    fn scored(score: f64) -> Self {
        let level = if score > 0.7 {
            "high"
        } else if score > 0.4 {
            "medium"
        } else {
            "low"
        };
        Self {
            level,
            score,
            ..Self::unknown()
        }
    }
}

/// `analyze_changes(store, [str(root / f) ...], repo_root=root, base=base,
/// heuristic_test_gap_node_limit=10)`, which diffs `base` itself; a failure
/// leaves the risk unknown, as Python's `except` does.
fn risk_of(store: &dagayn_graph::GraphStore, root: &Path, base: &str, files: &[String]) -> Risk {
    match parse_diff(root, base) {
        DiffParse::BaseUnresolved => {
            // No changed nodes: a zero score, but the files' flows still count.
            let absolute: Vec<String> = files.iter().map(|file| join(root, file)).collect();
            let Ok(flows) = store.get_affected_flows_annotated(&absolute) else {
                return Risk::unknown();
            };
            Risk {
                affected_flows: names(&flows, 5),
                ..Risk::scored(0.0)
            }
        }
        DiffParse::Ranges(mut ranges) => {
            // Only the named files' lines count; the rest of the base diff
            // adds no nodes.
            let wanted: HashSet<String> = files.iter().map(|file| join(root, file)).collect();
            ranges.retain(|rel, _| wanted.contains(&join(root, rel)));
            let Some(analysis) = analyze_changes_with(store, root, base, files, &ranges, false)
            else {
                return Risk::unknown();
            };
            let score = analysis
                .get("review_priority_score")
                .as_f64()
                .unwrap_or(0.0);
            let rows = |key: &str| analysis.get(key).as_array().cloned().unwrap_or_default();
            let mut priorities = rows("review_priorities");
            if priorities.is_empty() {
                priorities = rows("changed_functions");
            }
            Risk {
                top_affected: names(&priorities, 5),
                affected_flows: names(&rows("affected_flows"), 5),
                test_gap_count: rows("test_gaps").len(),
                ..Risk::scored(score)
            }
        }
    }
}

/// `str(Path(root) / rel)`.
fn join(root: &Path, rel: &str) -> String {
    root.join(rel)
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect::<std::path::PathBuf>()
        .to_string_lossy()
        .into_owned()
}

/// A queued repair: `repair`, and for a prepare `prepare`.
struct Repair {
    kind: &'static str,
    action: &'static str,
    task_id: i64,
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
    let changed_files: Vec<String> = match arguments.get("changed_files") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(files)) => files
            .iter()
            .map(|file| file.as_str().map(str::to_string))
            .collect::<Option<_>>()?,
        Some(_) => return None,
    };
    let base = match arguments.get("base") {
        None => "HEAD~1",
        Some(Value::String(base)) => base,
        Some(_) => return None,
    };
    let max_risk_files = max_risk_files()?;
    let local_embedding = context
        .local_embedding
        .as_deref()
        .filter(|mode| !mode.is_empty())
        .unwrap_or("none");
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    // `git`: `GIT_BACKED_VCS`, whose commit tier `commit_tier_from_sync` reads.
    let (git, vcs) = match detect_vcs(&root) {
        Vcs::Git => (true, "git"),
        Vcs::Jj => (true, "jj"),
        Vcs::Svn => (false, "svn"),
        Vcs::None => (false, "none"),
    };
    let graph = open_graph(&root)?;
    let seeded = graph
        .store
        .get_metadata(SEEDED_NEEDS_VERIFY_KEY)
        .ok()?
        .is_some_and(|value| value == "1");
    let sync = dagayn_build::assess_graph_sync(&graph.store, &root).ok()?;
    if seeded && sync.content_verified && !matches!(sync.state, "unbuilt" | "commit_drift") {
        // The store is read-only; best effort, as in Python.
        let _ = dagayn_build::clear_seed_verification(&graph.db_path);
    }

    // Observe vs Repair: never wait on a parse or an embed; queue one lane.
    // A root outside any repository is never queued.
    let mut repair: Option<Repair> = None;
    if let Some(auto) = &context.auto_prepare
        && vcs != "none"
    {
        let refresh = if local_embedding_requested(local_embedding) {
            // `get_embedding_status` reports an unreadable index as
            // unavailable, which skips.
            dagayn_build::embedding_refresh_action(&graph.store).unwrap_or(EmbeddingRefresh::Skip)
        } else {
            EmbeddingRefresh::Skip
        };
        let lane = if matches!(sync.state, "unbuilt" | "commit_drift")
            || refresh == EmbeddingRefresh::Inline
        {
            Some((
                "prepare",
                TaskPayload::default()
                    .put_str("local_embedding", local_embedding)
                    .put_bool("keep_local_embedding_server", true)
                    .put_int("budget_seconds", auto.budget_seconds),
            ))
        } else if refresh == EmbeddingRefresh::Queue {
            Some((
                "embed",
                TaskPayload::default()
                    .put_str("local_embedding", local_embedding)
                    .put_bool("keep_local_embedding_server", true),
            ))
        } else {
            None
        };
        if let Some((kind, payload)) = lane {
            let python = auto.python_executable.as_deref()?;
            let pythonpath = context.package_root.as_deref()?;
            let data_dir = graph.db_path.parent()?;
            let (action, task_id) =
                task_queue::enqueue(&data_dir.join(task_queue::QUEUE_DB_NAME), kind, &payload)
                    .ok()?;
            task_queue::ensure_worker(data_dir, &graph.root, python, pythonpath);
            repair = Some(Repair {
                kind,
                action,
                task_id,
            });
        }
    }

    let stats = graph.store.get_stats().ok()?;
    let freshness = commit_tier_from_sync(&sync, git);
    let health = Answerability::compute(&graph.store, &stats, freshness.as_ref());

    let workflow = workflow_for_task(task);
    let suggestions: Vec<&str> = workflow
        .tools
        .iter()
        .copied()
        .filter(|tool| context.exposes(tool))
        .collect();

    let risk = if changed_files.is_empty() {
        Risk::unknown()
    } else if changed_files.len() > max_risk_files {
        Risk {
            level: "skipped",
            skipped_count: changed_files.len(),
            ..Risk::unknown()
        }
    } else {
        risk_of(&graph.store, &graph.root, base, &changed_files)
    };

    // `get_communities(store, sort_by="size")[:3]`, then their names.
    let community_rows = json_rows(graph.store.get_communities_json("size", 0));
    let communities = names(&community_rows[..community_rows.len().min(3)], 3);
    let flows = names(&json_rows(graph.store.get_flows_json("criticality", 3)), 3);

    let mut summary = vec![format!(
        "{} nodes, {} edges across {} files.",
        stats.total_nodes, stats.total_edges, stats.files_count
    )];
    if risk.level != "unknown" {
        summary.push(format!(
            "Review priority: {} ({:.2}).",
            risk.level, risk.score
        ));
    }
    if risk.skipped_count != 0 {
        summary.push(format!(
            "Risk analysis skipped for {} files.",
            risk.skipped_count
        ));
    }
    if risk.test_gap_count != 0 {
        summary.push(format!("{} test gaps.", risk.test_gap_count));
    }

    // `compact_response`.
    let mut response = Ordered::default()
        .put("status", "ok")
        .put("summary", summary.join(" "));
    if !risk.top_affected.is_empty() {
        response = response.put("key_entities", json!(risk.top_affected));
    }
    if risk.level != "unknown" {
        response = response.put("risk", risk.level);
    }
    if !communities.is_empty() {
        response = response.put("communities", json!(communities));
    }
    if !flows.is_empty() {
        response = response.put("top_flows", json!(flows));
    }
    if !risk.affected_flows.is_empty() {
        response = response.put("flows_affected", json!(risk.affected_flows));
    }
    if !suggestions.is_empty() {
        response = response.put("next_tool_suggestions", json!(suggestions));
    }
    response = response
        .put("workflow", workflow.name)
        .put("recommended_action", workflow.recommended_action)
        .put("why", workflow.why)
        .put("confidence", workflow.confidence)
        .put("graph_health", health.without_counts())
        .put(
            "sync",
            json!({"state": sync.state, "status": legacy_status(sync.state), "vcs": vcs}),
        );
    if let Some(queued) = &repair {
        if queued.kind == "prepare" {
            response = response.put(
                "prepare",
                json!({
                    "status": "queued",
                    "action": "queued",
                    "reason": "enqueued_background_prepare",
                    "phases": null,
                }),
            );
        }
        response = response.put(
            "repair",
            json!({
                "state": if queued.action == "added" { "queued" } else { "coalesced" },
                "kind": queued.kind,
                "task_id": queued.task_id,
                "action": queued.action,
            }),
        );
    }
    if health.status == "empty" || sync.state == "unbuilt" {
        let mut tools = vec!["ensure_graph_tool"];
        tools.extend(
            suggestions
                .iter()
                .filter(|tool| **tool != "ensure_graph_tool"),
        );
        response = response
            .set(
                "recommended_action",
                json!(
                    "Call ensure_graph_tool first; the graph is empty and analysis tools will \
                     return nothing useful."
                ),
            )
            .set(
                "why",
                json!(
                    "graph_health reports an empty graph, so build/bootstrap must precede \
                     review, search, or architecture analysis."
                ),
            )
            .set("confidence", json!("high"))
            .set("next_tool_suggestions", json!(tools));
    } else if sync.state == "commit_drift" {
        let action = if repair.is_some() {
            "Graph repair is queued; call ensure_graph_tool only if you must wait for it."
        } else {
            "Call ensure_graph_tool to sync the graph."
        };
        // Python indexes `next_tool_suggestions`, which is absent when the
        // session exposes none of the workflow's tools (a `KeyError`).
        let mut tools: Vec<&str> = response
            .get("next_tool_suggestions")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if !tools.contains(&"ensure_graph_tool") {
            tools.insert(0, "ensure_graph_tool");
        }
        let tools = json!(tools);
        response = response
            .set("recommended_action", json!(action))
            .set("why", json!(format!("sync.state={}", sync.state)))
            .set("confidence", json!("high"))
            .set("next_tool_suggestions", tools);
    }
    let mut repo = graph.repo_context();
    if repair
        .as_ref()
        .is_some_and(|queued| queued.kind == "prepare")
    {
        // The prepare branch reopens the store by the probed root's path.
        repo["source"] = json!("explicit");
    }
    Some(response.put("_repo", repo).into_payload())
}
