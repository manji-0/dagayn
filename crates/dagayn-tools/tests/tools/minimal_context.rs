use std::process::Command;

use dagayn_build::db_path_for_build;
use dagayn_graph::GraphStore;
use serde_json::json;

use crate::common::{Repo, answer, auto_preparing, declines};

#[test]
fn minimal_context_routes_the_task_and_reports_health() {
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    let repo = Repo::new("context", true);
    repo.build();
    let context = repo.context();
    let review = answer(
        &context,
        "get_minimal_context_tool",
        json!({"task": "レビュー"}),
    );
    assert_eq!(review["workflow"], "review");
    assert_eq!(
        review["sync"],
        json!({"state": "commit_synced", "status": "synced", "vcs": "git"})
    );
    assert_eq!(review["graph_health"]["status"], "ok");
    assert!(review["graph_health"].get("counts").is_none());
    let general = answer(&context, "get_minimal_context_tool", json!({}));
    assert_eq!(general["workflow"], "general");
    assert_eq!(general["confidence"], "low");

    repo.write("app.py", "def main():\n    return 1\n");
    let dirty = answer(&context, "get_minimal_context_tool", json!({"task": "fix"}));
    assert_eq!(dirty["sync"]["state"], "worktree_behind");
    assert!(
        dirty["graph_health"]["reason_codes"]
            .as_array()
            .expect("codes")
            .contains(&json!("uncommitted_changes_may_be_unindexed"))
    );

    // `casefold`, not `lower`: the ligature folds to `fi`.
    let folded = answer(
        &context,
        "get_minimal_context_tool",
        json!({"task": "\u{fb01}x Straße"}),
    );
    assert_eq!(folded["workflow"], "debug");

    // One commit: `HEAD~1` does not resolve, which the summary says.
    let unresolved = answer(
        &context,
        "get_minimal_context_tool",
        json!({"changed_files": ["app.py"], "base": "HEAD~1"}),
    );
    assert!(unresolved.get("risk").is_none());
    assert_eq!(unresolved["changes"]["state"], "unresolved");
    assert!(
        unresolved["summary"]
            .as_str()
            .expect("summary")
            .ends_with("Changes: 1 file(s); base HEAD~1 does not resolve.")
    );
    let against_head = answer(
        &context,
        "get_minimal_context_tool",
        json!({"changed_files": ["app.py"], "base": "HEAD"}),
    );
    // Every node of a changed file, not only those its hunks touch, in
    // file order.
    assert_eq!(against_head["key_entities"], json!(["main", "helper"]));
    // review_tool's findings, counted: main's test should run; helper went
    // with its only caller, so nothing dangles.
    assert_eq!(against_head["changes"]["state"], "analysed");
    assert_eq!(
        against_head["changes"]["findings"],
        json!({"tests_to_run": 1})
    );
    assert!(against_head["summary"].as_str().is_some_and(|s| {
        s.ends_with("Changes: 1 file(s); review_tool findings: 1 tests_to_run.")
    }));
    assert!(declines(
        &context,
        "get_minimal_context_tool",
        json!({"changed_files": [1]})
    ));

    // Without auto_prepare nothing is queued, whatever the embedding index.
    let mut embedding = repo.context();
    embedding.local_embedding = Some("bge-m3".to_string());
    let observed = answer(&embedding, "get_minimal_context_tool", json!({}));
    assert!(observed.get("repair").is_none());
    assert!(!repo.0.join(".dagayn/task_queue.db").exists());
}

fn queued_tasks(repo: &Repo) -> Vec<(i64, String, i64, String)> {
    let conn = rusqlite::Connection::open(repo.0.join(".dagayn/task_queue.db")).expect("queue");
    let mut stmt = conn
        .prepare("SELECT id, kind, priority, payload FROM tasks ORDER BY id")
        .expect("select");
    stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .expect("rows")
    .collect::<Result<_, _>>()
    .expect("tasks")
}

#[test]
fn minimal_context_queues_a_prepare_for_an_unbuilt_graph() {
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    // dagayn: tests crates/dagayn-tools/src/ensure.rs::ensure_graph
    let repo = Repo::new("unbuilt", true);
    // Python creates a missing graph; this tool only reads one.
    assert!(declines(
        &repo.context(),
        "get_minimal_context_tool",
        json!({})
    ));
    GraphStore::open(db_path_for_build(&repo.0).expect("db path")).expect("empty graph");

    let observed = answer(&repo.context(), "get_minimal_context_tool", json!({}));
    assert_eq!(observed["sync"]["state"], "unbuilt");
    assert_eq!(observed["sync"]["status"], "empty");
    assert_eq!(observed["graph_health"]["status"], "empty");
    // The graph first, with nothing to fill in.
    assert_eq!(
        observed["next"][0],
        json!({
            "tool": "ensure_graph_tool",
            "args": {},
            "why": "the graph is empty; build it before any analysis",
        })
    );
    assert!(observed.get("next_tool_suggestions").is_none());
    assert!(observed.get("recommended_action").is_none());
    assert!(observed.get("repair").is_none());

    // Queuing needs an interpreter for the worker.
    let mut no_python = auto_preparing(&repo);
    if let Some(auto) = no_python.auto_prepare.as_mut() {
        auto.python_executable = None;
    }
    assert!(declines(&no_python, "get_minimal_context_tool", json!({})));

    let context = auto_preparing(&repo);
    let first = answer(&context, "get_minimal_context_tool", json!({}));
    assert_eq!(
        first["repair"],
        json!({"state": "queued", "kind": "prepare", "task_id": 1, "action": "added"})
    );
    assert_eq!(
        first["prepare"],
        json!({"status": "queued", "action": "queued", "reason": "enqueued_background_prepare", "phases": null})
    );
    let second = answer(&context, "get_minimal_context_tool", json!({}));
    assert_eq!(
        second["repair"],
        json!({"state": "coalesced", "kind": "prepare", "task_id": 1, "action": "coalesced"})
    );
    assert_eq!(
        queued_tasks(&repo),
        vec![(
            1,
            "prepare".to_string(),
            10,
            r#"{"local_embedding": "none", "keep_local_embedding_server": true, "budget_seconds": 300}"#
                .to_string()
        )]
    );
}

#[test]
fn minimal_context_reports_and_repairs_commit_drift() {
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    // dagayn: tests crates/dagayn-tools/src/ensure.rs::ensure_graph
    let repo = Repo::new("drift", true);
    repo.build();
    repo.write("app.py", "def main():\n    return 2\n");
    let out = Command::new("git")
        .args(["commit", "-q", "--no-gpg-sign", "-am", "next"])
        .current_dir(&repo.0)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .expect("git");
    assert!(out.status.success(), "{out:?}");

    let observed = answer(
        &repo.context(),
        "get_minimal_context_tool",
        json!({"task": "fix"}),
    );
    assert_eq!(
        observed["sync"],
        json!({"state": "commit_drift", "status": "git_drift", "vcs": "git"})
    );
    assert_eq!(observed["why"], "sync.state=commit_drift");
    assert_eq!(observed["next"][0]["tool"], "ensure_graph_tool");
    assert!(
        observed["graph_health"]["reason_codes"]
            .as_array()
            .expect("codes")
            .contains(&json!("graph_describes_another_commit"))
    );

    let repaired = answer(
        &auto_preparing(&repo),
        "get_minimal_context_tool",
        json!({"task": "fix"}),
    );
    assert!(
        repaired["why"]
            .as_str()
            .is_some_and(|why| why.contains("repair is queued")),
        "{}",
        repaired["why"]
    );
    // A queued repair is not something to call.
    assert_ne!(repaired["next"][0]["tool"], "ensure_graph_tool");
    assert_eq!(repaired["repair"]["kind"], "prepare");
    // How the root was found is diagnosis; the reply names the repository.
    assert_eq!(
        repaired["_repo"].as_object().map(|repo| repo.len()),
        Some(1)
    );
}

#[test]
fn minimal_context_queues_missing_local_embeddings() {
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    let repo = Repo::new("embed", true);
    repo.build();
    let mut context = auto_preparing(&repo);
    // At HEAD: nothing to repair without a local embedding mode.
    let observed = answer(&context, "get_minimal_context_tool", json!({}));
    assert!(observed.get("repair").is_none());
    // No vectors at all refresh inline: the prepare lane.
    context.local_embedding = Some("bge-m3".to_string());
    let queued = answer(&context, "get_minimal_context_tool", json!({}));
    assert_eq!(queued["sync"]["state"], "commit_synced");
    assert_eq!(queued["repair"]["kind"], "prepare");
    assert_eq!(queued["next"], observed["next"]);
    assert_eq!(
        queued_tasks(&repo)[0].3,
        r#"{"local_embedding": "bge-m3", "keep_local_embedding_server": true, "budget_seconds": 300}"#
    );
}

#[test]
fn minimal_context_never_queues_outside_a_repository() {
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    let repo = Repo::new("novcs", false);
    std::fs::remove_dir_all(repo.0.join(".git")).expect("unmark");
    GraphStore::open(db_path_for_build(&repo.0).expect("db path")).expect("empty graph");
    let observed = answer(
        &auto_preparing(&repo),
        "get_minimal_context_tool",
        json!({}),
    );
    assert_eq!(
        observed["sync"],
        json!({"state": "unbuilt", "status": "empty", "vcs": "none"})
    );
    assert!(observed.get("repair").is_none());
    assert!(!repo.0.join(".dagayn/task_queue.db").exists());
}

/// #191: `changed_files` omitted detects the change as `review_tool` does;
/// an explicit empty list still looks at none.
#[test]
fn minimal_context_detects_changed_files_when_omitted() {
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    let repo = Repo::new("context-detect", true);
    repo.build();
    let context = repo.context();
    let clean = answer(
        &context,
        "get_minimal_context_tool",
        json!({"task": "review recent changes"}),
    );
    let summary = clean["summary"].as_str().unwrap_or_default();
    assert!(
        summary.contains("No changed files against HEAD~1"),
        "{clean}"
    );
    assert!(clean.get("changes").is_none(), "{clean}");

    repo.write(
        "app.py",
        "def main():\n    return helper() + 1\n\n\ndef helper():\n    pass\n",
    );
    let dirty = answer(
        &context,
        "get_minimal_context_tool",
        json!({"task": "review recent changes"}),
    );
    assert_eq!(dirty["changes"]["state"], "analysed", "{dirty}");
    assert_eq!(dirty["changes"]["files"], 1, "{dirty}");
    assert!(
        dirty["key_entities"]
            .as_array()
            .is_some_and(|names| names.contains(&json!("main"))),
        "{dirty}"
    );
    let skipped = answer(
        &context,
        "get_minimal_context_tool",
        json!({"task": "review recent changes", "changed_files": []}),
    );
    assert!(skipped.get("changes").is_none(), "{skipped}");
    assert!(
        !skipped["summary"]
            .as_str()
            .unwrap_or_default()
            .contains("Changes")
    );
}
