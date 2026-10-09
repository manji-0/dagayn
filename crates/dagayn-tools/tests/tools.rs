//! The native tools against a graph built in Rust: what they answer, and
//! where they leave the call to Python (`None`). The pytest suite
//! (`tests/test_mcp_frontend.py`) checks the answers equal fastmcp's.

use std::path::{Path, PathBuf};
use std::process::Command;

use dagayn_build::{BuildOptions, PostprocessLevel, db_path_for_build, full_build};
use dagayn_graph::GraphStore;
use dagayn_tools::{Context, call};
use serde_json::{Value, json};

struct Repo(PathBuf);

impl Repo {
    fn new(label: &str, git: bool) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "dagayn-tools-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create repo");
        let root = root.canonicalize().expect("canonical repo");
        let repo = Self(root);
        repo.write(
            "app.py",
            "def main():\n    return helper()\n\n\ndef helper():\n    pass\n",
        );
        repo.write(
            "test_app.py",
            "from app import main\n\n\ndef test_main():\n    main()\n",
        );
        repo.write(
            "docs/LLM-OPTIMIZED-REFERENCE.md",
            "<section name=\"trust\">\n  Graph reach is not correctness.\n</section>\n",
        );
        if git {
            for args in [
                &["init", "-q", "-b", "main"][..],
                &["add", "-A"],
                &["commit", "-q", "--no-gpg-sign", "-m", "init"],
            ] {
                let out = Command::new("git")
                    .args(args)
                    .current_dir(&repo.0)
                    .env("GIT_AUTHOR_NAME", "t")
                    .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
                    .env("GIT_COMMITTER_NAME", "t")
                    .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .output()
                    .expect("git");
                assert!(out.status.success(), "git {args:?}: {out:?}");
            }
        } else {
            std::fs::create_dir_all(repo.0.join(".git")).expect("mark project root");
        }
        repo
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    fn build(&self) {
        let db = db_path_for_build(&self.0).expect("db path");
        let mut store = GraphStore::open(&db).expect("store");
        full_build(
            &self.0,
            &mut store,
            &BuildOptions {
                recurse_submodules: false,
                postprocess: PostprocessLevel::Full,
            },
        )
        .expect("build");
    }

    fn context(&self) -> Context {
        Context {
            pinned_repo: Some(self.0.clone()),
            ..Context::default()
        }
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Each test that calls a tool by name declares the function that tool
// dispatches to (`dagayn: tests`): `dagayn_tools::call` picks it by the
// name, which the call graph cannot follow (docs/plans/TEST-REACH-TARGET.md).
fn answer(context: &Context, name: &str, arguments: Value) -> Value {
    let payload = call(context, name, &arguments)
        .unwrap_or_else(|| panic!("{name} {arguments} was not answered"));
    let parsed: Value = serde_json::from_str(&payload.text).expect("text is JSON");
    assert_eq!(parsed, payload.value, "text and value agree");
    payload.value
}

fn declines(context: &Context, name: &str, arguments: Value) -> bool {
    call(context, name, &arguments).is_none()
}

#[test]
fn stats_count_the_graph_and_report_the_repository() {
    // dagayn: tests crates/dagayn-tools/src/stats.rs::list_graph_stats
    let repo = Repo::new("stats", false);
    repo.build();
    let stats = answer(&repo.context(), "list_graph_stats_tool", json!({}));
    assert_eq!(stats["status"], "ok");
    assert_eq!(stats["embeddings_count"], 0);
    assert_eq!(stats["files_count"], 3);
    assert_eq!(stats["_repo"]["source"], "explicit");
    assert_eq!(
        stats["_repo"]["repo_root"],
        repo.0.to_string_lossy().as_ref()
    );
    assert_eq!(
        stats["next_tool_suggestions"].as_array().map(Vec::len),
        Some(3)
    );
    let text = call(&repo.context(), "list_graph_stats_tool", &json!({}))
        .expect("answered")
        .text;
    assert!(
        text.starts_with(r#"{"status":"ok","summary":"Graph stats for "#),
        "{text}"
    );
}

#[test]
fn suggestions_follow_the_tool_surface() {
    // dagayn: tests crates/dagayn-tools/src/flow.rs::flow
    // dagayn: tests crates/dagayn-tools/src/stats.rs::list_graph_stats
    let repo = Repo::new("surface", false);
    repo.build();
    let mut context = repo.context();
    context.allowed_tools = Some(["flow_tool".to_string()].into_iter().collect());
    let stats = answer(&context, "list_graph_stats_tool", json!({}));
    assert_eq!(
        stats["next_tool_suggestions"],
        json!(["flow_tool mode=\"list\" -- inspect critical reachable-set flows"])
    );
}

#[test]
fn docs_sections_come_from_the_repository_and_are_truncated_by_characters() {
    // dagayn: tests crates/dagayn-tools/src/docs.rs::get_docs_section
    let repo = Repo::new("docs", false);
    repo.build();
    let context = repo.context();
    let section = answer(
        &context,
        "get_docs_section_tool",
        json!({"section_name": "TRUST"}),
    );
    assert_eq!(section["content"], "Graph reach is not correctness.");
    assert_eq!(section["truncated"], false);
    let cut = answer(
        &context,
        "get_docs_section_tool",
        json!({"section_name": "trust", "max_chars": 5}),
    );
    assert_eq!(cut["content"], "Graph\n... (truncated)");
    assert_eq!(cut["truncated"], true);
    // An unknown section lists the sections the reference file holds.
    let missing = answer(
        &context,
        "get_docs_section_tool",
        json!({"section_name": "nope"}),
    );
    assert_eq!(missing["status"], "not_found");
    assert!(
        missing["error"]
            .as_str()
            .unwrap()
            .starts_with("Section 'nope' not found. Available: ")
    );
    let negative = answer(
        &context,
        "get_docs_section_tool",
        json!({"section_name": "trust", "max_chars": -100000}),
    );
    assert_eq!(negative["content"], "\n... (truncated)");
}

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

/// A context that queues repairs with an interpreter that cannot start, so
/// no worker ever runs.
fn auto_preparing(repo: &Repo) -> Context {
    Context {
        package_root: Some(repo.0.clone()),
        auto_prepare: Some(dagayn_tools::AutoPrepare {
            python_executable: Some(PathBuf::from("/nonexistent/python")),
            budget_seconds: Some(300),
        }),
        ..repo.context()
    }
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

#[test]
fn query_graph_counts_one_row_per_node_everywhere() {
    // dagayn: tests crates/dagayn-tools/src/query.rs::query_graph
    let repo = Repo::new("query-counts", false);
    repo.write(
        "twice.py",
        "from app import helper\n\n\ndef twice():\n    helper()\n    return helper()\n",
    );
    repo.build();
    let context = repo.context();
    let callers = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "app.py::helper"}),
    );
    // main and twice, though twice calls it from two lines.
    assert_eq!(callers["result_count"], 2, "{}", callers["results"]);
    assert!(callers.get("guidance").is_none());
    // full lists one row per edge, and its guidance counts the same rows.
    let full = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "app.py::helper", "detail_level": "full"}),
    );
    assert_eq!(
        full["guidance"][0]["counts"]["result_count"],
        full["result_count"]
    );
    assert!(
        callers["summary"]
            .as_str()
            .is_some_and(|summary| summary.starts_with("Found 2 "))
    );
}

#[test]
fn an_ambiguous_target_names_its_retries() {
    // dagayn: tests crates/dagayn-tools/src/query.rs::query_graph
    // dagayn: tests crates/dagayn-tools/src/flow.rs::flow
    let repo = Repo::new("ambiguous", false);
    repo.write("lib.py", "def helper():\n    pass\n");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let query = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "helper", "depth": 2}),
    );
    assert_eq!(query["status"], "ambiguous");
    let targets: Vec<&Value> = query["next"]
        .as_array()
        .expect("next")
        .iter()
        .map(|call| &call["args"]["target"])
        .collect();
    assert_eq!(targets.len(), 2, "{}", query["next"]);
    assert!(targets.contains(&&json!("app.py::helper")), "{targets:?}");
    assert!(targets.contains(&&json!("lib.py::helper")), "{targets:?}");
    assert_eq!(query["next"][0]["args"]["depth"], 2);
    assert_eq!(query["next"][0]["args"]["pattern"], "callers_of");

    let flow = answer(
        &context,
        "flow_tool",
        json!({"mode": "entry_points", "target": "helper"}),
    );
    assert_eq!(flow["status"], "ambiguous");
    assert_eq!(flow["next"][0]["tool"], "flow_tool");
    assert_eq!(flow["next"][0]["args"]["mode"], "entry_points");
    // `next` is the one place that says what to call.
    assert!(flow.get("_hints").is_none());
    let retried = answer(&context, "flow_tool", flow["next"][0]["args"].clone());
    assert_eq!(retried["status"], "ok");
}

#[test]
fn query_graph_answers_callers_and_callees_of_exact_targets() {
    // dagayn: tests crates/dagayn-tools/src/query.rs::query_graph
    // dagayn: tests crates/dagayn-tools/src/search.rs::semantic_search
    let repo = Repo::new("query", false);
    repo.build();
    let context = repo.context();
    let callers = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "app.py::helper"}),
    );
    assert_eq!(callers["result_count"], 1);
    assert_eq!(callers["results"][0]["qualified_name"], "app.py::main");
    assert_eq!(callers["results"][0]["lines"], json!([2]));
    assert!(callers["results"][0].get("file_path").is_none());
    assert!(callers.get("guidance").is_none());
    assert_eq!(callers["results_complete"], true);
    // Graph-wide health is get_minimal_context_tool's; detail_level="full" keeps it.
    assert!(callers.get("answerability").is_none());
    assert!(callers["missingness"].is_array());
    let full = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "app.py::helper", "detail_level": "full"}),
    );
    assert!(full["answerability"]["counts"].is_object());

    let minimal = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callees_of", "target": "app.py::main", "detail_level": "minimal"}),
    );
    assert!(minimal.get("guidance").is_none());
    assert_eq!(minimal["results"][0]["evidence_type"], "extracted");

    // Found by name: reported by its qualified name.
    let by_name = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "helper"}),
    );
    assert_eq!(by_name["resolution"], "exact_name");
    assert_eq!(by_name["target"], "app.py::helper");
    assert_eq!(by_name["original_target"], "helper");
    let missing = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callees_of", "target": "zz_no_such_symbol"}),
    );
    assert_eq!(missing["status"], "not_found");
    assert_eq!(missing["next"][0]["tool"], "semantic_search_nodes_tool");
    assert!(missing.get("_hints").is_none());

    // The live span, as the worktree holds it.
    let source = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "source_of", "target": "app.py::helper"}),
    );
    assert_eq!(source["results"][0]["source"], "def helper():\n    pass");
    assert_eq!(source["source_coverage"]["truncated"], false);
    assert_eq!(source["status"], "ok");
    repo.write(
        "app.py",
        "def main():\n    return helper()\n\n\ndef helper():\n    return 2\n",
    );
    let stale = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "source_of", "target": "app.py::helper"}),
    );
    assert_eq!(stale["status"], "degraded");
    assert_eq!(stale["results"][0]["source_stale"], true);

    let transitive = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "app.py::helper", "depth": 3}),
    );
    assert_eq!(transitive["reachability"]["state"], "complete");
    assert_eq!(transitive["depth"], 3);
    let full = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "app.py::helper", "detail_level": "full"}),
    );
    assert_eq!(full["edges"].as_array().map(Vec::len), Some(1));
    assert!(full["results"][0].get("id").is_some());
    let children = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "children_of", "target": "app.py"}),
    );
    assert_eq!(children["result_count"], 2);
    let summary = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "file_summary", "target": "missing.py"}),
    );
    assert_eq!(summary["status"], "not_found");
    assert_eq!(summary["pattern"], "file_summary");

    // `test_main` is linked by TESTED_BY and by its module and name.
    let tests = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "app.py::main"}),
    );
    assert_eq!(
        tests["results"][0]["qualified_name"],
        "test_app.py::test_main"
    );
    assert_eq!(tests["results"][0]["confidence"], "high");
    assert_eq!(tests["results"][0]["coverage_source"], "graph_edge");

    for (arguments, message) in [
        (
            json!({"pattern": "callers_of", "target": "app.py::helper", "depth": 0}),
            "depth must be 1 or more, got 0.",
        ),
        (
            json!({"pattern": "callees_of", "target": "app.py::helper", "depth": 2}),
            "depth applies only to ['callers_of', 'importers_of']; 'callees_of' returns \
             direct relationships only.",
        ),
    ] {
        let reply = answer(&context, "query_graph_tool", arguments.clone());
        assert_eq!(reply["status"], "error", "{arguments}");
        assert_eq!(reply["error"], message, "{arguments}");
    }
    let unknown = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "nope", "target": "app.py::helper"}),
    );
    assert!(
        unknown["error"]
            .as_str()
            .unwrap()
            .starts_with("Unknown pattern 'nope'. Available: ['callers_of', 'callees_of',")
    );
}

#[test]
fn query_graph_answers_trimmed_unread_and_dotted_targets() {
    // dagayn: tests crates/dagayn-tools/src/query.rs::query_graph
    let repo = Repo::new("query-gaps", false);
    let many: String = (0..400)
        .map(|i| format!("def function_with_a_long_name_{i:03}():\n    pass\n\n\n"))
        .collect();
    repo.write("many.py", &many);
    repo.build();
    let context = repo.context();

    // `apply_output_budget` halves the rows until the answer fits.
    let trimmed = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "children_of", "target": "many.py", "detail_level": "minimal"}),
    );
    assert_eq!(trimmed["truncated"], true);
    assert_eq!(trimmed["_truncation"]["results"]["total"], 400);
    let kept = trimmed["_truncation"]["results"]["kept"].as_u64().unwrap();
    assert!(kept < 400);
    assert_eq!(
        trimmed["results"].as_array().map(Vec::len),
        Some(kept as usize)
    );
    assert_eq!(trimmed["results_complete"], false);
    assert_eq!(trimmed["result_count"], 400);

    // A target Python resolves without the file existing.
    for target in ["../outside.py", "/nowhere/missing.py", "sub/../missing.py"] {
        let reply = answer(
            &context,
            "query_graph_tool",
            json!({"pattern": "file_summary", "target": target}),
        );
        assert_eq!(reply["status"], "not_found", "{target}");
    }
    let dotted = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "file_summary", "target": "./sub/../app.py"}),
    );
    assert_eq!(dotted["status"], "ok");
    assert_eq!(dotted["result_count"], 3);

    // Every name in `_BUILTIN_CALL_NAMES` is skipped, not only array methods.
    let builtin = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "addEventListener"}),
    );
    assert_eq!(builtin["results"], json!([]));
    assert!(
        builtin["summary"]
            .as_str()
            .unwrap()
            .contains("common builtin")
    );

    // A file gone from the worktree: a `read_error` row, not a decline.
    std::fs::remove_file(repo.0.join("app.py")).expect("remove");
    let gone = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "source_of", "target": "app.py::helper"}),
    );
    assert_eq!(gone["status"], "degraded");
    assert_eq!(gone["results"][0]["read_error"], "not_a_file");
    assert_eq!(gone["results"][0]["source"], "");
    assert_eq!(gone["source_coverage"]["read_error"], "not_a_file");
}

#[test]
fn a_missing_or_foreign_graph_goes_to_python() {
    // dagayn: tests crates/dagayn-tools/src/stats.rs::list_graph_stats
    let repo = Repo::new("nograph", false);
    let context = repo.context();
    assert!(declines(&context, "list_graph_stats_tool", json!({})));
    assert!(declines(
        &context,
        "list_graph_stats_tool",
        json!({"repo_root": "${workspaceFolder}"})
    ));
    let elsewhere: &Path = Path::new("/nonexistent/dagayn-tools");
    assert!(declines(
        &context,
        "list_graph_stats_tool",
        json!({"repo_root": elsewhere.to_string_lossy()})
    ));
}

#[test]
fn search_without_embeddings_ranks_fts_hits() {
    // dagayn: tests crates/dagayn-tools/src/search.rs::semantic_search
    let repo = Repo::new("search", false);
    repo.build();
    let context = repo.context();
    let found = answer(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "helper"}),
    );
    assert_eq!(found["search_mode"], "fts_only");
    assert!(found.get("embedding_health").is_none());
    let verbose = answer(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "helper", "detail_level": "verbose"}),
    );
    assert_eq!(
        verbose["embedding_health"]["status"],
        "provider_unavailable"
    );
    assert_eq!(found["results"][0]["qualified_name"], "app.py::helper");
    assert_eq!(found["exactness"]["exact_match_count"], 1);
    let none = answer(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "zz_nothing", "detail_level": "minimal"}),
    );
    assert_eq!(none["result_count"], 0);
    assert_eq!(none["zero_result_reason"], "not_found_in_current_graph");

    // Anything that embeds the query is Python's.
    assert!(declines(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "x", "provider": "openai"})
    ));
    let mut configured = repo.context();
    configured.embedding_provider = Some("openai".to_string());
    assert!(declines(
        &configured,
        "semantic_search_nodes_tool",
        json!({"query": "helper"})
    ));
    let conn = rusqlite::Connection::open(repo.0.join(".dagayn/graph.db")).expect("open");
    conn.execute(
        "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) \
         VALUES ('app.py::main', x'00000000', 'h', 'local#dim=1')",
        [],
    )
    .expect("insert vector");
    assert!(declines(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "helper"})
    ));
}

#[test]
fn review_answers_affected_flows_from_the_worktree_and_explicit_files() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("review", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({"package": "dagayn", "pid": 1})),
        ..repo.context()
    };
    repo.write(
        "app.py",
        "def main():\n    return helper()\n\n\ndef helper():\n    return 1\n",
    );
    let auto = answer(&context, "review_tool", json!({"mode": "affected_flows"}));
    assert_eq!(auto["change_file_sources"]["unstaged"], json!(["app.py"]));
    assert_eq!(auto["changed_files"], json!(["app.py"]));
    // Only `helper`'s line changed; `main` reaches it, and `test_main`,
    // which calls `main`, is test code the search does not walk.
    assert_eq!(auto["changed_function_count"], 1);
    assert_eq!(
        auto["entry_points"],
        json!([{
            "entry_point": "app.py::main",
            "kind": "main",
            "hops": 1,
            "chain": ["app.py::main", "app.py::helper"],
            "file": "app.py",
            "line": 1,
        }])
    );
    assert!(auto.get("affected_flows").is_none());
    assert!(auto.get("_runtime").is_none());
    assert_eq!(auto["_repo"].as_object().map(|repo| repo.len()), Some(1));
    assert!(auto["next"].is_array());
    assert!(auto.get("_hints").is_none());
    let text = call(&context, "review_tool", &json!({"mode": "affected_flows"}))
        .expect("answered")
        .text;
    assert!(
        text.starts_with(
            r#"{"status":"ok","mode":"affected_flows","summary":"1 entry point(s) reach the 1 changed function(s) in 1 file(s)"#
        ),
        "{text}"
    );
    let verbose = answer(
        &context,
        "review_tool",
        json!({"mode": "affected_flows", "detail_level": "verbose"}),
    );
    assert_eq!(verbose["total"], 1);
    let flow = &verbose["affected_flows"][0];
    assert_eq!(flow["steps"][0]["step_kind"], "entry");
    assert_eq!(flow["bridge_step_count"], 0);
    assert_eq!(flow["missing_step_count"], 0);
    assert_eq!(
        verbose["deprecated_fields"],
        json!(["affected_flows", "total", "_hints"])
    );

    let none = answer(
        &context,
        "review_tool",
        json!({"mode": "affected_flows", "changed_files": []}),
    );
    assert_eq!(none["summary"], "No changed files detected.");
    assert!(none.get("change_file_sources").is_none());
    let explicit = answer(
        &context,
        "review_tool",
        json!({"mode": "affected_flows", "changed_files": ["./app.py", "gone.py"]}),
    );
    assert_eq!(
        explicit["change_file_sources"],
        json!({"files": ["./app.py", "gone.py"], "explicit": ["./app.py", "gone.py"]})
    );
    assert_eq!(explicit["entry_points"][0]["entry_point"], "app.py::main");
}

#[test]
fn review_leaves_other_modes_and_unknowns_to_python() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("review-declines", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    for arguments in [
        json!({"mode": "context", "max_lines_per_file": 1.5}),
        json!({"mode": "affected_flows", "detail_level": "full"}),
        json!({"mode": "affected_flows", "include_source": "yes"}),
        json!({"mode": "affected_flows", "changed_files": [1]}),
        json!({"mode": "affected_flows", "other": 1}),
    ] {
        assert!(
            declines(&context, "review_tool", arguments.clone()),
            "{arguments}"
        );
    }
    // Without the host's `_runtime`, nothing.
    assert!(declines(
        &repo.context(),
        "review_tool",
        json!({"mode": "affected_flows", "changed_files": ["app.py"]})
    ));
    assert!(!declines(
        &context,
        "review_tool",
        json!({"mode": "affected_flows", "changed_files": ["app.py"]})
    ));
}

#[test]
fn review_impact_reports_the_blast_radius_and_trims_like_python() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("impact", true);
    let callers: String = (0..400)
        .map(|i| format!("def caller_{i}():\n    return helper()\n\n\n"))
        .collect();
    repo.write("many.py", &format!("from app import helper\n\n\n{callers}"));
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let impact = |arguments: Value| answer(&context, "review_tool", arguments);

    let small = impact(json!({"mode": "impact", "changed_files": ["test_app.py", "gone.py"]}));
    assert!(small.get("called_subtool").is_none());
    assert_eq!(small["unmatched_changed_files"], json!(["gone.py"]));
    assert!(
        small["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("1 of 2 changed file(s) are NOT in the graph"))
    );
    let reasons: Vec<&str> = small["missingness"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["reason_code"].as_str())
        .collect();
    assert!(
        reasons.contains(&"changed_files_not_in_graph"),
        "{reasons:?}"
    );
    assert!(small["next"].is_array());
    assert!(small.get("_hints").is_none());

    let wide = impact(json!({"mode": "impact", "changed_files": ["app.py"], "max_nodes": 500}));
    assert_eq!(wide["truncated"], true);
    let kept = &wide["_truncation"]["edges"];
    assert!(kept["kept"].as_u64() < kept["total"].as_u64(), "{kept}");
    let text = call(
        &context,
        "review_tool",
        &json!({"mode": "impact", "changed_files": ["app.py"], "max_nodes": 500}),
    )
    .expect("answered")
    .text;
    let (payload_end, trim) = (
        text.find(r#""missingness":"#),
        text.find(r#""_truncation":{"#),
    );
    assert!(
        payload_end.is_some() && trim > payload_end,
        "the trim record follows the payload: {text}"
    );

    let minimal =
        impact(json!({"mode": "impact", "changed_files": ["app.py"], "detail_level": "minimal"}));
    assert_eq!(minimal["risk"], "high");
    assert_eq!(minimal["key_entities"].as_array().map(Vec::len), Some(5));
    assert!(minimal.get("_truncation").is_none());

    let none = impact(json!({"mode": "impact", "changed_files": []}));
    assert_eq!(none["summary"], "No changed files detected.");
    assert_eq!(none["total_impacted"], 0);
}

#[test]
fn review_changes_scores_the_diff_against_base() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    let repo = Repo::new("changes", true);
    repo.build();
    repo.write(
        "app.py",
        "def main():\n    return helper()\n\n\ndef helper():\n    return 1\n\n\ndef auth_token():\n    return 2\n",
    );
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "--no-gpg-sign", "-m", "edit"],
    ] {
        let out = Command::new("git")
            .args(args)
            .current_dir(&repo.0)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .expect("git");
        assert!(out.status.success(), "{out:?}");
    }
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    assert_eq!(changes["mode"], "changes");
    assert!(changes.get("called_subtool").is_none());
    assert_eq!(changes["changed_files"], json!(["app.py"]));
    assert_eq!(changes["base"], "HEAD~1");
    assert!(changes.get("diff_parse_status").is_none());
    assert!(changes.get("risk_level").is_none());
    let names: Vec<&str> = changes["changed_functions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert!(names.contains(&"auth_token"), "{names:?}");
    let added = changes["changed_functions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["name"] == "auth_token")
        .expect("auth_token");
    assert_eq!(added["change_status"], "added");
    // No test reaches the new function: one untested_change for app.py.
    let findings = changes["findings"].as_array().expect("findings");
    let untested = findings
        .iter()
        .find(|f| f["kind"] == "untested_change")
        .expect("untested_change");
    assert_eq!(untested["file"], "app.py");
    assert!(
        untested["targets"]
            .as_array()
            .is_some_and(|t| t.contains(&json!("app.py::auth_token")))
    );
    assert!(
        changes["summary"]
            .as_str()
            .is_some_and(|s| s.starts_with("1 changed file(s), ") && s.contains("untested_change"))
    );
    assert!(changes.get("next_drill_downs").is_none());
    assert!(changes.get("_hints").is_none());
    assert!(
        changes["next"]
            .as_array()
            .is_some_and(|next| !next.is_empty())
    );

    let minimal = answer(
        &context,
        "review_tool",
        json!({"mode": "changes", "detail_level": "minimal"}),
    );
    assert_eq!(minimal["changed_file_count"], 1);
    assert!(minimal.get("changed_functions").is_none());
    assert!(minimal.get("review_priorities").is_none());
    assert_eq!(minimal["findings"], changes["findings"]);

    // verbose keeps the score-first fields for one release.
    let verbose_changes = answer(
        &context,
        "review_tool",
        json!({"mode": "changes", "detail_level": "verbose"}),
    );
    assert_eq!(verbose_changes["diff_parse_status"], "ok");
    assert_eq!(
        verbose_changes["next_drill_downs"]["flows"]["mode"],
        "affected_flows"
    );
    let deprecated = verbose_changes["deprecated_fields"]
        .as_array()
        .expect("deprecated_fields");
    assert!(deprecated.contains(&json!("risk_score")), "{deprecated:?}");
    assert!(
        deprecated.contains(&json!("next_drill_downs")),
        "{deprecated:?}"
    );
    assert!(deprecated.contains(&json!("_hints")), "{deprecated:?}");
    // verbose keeps the diagnosis the agent's envelope leaves out.
    assert_eq!(verbose_changes["called_subtool"], "detect_changes_func");
    assert!(verbose_changes["_runtime"].is_object());
    assert!(verbose_changes["_repo"]["db_path"].is_string());
    assert!(
        verbose_changes["analysis_summary"]["reason_codes"]
            .as_array()
            .is_some()
    );
    assert!(verbose_changes["review_priorities"].as_array().is_some());
    assert!(
        verbose_changes["symbol_delta"]["removed"]
            .as_array()
            .is_some()
    );
    assert!(
        verbose_changes["deprecated_fields"]
            .as_array()
            .is_some_and(|f| f.contains(&json!("analysis_summary")))
    );

    let none = answer(&context, "review_tool", json!({"changed_files": []}));
    assert_eq!(none["summary"], "No changed files detected.");
    assert_eq!(none["findings"], json!([]));

    // An explicit list scopes the review: app.py's lines in the base diff
    // add none of its nodes when only another file is named.
    let scoped = answer(
        &context,
        "review_tool",
        json!({"changed_files": ["notes.txt"]}),
    );
    assert_eq!(scoped["changed_files"], json!(["notes.txt"]));
    assert_eq!(scoped["changed_functions"], json!([]));
    let scoped_context = answer(
        &context,
        "get_minimal_context_tool",
        json!({"changed_files": ["notes.txt"]}),
    );
    assert!(
        !scoped_context.to_string().contains("auth_token"),
        "{scoped_context}"
    );

    let sourced = answer(&context, "review_tool", json!({"include_source": true}));
    let functions = sourced["changed_functions"].as_array().unwrap();
    assert!(!functions.is_empty());
    for function in functions {
        let source = function["source"].as_str().unwrap();
        let first = format!("{}: ", function["line_start"]);
        assert!(source.starts_with(&first), "{source}");
    }
    let verbose = answer(&context, "review_tool", json!({"detail_level": "verbose"}));
    for contract in verbose["analysis_summary"]["stability_contracts"]
        .as_array()
        .unwrap()
    {
        assert_eq!(contract["supplemental_test_density_evaluated"], true);
    }

    // No HEAD~5 to diff against: the unresolved base is reported, without
    // hints.
    let unresolved = answer(
        &context,
        "review_tool",
        json!({"base": "HEAD~5", "changed_files": ["app.py"]}),
    );
    assert_eq!(unresolved["status"], "error");
    assert_eq!(unresolved["diff_parse_status"], "base_unresolved");
    assert!(
        unresolved["error"]
            .as_str()
            .is_some_and(|e| e.starts_with("Could not resolve the diff base 'HEAD~5' in "))
    );
    assert!(unresolved.get("_hints").is_none());
    let codes: Vec<&str> = unresolved["missingness"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["reason_code"].as_str())
        .collect();
    assert!(codes.contains(&"diff_base_unreachable"), "{codes:?}");

    // A ref Python rejects lists the working tree instead.
    let flows = answer(
        &context,
        "review_tool",
        json!({"mode": "affected_flows", "base": "bad ref"}),
    );
    assert_eq!(flows["summary"], "No changed files detected.");

    // No base: a clean tree reviews the last commit, a dirty one only its
    // work in progress.
    assert_eq!(changes["change_entity_summary"]["base"], "HEAD~1");
    repo.write(
        "app.py",
        "def main():\n    return helper()\n\n\ndef helper():\n    return 3\n\n\ndef auth_token():\n    return 2\n",
    );
    repo.build();
    let dirty = answer(&context, "review_tool", json!({}));
    assert_eq!(dirty["change_entity_summary"]["base"], "HEAD");
    let names: Vec<&str> = dirty["changed_functions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert_eq!(names, ["helper"]);
}

#[test]
fn review_context_reads_contained_sources_and_caps_long_files() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("context", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let review = |arguments: Value| answer(&context, "review_tool", arguments);
    let full =
        review(json!({"mode": "context", "changed_files": ["app.py", "../escape.py", "gone.py"]}));
    assert!(full.get("called_subtool").is_none());
    let ctx = &full["context"];
    assert_eq!(
        ctx["source_snippets"]["app.py"],
        "1: def main():\n2:     return helper()\n3: \n4: \n5: def helper():\n6:     pass"
    );
    assert_eq!(ctx["out_of_repo_files"], json!(["../escape.py"]));
    assert_eq!(
        ctx["unmatched_changed_files"],
        json!(["../escape.py", "gone.py"])
    );
    assert!(
        full["summary"]
            .as_str()
            .is_some_and(|s| s.contains("Review guidance:"))
    );

    // Over the line cap and no node of that (relative) path: the first lines.
    let capped =
        review(json!({"mode": "context", "changed_files": ["app.py"], "max_lines_per_file": 2}));
    assert_eq!(
        capped["context"]["source_snippets"]["app.py"],
        "1: def main():\n2:     return helper()\n3: \n4: \n5: def helper():\n6:     pass"
    );
    let minimal =
        review(json!({"mode": "context", "changed_files": ["app.py"], "detail_level": "minimal"}));
    assert_eq!(minimal["risk"], "low");
    assert!(minimal["key_entities"].as_array().is_some_and(|k| {
        k.iter()
            .all(|e| !e.as_str().unwrap_or("/").starts_with('/'))
    }));
    let none = review(json!({"mode": "context", "changed_files": []}));
    assert_eq!(none["summary"], "No changes detected. Nothing to review.");
}

#[test]
fn review_context_fits_its_budget_below_verbose() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("context-budget", true);
    let callers: String = (0..400)
        .map(|i| format!("def caller_{i}():\n    return helper()\n\n\n"))
        .collect();
    repo.write("many.py", &format!("from app import helper\n\n\n{callers}"));
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let arguments = |level: &str| {
        json!({
            "mode": "context",
            "changed_files": ["app.py", "many.py"],
            "max_lines_per_file": 2000,
            "detail_level": level,
        })
    };
    let text = |level: &str| {
        call(&context, "review_tool", &arguments(level))
            .expect("answered")
            .text
    };

    // docs/plans/AGENT-WORKFLOW-TARGET.md#target-contract: 32K at standard.
    let standard = text("standard");
    assert!(standard.len() <= 33_000, "{} characters", standard.len());
    let payload: Value = serde_json::from_str(&standard).expect("json");
    assert_eq!(payload["truncated"], true);
    let snippets = &payload["_truncation"]["source_snippets"];
    assert_eq!(snippets["total"], 2, "{snippets}");

    // verbose keeps the source the budget clips.
    assert!(text("verbose").len() > standard.len());
}

#[test]
fn flow_tool_lists_and_reads_stored_flows() {
    // dagayn: tests crates/dagayn-tools/src/flow.rs::flow
    let repo = Repo::new("flows", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let listed = answer(&context, "flow_tool", json!({}));
    assert!(listed.get("called_subtool").is_none());
    let flows = listed["flows"].as_array().expect("flows");
    assert!(!flows.is_empty());
    assert_eq!(flows[0]["missing_node_count"], 0);
    for key in ["path", "members", "files"] {
        assert!(flows[0].get(key).is_none(), "{key}");
    }
    assert!(listed.get("_hints").is_none());
    let id = flows[0]["id"].clone();
    let minimal = answer(&context, "flow_tool", json!({"detail_level": "minimal"}));
    assert_eq!(minimal["flows"][0]["entry_point"], flows[0]["entry_point"]);

    let got = answer(
        &context,
        "flow_tool",
        json!({"mode": "get", "flow_id": id, "include_source": true}),
    );
    assert!(got.get("called_subtool").is_none());
    assert_eq!(got["status"], "ok");
    let steps = got["flow"]["steps"].as_array().expect("steps");
    assert_eq!(steps[0]["step_kind"], "entry");
    assert!(
        steps[0]["source"]
            .as_str()
            .is_some_and(|s| s.starts_with("1: def "))
    );
    let trimmed = answer(
        &context,
        "flow_tool",
        json!({"mode": "get", "flow_id": id, "detail_level": "minimal"}),
    );
    let flow = &trimmed["flow"];
    assert!(flow.get("path").is_none() && flow.get("members").is_none());
    assert_eq!(flow["steps_omitted"], 0);
    assert_eq!(
        flow["steps"][0]["qualified_name"],
        steps[0]["qualified_name"]
    );
    assert_eq!(flow["steps"][0]["line_start"], steps[0]["line_start"]);
    assert!(flow["steps"][0].get("step_kind").is_none());

    let missing = answer(
        &context,
        "flow_tool",
        json!({"mode": "get", "flow_id": 999}),
    );
    assert_eq!(missing["status"], "not_found");
    assert!(missing.get("_hints").is_none());

    for arguments in [
        json!({"mode": "get"}),
        json!({"mode": "get", "flow_name": ""}),
    ] {
        let reply = answer(&context, "flow_tool", arguments.clone());
        assert_eq!(reply["status"], "error", "{arguments}");
        assert_eq!(reply["called_subtool"], Value::Null, "{arguments}");
        assert_eq!(
            reply["error"],
            "Value error, mode=\"get\" requires flow_id or flow_name."
        );
        assert!(reply["missingness"].is_array());
    }
    for arguments in [
        json!({"sort_by": "bogus"}),
        json!({"flow_id": true, "mode": "get"}),
    ] {
        assert!(
            declines(&context, "flow_tool", arguments.clone()),
            "{arguments}"
        );
    }
}

#[test]
fn flow_tool_finds_the_entry_points_that_reach_a_target() {
    // dagayn: tests crates/dagayn-tools/src/flow.rs::flow
    let repo = Repo::new("entry", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let found = answer(
        &context,
        "flow_tool",
        json!({"mode": "entry_points", "target": "helper"}),
    );
    assert!(found.get("called_subtool").is_none());
    assert_eq!(found["target"], "app.py::helper");
    // `test_main` also calls main, through test code that is not walked.
    assert_eq!(
        found["entry_points"],
        json!([{
            "entry_point": "app.py::main",
            "kind": "main",
            "hops": 1,
            "chain": ["app.py::main", "app.py::helper"],
            "file": "app.py",
            "line": 1,
        }])
    );
    assert_eq!(found["entry_points_omitted"], 0);

    let minimal = answer(
        &context,
        "flow_tool",
        json!({"mode": "entry_points", "target": "app.py::helper", "detail_level": "minimal"}),
    );
    assert!(minimal["entry_points"][0].get("file").is_none());

    let missing = answer(
        &context,
        "flow_tool",
        json!({"mode": "entry_points", "target": "nowhere"}),
    );
    assert_eq!(missing["status"], "not_found");
    // Without a target: the repository's entry points per unit.
    let listed = answer(&context, "flow_tool", json!({"mode": "entry_points"}));
    assert_eq!(listed["entry_point_count"], 1);
    assert_eq!(listed["kinds"], json!({"main": 1}));
    assert_eq!(
        listed["units"][0]["entry_points"][0]["entry_point"],
        "app.py::main"
    );
    assert_eq!(listed["units"][0]["entry_points_omitted"], 0);
}

#[test]
fn architecture_metrics_follow_the_requested_view() {
    // dagayn: tests crates/dagayn-tools/src/arch_tool.rs::architecture
    // dagayn: tests crates/dagayn-tools/src/refactor.rs::refactor
    let repo = Repo::new("arch", true);
    repo.write(
        "pkg/core.py",
        "from app import main\n\n\nclass Base:\n    pass\n",
    );
    repo.write(
        "app.py",
        "from pkg.core import Base\n\n\ndef main():\n    pass\n",
    );
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let arch = |arguments: Value| answer(&context, "architecture_analysis_tool", arguments);
    let sdp = arch(json!({"mode": "sdp_metrics", "granularity": "file", "top_n": 1}));
    assert!(sdp.get("called_subtool").is_none());
    assert!(sdp.get("answerability").is_none());
    assert!(sdp.get("_hints").is_none());
    assert_eq!(sdp["metrics"].as_array().map(Vec::len), Some(1));
    // verbose keeps the earlier next-step fields for one release, named.
    let sdp = arch(json!({"mode": "sdp_metrics", "detail_level": "verbose"}));
    assert!(
        sdp["deprecated_fields"]
            .as_array()
            .is_some_and(|fields| fields.contains(&json!("_hints")))
    );
    let violations = arch(json!({"mode": "sdp_violations", "min_delta": 0}));
    assert!(
        violations["summary"]
            .as_str()
            .is_some_and(|s| s.contains("min_delta=0.0"))
    );
    let sap = arch(json!({"mode": "sap_metrics", "detail_level": "verbose"}));
    assert_eq!(sap["inapplicable_visibility"], "included_in_metrics");
    let tiny = arch(json!({"mode": "sdp_violations", "min_delta": 0.00001}));
    assert!(
        tiny["summary"]
            .as_str()
            .is_some_and(|s| s.contains("min_delta=1e-05)"))
    );
    let sap_v = arch(json!({"mode": "sap_violations", "min_distance": 0.0}));
    assert!(sap_v["violations"].is_array());
    let huge = arch(json!({"mode": "sap_violations", "min_distance": 1.5e16}));
    assert!(
        huge["summary"]
            .as_str()
            .is_some_and(|s| s.contains("min_distance=1.5e+16)"))
    );
    let overview = arch(json!({}));
    assert!(overview.get("called_subtool").is_none());
    assert!(overview["units"].is_array());
    assert!(overview.get("architecture_health").is_none());
    // `app.py` and `pkg/core.py` import each other at module level.
    let cycle = &overview["findings"][0];
    assert_eq!(cycle["kind"], "import_cycle");
    assert_eq!(cycle["targets"], json!(["app.py", "pkg/core.py"]));
    assert_eq!(cycle["cut"].as_array().map(Vec::len), Some(1));
    let verbose = arch(json!({"detail_level": "verbose"}));
    assert_eq!(verbose["architecture_health"]["status"], "ok");
    assert!(verbose["stable_component_policy"]["counts"].is_object());
    assert!(
        verbose["deprecated_fields"]
            .as_array()
            .is_some_and(|fields| fields.contains(&json!("architecture_health")))
    );
    let communities = arch(json!({"mode": "communities", "detail_level": "standard"}));
    assert!(communities.get("called_subtool").is_none());
    assert!(communities.get("answerability").is_none());
    let first = communities["communities"][0]["id"].clone();
    let community =
        arch(json!({"mode": "community", "community_id": first, "include_members": true}));
    assert!(community["community"]["member_details"].is_array());
    let missing = arch(json!({"mode": "community", "community_name": "zz-none"}));
    assert_eq!(missing["status"], "not_found");
    let unselected = arch(json!({"mode": "community"}));
    assert_eq!(unselected["status"], "error");
    assert_eq!(
        unselected["error"],
        "Value error, mode=\"community\" requires community_id or community_name."
    );
    for arguments in [
        json!({"mode": "sdp_metrics", "dependency_profile": "bogus"}),
        // Removed in favour of the overview's map and findings.
        json!({"mode": "hubs"}),
        json!({"mode": "adp_violations"}),
        json!({"mode": "sap_metrics", "unit_filter": "pkg"}),
    ] {
        assert!(
            declines(&context, "architecture_analysis_tool", arguments.clone()),
            "{arguments}"
        );
    }
}

#[test]
fn refactor_finds_dead_code_and_suggests() {
    // dagayn: tests crates/dagayn-tools/src/refactor.rs::refactor
    let repo = Repo::new("refactor", true);
    repo.write("unused.py", "def orphan():\n    return 1\n");
    repo.build();
    let context = repo.context();
    let dead = answer(&context, "refactor_tool", json!({"mode": "dead_code"}));
    let names: Vec<&str> = dead["dead_code"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["name"].as_str())
        .collect();
    assert!(names.contains(&"orphan"), "{names:?}");
    assert!(dead["next"].is_array());
    assert!(dead.get("_hints").is_none());
    let suggest = answer(&context, "refactor_tool", json!({}));
    assert_eq!(suggest["findings"][0]["kind"], "unused_symbol");
    assert_eq!(
        suggest["findings"][0]["qualified_name"],
        "unused.py::orphan"
    );
    assert!(suggest["findings"][0]["evidence"].is_object());
    assert_eq!(suggest["findings_omitted"], json!({}));
    assert_eq!(suggest["summary"], "Findings: 1 unused_symbol.");
    // The size-based suggestions are verbose-only, for one release.
    assert!(suggest.get("suggestions").is_none());
    let minimal = answer(
        &context,
        "refactor_tool",
        json!({"detail_level": "minimal"}),
    );
    assert!(minimal["findings"][0].get("evidence").is_none());
    let verbose = answer(
        &context,
        "refactor_tool",
        json!({"detail_level": "verbose"}),
    );
    assert!(
        verbose["suggestions"]
            .as_array()
            .is_some_and(|s| s.iter().any(|x| x["type"] == "remove"))
    );
    assert!(verbose["suggestions"][0]["work_pack"].is_object());
    assert!(verbose["work_packs"].is_array());
    assert_eq!(verbose["deprecated_fields"][0], "suggestions");
    let steps = &verbose["_hints"]["next_steps"];
    let unique: std::collections::HashSet<String> = steps
        .as_array()
        .into_iter()
        .flatten()
        .map(Value::to_string)
        .collect();
    assert_eq!(unique.len(), steps.as_array().map_or(0, Vec::len));
    let preview = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "orphan", "new_name": "kept"}),
    );
    let id = preview["refactor_id"].as_str().expect("id").to_string();
    assert_eq!(preview["edits"][0]["source"], "definition");
    assert_eq!(preview["edits_omitted"], 0);
    assert_eq!(
        preview["files"],
        json!([{"file": "unused.py", "edit_count": 1}])
    );
    let stored: Value =
        serde_json::from_str(&dagayn_tools::pending::get(&id).expect("pending")).expect("json");
    assert_eq!(stored["new_name"], "kept");
    let bad = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "orphan", "new_name": "1x"}),
    );
    assert_eq!(bad["status"], "error");
    let missing = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "zz_none_qq", "new_name": "x"}),
    );
    assert_eq!(missing["status"], "not_found");
    // Python's `\w`: a letter is one, a combining mark is not; `repr` escapes
    // what is not printable.
    let accented = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "orphan", "new_name": "\u{e9}t\u{e9}"}),
    );
    assert_eq!(accented["status"], "ok", "{accented}");
    let marked = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "orphan", "new_name": "e\u{301}\u{200b}"}),
    );
    assert_eq!(
        marked["error"],
        "new_name is not a valid identifier: 'e\u{301}\\u200b'"
    );
    assert!(declines(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "", "new_name": "b"})
    ));
}

#[test]
fn ensure_graph_answers_only_when_there_is_nothing_to_prepare() {
    // dagayn: tests crates/dagayn-tools/src/ensure.rs::ensure_graph
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/query.rs::query_graph
    let repo = Repo::new("ensure", true);
    assert!(declines(&repo.context(), "ensure_graph_tool", json!({})));
    repo.build();
    let context = repo.context();
    let ready = answer(&context, "ensure_graph_tool", json!({"force": false}));
    assert_eq!(ready["status"], "ok");
    assert_eq!(ready["action"], "noop");
    assert_eq!(ready["reason"], "graph_ready");
    assert_eq!(
        ready["phases"],
        json!({"structure": "noop", "embedding": "not_requested"})
    );
    assert_eq!(ready["sync"]["state"], "commit_synced");
    assert_eq!(ready["sync"]["content_verified"], true);
    assert_eq!(ready["sync"]["current_branch"], "main");
    assert!(ready["graph_health"].get("counts").is_some());
    assert_eq!(ready["worktree_seed"]["reason"], "not_linked_worktree");
    assert_eq!(
        ready["next_tool_suggestions"],
        json!([
            "get_minimal_context_tool",
            "review_tool",
            "query_graph_tool"
        ])
    );

    // A forced refresh, a missing local embedding index, and an edit the
    // graph lacks all run a phase in Python.
    assert!(declines(
        &context,
        "ensure_graph_tool",
        json!({"force": true})
    ));
    let mut embedding = repo.context();
    embedding.local_embedding = Some("bge-m3".to_string());
    assert!(declines(&embedding, "ensure_graph_tool", json!({})));
    repo.write("app.py", "def main():\n    return 1\n");
    assert!(declines(&context, "ensure_graph_tool", json!({})));
}

#[test]
fn ensure_graph_leaves_a_tree_outside_git_to_python() {
    // dagayn: tests crates/dagayn-tools/src/ensure.rs::ensure_graph
    let repo = Repo::new("ensure-nogit", false);
    repo.build();
    // The graph alone marks the project root.
    std::fs::remove_dir_all(repo.0.join(".git")).expect("unmark");
    assert!(declines(&repo.context(), "ensure_graph_tool", json!({})));
}

#[test]
fn large_functions_rank_by_line_count() {
    // dagayn: tests crates/dagayn-tools/src/large.rs::find_large_functions
    let repo = Repo::new("large", true);
    repo.write(
        "big.py",
        &format!("def big():\n{}    return 0\n", "    x = 1\n".repeat(60)),
    );
    repo.build();
    let context = repo.context();
    let found = answer(&context, "find_large_functions_tool", json!({}));
    assert_eq!(found["total_found"], 2, "{found}");
    assert_eq!(found["results"][0]["kind"], "File");
    assert_eq!(found["results"][1]["name"], "big");
    assert_eq!(found["results"][1]["line_count"], 62);
    assert_eq!(found["results"][1]["relative_path"], "big.py");
    let functions = answer(
        &context,
        "find_large_functions_tool",
        json!({"kind": "Function", "file_path_pattern": "big", "min_lines": 10}),
    );
    assert!(
        functions["summary"]
            .as_str()
            .expect("summary")
            .starts_with("Found 1 node(s) with >= 10 lines (kind=Function) matching 'big':")
    );
    assert!(declines(
        &context,
        "find_large_functions_tool",
        json!({"min_lines": "50"})
    ));
}

#[test]
fn traversal_walks_from_the_best_keyword_match() {
    // dagayn: tests crates/dagayn-tools/src/traverse.rs::traverse_graph
    let repo = Repo::new("traverse", true);
    repo.build();
    let context = repo.context();
    let bfs = answer(&context, "traverse_graph_tool", json!({"query": "helper"}));
    assert_eq!(bfs["status"], "ok");
    assert_eq!(bfs["start_node"], "app.py::helper");
    assert_eq!(bfs["traversal"][0]["depth"], 0);
    let names: Vec<&str> = bfs["traversal"]
        .as_array()
        .expect("traversal")
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(names.contains(&"main"), "{names:?}");
    let dfs = answer(
        &context,
        "traverse_graph_tool",
        json!({"query": "helper", "mode": "dfs", "depth": 1}),
    );
    assert_eq!(dfs["max_depth"], 1);
    let tight = answer(
        &context,
        "traverse_graph_tool",
        json!({"query": "helper", "token_budget": 1}),
    );
    assert_eq!(tight["truncated"], true);
    assert_eq!(tight["reachability"]["state"], "truncated");
    let missing = answer(&context, "traverse_graph_tool", json!({"query": "zzzqqq"}));
    assert_eq!(missing["status"], "not_found");
    assert_eq!(missing["reachability"]["state"], "not_found");

    assert!(declines(
        &context,
        "traverse_graph_tool",
        json!({"query": "helper", "mode": "xfs"})
    ));
    assert!(declines(
        &context,
        "traverse_graph_tool",
        json!({"query": "helper", "provider": "openai"})
    ));
}

#[test]
fn suggested_questions_come_high_priority_first() {
    // dagayn: tests crates/dagayn-tools/src/questions.rs::suggested_questions
    let repo = Repo::new("questions", true);
    repo.build();
    let context = repo.context();
    let all = answer(&context, "get_suggested_questions_tool", json!({}));
    assert_eq!(all["status"], "ok");
    let total = all["total"].as_u64().expect("total");
    let none = answer(
        &context,
        "get_suggested_questions_tool",
        json!({"top_n": 0}),
    );
    assert_eq!(none["questions"], json!([]));
    assert_eq!(none["truncated"], total > 0);
    assert_eq!(none["guidance"][0]["confidence"], "low");
}

#[test]
fn wiki_pages_are_read_by_slug_or_exact_name() {
    // dagayn: tests crates/dagayn-tools/src/docs.rs::get_wiki_page
    let repo = Repo::new("wiki", true);
    repo.build();
    repo.write(".dagayn/wiki/auth-flow.md", "# Auth\r\nline\r");
    let context = repo.context();
    let page = answer(
        &context,
        "get_wiki_page_tool",
        json!({"community_name": "Auth  Flow!"}),
    );
    assert_eq!(page["content"], "# Auth\nline\n");
    assert_eq!(page["summary"], "Wiki page for 'Auth  Flow!' (12 chars)");
    let exact = answer(
        &context,
        "get_wiki_page_tool",
        json!({"community_name": "auth-flow.md"}),
    );
    assert_eq!(exact["status"], "ok");
    let escape = answer(
        &context,
        "get_wiki_page_tool",
        json!({"community_name": "../graph.db"}),
    );
    assert_eq!(escape["status"], "not_found");

    // `str.lower()` folds the Kelvin sign to `k`; other non-ASCII is a gap.
    repo.write(".dagayn/wiki/k-v.md", "kelvin");
    let kelvin = answer(
        &context,
        "get_wiki_page_tool",
        json!({"community_name": "\u{212a}認証V"}),
    );
    assert_eq!(kelvin["content"], "kelvin");
    let unnamed = answer(
        &context,
        "get_wiki_page_tool",
        json!({"community_name": "認証"}),
    );
    assert_eq!(unnamed["status"], "not_found");

    // Bytes that are not UTF-8 read as U+FFFD, one per maximal subpart.
    std::fs::write(
        repo.0.join(".dagayn/wiki/bytes.md"),
        b"a\xe2\x82b\xed\xa0\x80\xf0\x9f\x98\x80\xff\r\n",
    )
    .expect("write");
    let bytes = answer(
        &context,
        "get_wiki_page_tool",
        json!({"community_name": "bytes"}),
    );
    assert_eq!(
        bytes["content"],
        "a\u{fffd}b\u{fffd}\u{fffd}\u{fffd}\u{1f600}\u{fffd}\n"
    );

    // A NUL reaches the slug lookup; past it, `Path.resolve()` raises.
    let nul = answer(
        &context,
        "get_wiki_page_tool",
        json!({"community_name": "bytes\u{0}"}),
    );
    assert_eq!(nul["status"], "ok");
    assert!(declines(
        &context,
        "get_wiki_page_tool",
        json!({"community_name": "absent\u{0}"})
    ));
}

#[test]
fn a_rename_preview_is_applied_or_shown_as_a_diff() {
    // dagayn: tests crates/dagayn-tools/src/refactor.rs::refactor
    // dagayn: tests crates/dagayn-tools/src/apply.rs::apply_refactor
    let repo = Repo::new("apply", true);
    repo.build();
    let context = repo.context();
    let rename = |old: &str, new: &str| {
        answer(
            &context,
            "refactor_tool",
            json!({"mode": "rename", "old_name": old, "new_name": new}),
        )["refactor_id"]
            .as_str()
            .expect("refactor_id")
            .to_string()
    };
    let id = rename("helper", "assist");
    let dry = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": id, "dry_run": true}),
    );
    assert_eq!(dry["status"], "ok");
    assert_eq!(dry["would_modify"], json!(["app.py"]));
    let diff = dry["diffs"]["app.py"].as_str().expect("diff");
    assert!(
        diff.starts_with("--- a/app.py\n+++ b/app.py\n@@ -1,6 +1,6 @@\n"),
        "{diff}"
    );
    assert!(diff.contains("-def helper():\n+def assist():\n"), "{diff}");

    let applied = answer(&context, "apply_refactor_tool", json!({"refactor_id": id}));
    assert_eq!(applied["status"], "ok");
    assert_eq!(applied["edits_applied"], 2);
    let source = std::fs::read_to_string(repo.0.join("app.py")).expect("app.py");
    assert!(source.contains("return assist()") && source.contains("def assist():"));
    let gone = answer(&context, "apply_refactor_tool", json!({"refactor_id": id}));
    assert_eq!(
        gone["error"],
        format!("Refactor '{id}' not found or expired.")
    );

    // Bytes that are not UTF-8 are replaced, and the file is written back
    // that way with `\n` line ends, as `read_text`/`write_text` do.
    let id = rename("main", "start");
    std::fs::write(
        repo.0.join("app.py"),
        b"def main():\r\n    return '\xff'\r\n",
    )
    .expect("write");
    let dry = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": id, "dry_run": true}),
    );
    assert!(
        dry["diffs"]["app.py"]
            .as_str()
            .expect("diff")
            .contains("+def start():\n     return '\u{fffd}'\n"),
        "{dry}"
    );
    let applied = answer(&context, "apply_refactor_tool", json!({"refactor_id": id}));
    assert_eq!(applied["status"], "ok", "{applied}");
    assert_eq!(
        std::fs::read_to_string(repo.0.join("app.py")).expect("app.py"),
        "def start():\n    return '\u{fffd}'\n"
    );

    // A preview Python would have stored: a path that does not exist yet
    // is skipped, an empty name matches nothing, and a float line counts.
    let stored = |id: &str, edits: Value| {
        dagayn_tools::pending::set(
            id,
            json!({"created_at": dagayn_tools::pending::now(), "edits": edits}).to_string(),
        );
    };
    stored(
        "odd",
        json!([
            {"file": "gone/new.py", "line": 1, "old": "a", "new": "b"},
            {"file": "app.py", "line": 1, "old": "", "new": "b"},
            {"file": "app.py", "line": 1.0, "old": "start", "new": "begin"},
            {"file": "app.py", "line": null, "old": "start", "new": "begin"},
            {"file": "app.py", "line": 9, "old": "start", "new": "begin"},
        ]),
    );
    let odd = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": "odd", "dry_run": true}),
    );
    assert_eq!(odd["status"], "partial");
    assert_eq!(odd["edits_applied"], 1);
    let reasons: Vec<&str> = odd["skipped"]
        .as_array()
        .expect("skipped")
        .iter()
        .filter_map(|skip| skip["reason"].as_str())
        .collect();
    assert_eq!(
        reasons,
        [
            "file_not_found",
            "line_no_longer_matches",
            "no_line_recorded",
            "line_out_of_range"
        ]
    );
    stored(
        "escape",
        json!([{"file": "../outside.py", "line": 1, "old": "a", "new": "b"}]),
    );
    let escape = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": "escape"}),
    );
    assert_eq!(
        escape["error"],
        "Edit path '../outside.py' is outside repo root."
    );

    // A named root is checked as `_validate_repo_root` does.
    let missing = repo.0.join("no-such-dir");
    let named = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": "odd", "repo_root": missing}),
    );
    assert_eq!(
        named["error"],
        format!(
            "repo_root is not an existing directory: {}",
            missing.display()
        )
    );
    let plain = repo.0.join("docs");
    let named = answer(
        &context,
        "apply_refactor_tool",
        json!({"refactor_id": "odd", "repo_root": plain}),
    );
    assert_eq!(
        named["error"],
        format!(
            "repo_root does not look like a project root (no .git or .dagayn/graph.db found): {}",
            plain.display()
        )
    );
}

/// A one-request-at-a-time OpenAI-compatible endpoint that embeds every
/// query as `vector`.
fn fake_embedding_server(vector: [f32; 4]) -> u16 {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let reply = json!({"data": [{"index": 0, "embedding": vector}]}).to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    port
}

#[test]
fn search_ranks_stored_vectors_against_the_sidecar_query() {
    // dagayn: tests crates/dagayn-tools/src/search.rs::semantic_search
    // dagayn: tests crates/dagayn-tools/src/traverse.rs::traverse_graph
    let repo = Repo::new("vectors", true);
    repo.build();
    let port = fake_embedding_server([0.0, 1.0, 0.0, 0.0]);
    let provider = format!("openai:fake-model@http://127.0.0.1:{port}/v1#dim=4#text=material");
    {
        let conn =
            rusqlite::Connection::open(db_path_for_build(&repo.0).expect("db")).expect("open");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS embeddings (qualified_name TEXT NOT NULL, vector BLOB NOT NULL, \
             text_hash TEXT NOT NULL, provider TEXT NOT NULL, PRIMARY KEY (qualified_name, provider))",
        )
        .expect("schema");
        // Two of the three embeddable nodes: partial coverage.
        let rows: [(&str, [f32; 4]); 2] = [
            ("app.py::main", [1.0, 0.0, 0.0, 0.0]),
            ("app.py::helper", [0.0, 1.0, 0.0, 0.0]),
        ];
        for (name, vector) in rows {
            let blob: Vec<u8> = vector.iter().flat_map(|v| v.to_ne_bytes()).collect();
            conn.execute(
                "INSERT INTO embeddings VALUES (?1, ?2, 'h', ?3)",
                rusqlite::params![name, blob, provider],
            )
            .expect("insert");
        }
    }
    let context = repo.context();
    let found = answer(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "something that assists", "limit": 3, "detail_level": "verbose"}),
    );
    let health = &found["embedding_health"];
    assert_eq!(health["status"], "degraded", "{health}");
    assert_eq!(health["resolved_provider_key"], provider);
    assert_eq!(health["auto_resolved_provider"], provider);
    assert_eq!(health["matching_vector_count"], 2);
    assert_eq!(found["search_mode"], "embedding_only", "{found}");
    assert_eq!(found["results"][0]["qualified_name"], "app.py::helper");
    assert_eq!(found["results"][0]["source"], "embedding");
    let codes: Vec<&str> = found["missingness"]
        .as_array()
        .expect("missingness")
        .iter()
        .filter_map(|item| item["reason_code"].as_str())
        .collect();
    assert!(codes.contains(&"partial_embeddings"), "{codes:?}");
    assert!(!codes.contains(&"missing_embeddings"), "{codes:?}");

    // Traversal starts from the same hybrid top hit.
    let walk = answer(
        &context,
        "traverse_graph_tool",
        json!({"query": "something that assists", "depth": 1}),
    );
    assert_eq!(walk["start_node"], "app.py::helper", "{walk}");

    // A provider named in the call is Python's.
    assert!(declines(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "x", "provider": "openai"})
    ));
    assert!(declines(
        &context,
        "traverse_graph_tool",
        json!({"query": "x", "model": "m"})
    ));
}

#[test]
fn postprocess_reruns_the_steps_asked_for() {
    // dagayn: tests crates/dagayn-tools/src/postprocess.rs::run_postprocess
    // dagayn: tests crates/dagayn-tools/src/query.rs::query_graph
    let repo = Repo::new("postprocess", true);
    repo.build();
    let context = repo.context();
    let all = answer(&context, "run_postprocess_tool", json!({}));
    assert_eq!(all["status"], "ok");
    assert_eq!(all["summary"], "Post-processing complete.");
    assert_eq!(all["signatures_updated"], true);
    assert!(all["fts_indexed"].as_i64().expect("fts") > 0, "{all}");
    assert!(all.get("flows_detected").is_some() && all.get("communities_detected").is_some());
    assert_eq!(all["warnings"], json!([]));
    assert_eq!(
        answer(&context, "run_postprocess_tool", json!({})),
        all,
        "a rerun gives the same counts"
    );
    let only = answer(
        &context,
        "run_postprocess_tool",
        json!({"flows": false, "fts": false}),
    );
    assert!(only.get("fts_indexed").is_none() && only.get("flows_detected").is_none());
    assert!(only.get("communities_detected").is_some());

    // A graph another writer holds is Python's to wait for.
    let db = db_path_for_build(&repo.0).expect("db");
    let _held =
        dagayn_build::GraphLock::acquire(&db, std::time::Duration::from_secs(5)).expect("lock");
    assert!(declines(&context, "run_postprocess_tool", json!({})));
    assert!(dagayn_tools::writes_graph("run_postprocess_tool"));
    assert!(!dagayn_tools::writes_graph("query_graph_tool"));
}

/// docs/plans/AGENT-WORKFLOW-TARGET.md#evaluation, the contract check: every
/// Tier 1 reply carries `next`, at most three calls; every call a reply names
/// (but a shell command) is answered without an error, an ambiguity, or a
/// missing node; and every reply fits its level's budget.
#[test]
fn every_reply_names_calls_that_answer_and_fits_its_budget() {
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    // dagayn: tests crates/dagayn-tools/src/search.rs::semantic_search
    // dagayn: tests crates/dagayn-tools/src/query.rs::query_graph
    // dagayn: tests crates/dagayn-tools/src/flow.rs::flow
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/arch_tool.rs::architecture
    // dagayn: tests crates/dagayn-tools/src/refactor.rs::refactor
    let repo = Repo::new("contract", true);
    // Long paths, as a real tree's are: lists of them must not outgrow a level.
    let module = |i: usize| format!("pkg/a_rather_long_package_name/and_a_subpackage/mod_{i}.py");
    for i in 0..120 {
        repo.write(
            &module(i),
            &format!("from app import helper\n\n\ndef run_{i}():\n    return helper()\n"),
        );
    }
    repo.write(".gitignore", ".dagayn/\n");
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "--no-gpg-sign", "-m", "pkg"],
    ] {
        let out = Command::new("git")
            .args(args)
            .current_dir(&repo.0)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }
    // An uncommitted change to every module: many findings, one review.
    for i in 0..120 {
        repo.write(
            &module(i),
            &format!(
                "from app import helper\n\n\ndef run_{i}(flag=False):\n    return helper() if flag else None\n"
            ),
        );
    }
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let cases: Vec<(&str, Value)> = vec![
        (
            "get_minimal_context_tool",
            json!({"task": "review my change"}),
        ),
        (
            "get_minimal_context_tool",
            json!({"task": "debug why helper fails"}),
        ),
        (
            "get_minimal_context_tool",
            json!({"task": "explain the architecture"}),
        ),
        (
            "get_minimal_context_tool",
            json!({"task": "refactor helper"}),
        ),
        ("get_minimal_context_tool", json!({"task": "helper"})),
        ("semantic_search_nodes_tool", json!({"query": "helper"})),
        (
            "semantic_search_nodes_tool",
            json!({"query": "app", "detail_level": "minimal"}),
        ),
        (
            "query_graph_tool",
            json!({"pattern": "source_of", "target": "app.py::helper"}),
        ),
        (
            "query_graph_tool",
            json!({"pattern": "callers_of", "target": "app.py::helper"}),
        ),
        (
            "query_graph_tool",
            json!({"pattern": "callers_of", "target": "app.py::helper", "detail_level": "minimal"}),
        ),
        (
            "query_graph_tool",
            json!({"pattern": "tests_for", "target": "app.py::main"}),
        ),
        (
            "query_graph_tool",
            json!({"pattern": "importers_of", "target": "app.py"}),
        ),
        (
            "query_graph_tool",
            json!({"pattern": "file_summary", "target": "app.py"}),
        ),
        (
            "query_graph_tool",
            json!({"pattern": "callers_of", "target": "no_such_symbol"}),
        ),
        (
            "flow_tool",
            json!({"mode": "entry_points", "target": "app.py::helper"}),
        ),
        ("review_tool", json!({"mode": "changes"})),
        (
            "review_tool",
            json!({"mode": "changes", "detail_level": "minimal"}),
        ),
        ("review_tool", json!({"mode": "impact"})),
        (
            "review_tool",
            json!({"mode": "impact", "detail_level": "minimal"}),
        ),
        ("review_tool", json!({"mode": "context"})),
        (
            "review_tool",
            json!({"mode": "context", "detail_level": "minimal"}),
        ),
        ("review_tool", json!({"mode": "affected_flows"})),
        ("architecture_analysis_tool", json!({})),
        (
            "architecture_analysis_tool",
            json!({"detail_level": "standard"}),
        ),
        ("refactor_tool", json!({})),
        ("refactor_tool", json!({"detail_level": "minimal"})),
    ];
    let mut failures = Vec::new();
    let mut followed = 0;
    for (tool, arguments) in &cases {
        let Some(payload) = call(&context, tool, arguments) else {
            failures.push(format!("{tool} {arguments}: not answered"));
            continue;
        };
        let budget = match arguments["detail_level"].as_str() {
            Some("minimal") => 8_000,
            // get_minimal_context_tool has no level; it is held to minimal.
            _ if *tool == "get_minimal_context_tool" => 8_000,
            _ => 32_000,
        };
        if payload.text.len() > budget {
            failures.push(format!(
                "{tool} {arguments}: {} characters, budget {budget}",
                payload.text.len()
            ));
        }
        let Some(next) = payload.value["next"].as_array() else {
            failures.push(format!("{tool} {arguments}: no next"));
            continue;
        };
        if next.len() > 3 {
            failures.push(format!("{tool} {arguments}: {} calls in next", next.len()));
        }
        for step in next {
            let (Some(name), Some(args)) = (step["tool"].as_str(), step.get("args")) else {
                failures.push(format!("{tool} {arguments}: malformed {step}"));
                continue;
            };
            if step["why"].as_str().is_none_or(str::is_empty) {
                failures.push(format!("{tool} {arguments}: {step} says no why"));
            }
            if name == "shell" {
                continue;
            }
            followed += 1;
            match call(&context, name, args) {
                None => failures.push(format!("{tool} {arguments} -> {name} {args}: not answered")),
                Some(reply) => {
                    let status = reply.value["status"].as_str().unwrap_or("");
                    if matches!(status, "error" | "ambiguous" | "not_found") {
                        failures.push(format!(
                            "{tool} {arguments} -> {name} {args}: {status} {}",
                            reply.value["summary"]
                        ));
                    }
                }
            }
        }
    }
    assert!(followed >= 10, "only {followed} calls followed");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Runs `first` and then each reply's `next[0]` (but a shell command), at
/// most `limit` calls in all; the calls made, with their replies.
fn follow_next(
    context: &Context,
    first: (&str, Value),
    limit: usize,
) -> Vec<(String, Value, Value)> {
    let mut trace = Vec::new();
    let (mut tool, mut args) = (first.0.to_string(), first.1);
    while trace.len() < limit {
        let reply = answer(context, &tool, args.clone());
        let step = reply["next"]
            .as_array()
            .and_then(|next| next.iter().find(|call| call["tool"] != "shell"))
            .cloned();
        trace.push((tool, args, reply));
        let Some(step) = step else {
            break;
        };
        tool = step["tool"].as_str().expect("tool").to_string();
        args = step["args"].clone();
    }
    trace
}

/// docs/plans/AGENT-WORKFLOW-TARGET.md#evaluation, the follow-the-next
/// traces: from the first call of a task, following `next[0]` reaches the
/// answer within six calls.
#[test]
fn following_next_reaches_the_answer() {
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    // dagayn: tests crates/dagayn-tools/src/query.rs::query_graph
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("traces", true);
    repo.write(".gitignore", ".dagayn/\n");
    repo.write("lib.py", "def helper():\n    return 2\n");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let shown = |trace: &[(String, Value, Value)]| {
        trace
            .iter()
            .map(|(tool, args, reply)| format!("{tool} {args} -> {}", reply["status"]))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // A symptom: the trace finds the function and reads who calls it.
    let trace = follow_next(
        &context,
        (
            "get_minimal_context_tool",
            json!({"task": "debug why main fails"}),
        ),
        6,
    );
    assert!(
        trace
            .iter()
            .any(|(tool, args, reply)| tool == "query_graph_tool"
                && args["pattern"] == "callers_of"
                && args["target"] == "app.py::main"
                && reply["status"] == "ok"),
        "{}",
        shown(&trace)
    );

    // An ambiguous name: one retry, then an answer.
    let trace = follow_next(
        &context,
        (
            "query_graph_tool",
            json!({"pattern": "callers_of", "target": "helper"}),
        ),
        2,
    );
    assert_eq!(trace[0].2["status"], "ambiguous", "{}", shown(&trace));
    assert_eq!(trace[1].2["status"], "ok", "{}", shown(&trace));

    // A change that nothing tests: the review names it.
    repo.write(
        "app.py",
        "def main():\n    return helper()\n\n\ndef helper():\n    pass\n\n\ndef added():\n    return 1\n",
    );
    repo.build();
    let trace = follow_next(
        &context,
        (
            "get_minimal_context_tool",
            json!({"task": "review my change"}),
        ),
        6,
    );
    assert!(
        trace.iter().any(|(tool, _, reply)| tool == "review_tool"
            && reply["findings"]
                .as_array()
                .is_some_and(|findings| findings.iter().any(|f| f["kind"] == "untested_change"))),
        "{}",
        shown(&trace)
    );
}

/// docs/plans/AGENT-WORKFLOW-TARGET.md#evaluation, the discrimination check:
/// on a graph that matches its commit, no reply carries a medium or high
/// caveat about the graph.
#[test]
fn a_fresh_graph_raises_no_caveats() {
    // dagayn: tests crates/dagayn-tools/src/search.rs::semantic_search
    // dagayn: tests crates/dagayn-tools/src/query.rs::query_graph
    // dagayn: tests crates/dagayn-tools/src/flow.rs::flow
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/arch_tool.rs::architecture
    // dagayn: tests crates/dagayn-tools/src/refactor.rs::refactor
    let repo = Repo::new("fresh", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let graph_codes = [
        "stale_derived_structures",
        "missing_flows",
        "missing_communities",
        "graph_describes_another_commit",
        "uncommitted_changes_may_be_unindexed",
        "graph_built_by_older_extractor",
        "many_unresolved_cross_artifact_edges",
    ];
    let cases = [
        ("semantic_search_nodes_tool", json!({"query": "helper"})),
        (
            "query_graph_tool",
            json!({"pattern": "callers_of", "target": "app.py::helper"}),
        ),
        (
            "flow_tool",
            json!({"mode": "entry_points", "target": "app.py::helper"}),
        ),
        ("review_tool", json!({"mode": "changes", "base": "HEAD"})),
        ("architecture_analysis_tool", json!({})),
        ("refactor_tool", json!({})),
    ];
    let mut noisy = Vec::new();
    for (tool, arguments) in cases {
        let reply = answer(&context, tool, arguments.clone());
        for item in reply["missingness"].as_array().into_iter().flatten() {
            let code = item["reason_code"].as_str().unwrap_or("");
            if graph_codes.contains(&code) && item["severity"] != "low" {
                noisy.push(format!("{tool} {arguments}: {code}"));
            }
        }
    }
    assert!(noisy.is_empty(), "{}", noisy.join("\n"));
}

/// `git add -A && git commit` in `repo`.
fn commit_all(repo: &Repo, message: &str) {
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "--no-gpg-sign", "-m", message],
    ] {
        let out = Command::new("git")
            .args(args)
            .current_dir(&repo.0)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }
}

/// A Cargo workspace in layers: `kernel` (used by `feature` and `app`),
/// `feature`, `app`, and `plugins`, which depends on two leaf crates.
fn layered_workspace(repo: &Repo) {
    let crate_manifest = |name: &str, deps: &[&str]| {
        let mut text =
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        if !deps.is_empty() {
            text.push_str("\n[dependencies]\n");
            for dep in deps {
                text.push_str(&format!("{dep} = {{ path = \"../{dep}\" }}\n"));
            }
        }
        text
    };
    repo.write(".gitignore", ".dagayn/\n");
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"kernel\", \"feature\", \"app\", \"plugins\", \"extras\", \"thirdparty\"]\n",
    );
    for (name, deps) in [
        ("kernel", &[][..]),
        ("feature", &["kernel"][..]),
        ("app", &["feature", "kernel"][..]),
        ("plugins", &["extras", "thirdparty"][..]),
        ("extras", &[][..]),
        ("thirdparty", &[][..]),
    ] {
        repo.write(&format!("{name}/Cargo.toml"), &crate_manifest(name, deps));
    }
    repo.write("kernel/src/lib.rs", "pub fn base() -> i64 {\n    1\n}\n");
    repo.write(
        "feature/src/lib.rs",
        "use kernel::base;\n\npub fn feature() -> i64 {\n    base() + 1\n}\n",
    );
    repo.write(
        "app/src/lib.rs",
        "use feature::feature;\nuse kernel::base;\n\npub fn run() -> i64 {\n    feature() + base()\n}\n",
    );
    repo.write(
        "plugins/src/lib.rs",
        "use extras::extra;\nuse thirdparty::vendored;\n\npub fn plugin() -> i64 {\n    extra() + vendored()\n}\n",
    );
    repo.write("extras/src/lib.rs", "pub fn extra() -> i64 {\n    2\n}\n");
    repo.write(
        "thirdparty/src/lib.rs",
        "pub fn vendored() -> i64 {\n    3\n}\n",
    );
}

/// docs/plans/STABILITY-FINDING-TARGET.md: the overview names every unit
/// that depends on a less stable one; a review names the ones the change
/// introduced, and not a dependency that predates it.
#[test]
fn unstable_dependencies_are_found_where_they_are_introduced() {
    // dagayn: tests crates/dagayn-tools/src/arch_tool.rs::architecture
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("stability", true);
    layered_workspace(&repo);
    commit_all(&repo, "layers");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let unstable = |reply: &Value| -> Vec<Value> {
        reply["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|finding| finding["kind"] == "unstable_dependency")
            .cloned()
            .collect()
    };

    // Layered as SDP asks: nothing to report.
    let overview = answer(&context, "architecture_analysis_tool", json!({}));
    assert!(unstable(&overview).is_empty(), "{}", overview["findings"]);

    // An unstable crate starting to use another is no finding either.
    repo.write(
        "app/src/lib.rs",
        "use feature::feature;\nuse kernel::base;\nuse plugins::plugin;\n\npub fn run() -> i64 {\n    feature() + base() + plugin()\n}\n",
    );
    repo.build();
    let review = answer(&context, "review_tool", json!({"base": "HEAD"}));
    assert!(unstable(&review).is_empty(), "{}", review["findings"]);
    commit_all(&repo, "app uses plugins");

    // The stable kernel starts using plugins: the review and the overview
    // both name it, at the import.
    repo.write(
        "kernel/src/lib.rs",
        "use plugins::plugin;\n\npub fn base() -> i64 {\n    1 + plugin()\n}\n",
    );
    repo.build();
    let review = answer(&context, "review_tool", json!({"base": "HEAD"}));
    let found = unstable(&review);
    assert_eq!(found.len(), 1, "{}", review["findings"]);
    let finding = &found[0];
    assert_eq!(finding["targets"], json!(["kernel", "plugins"]));
    assert_eq!(finding["file"], "kernel/src/lib.rs");
    assert_eq!(finding["line"], 1);
    let evidence = &finding["evidence"];
    assert!(
        evidence["target"]["instability"].as_f64() > evidence["source"]["instability"].as_f64(),
        "{evidence}"
    );
    assert!(evidence["delta"].as_f64() > Some(0.1), "{evidence}");
    assert!(
        review["next"].as_array().is_some_and(|next| next
            .iter()
            .any(|call| call["args"]["target"] == "kernel/src/lib.rs")),
        "{}",
        review["next"]
    );
    let overview = answer(&context, "architecture_analysis_tool", json!({}));
    assert_eq!(unstable(&overview).len(), 1, "{}", overview["findings"]);

    // Once committed, editing another line of kernel does not introduce it
    // again; the overview still reports it.
    commit_all(&repo, "kernel uses plugins");
    repo.write(
        "kernel/src/lib.rs",
        "use plugins::plugin;\n\npub fn base() -> i64 {\n    2 + plugin()\n}\n",
    );
    repo.build();
    let review = answer(&context, "review_tool", json!({"base": "HEAD"}));
    assert!(unstable(&review).is_empty(), "{}", review["findings"]);
    let overview = answer(&context, "architecture_analysis_tool", json!({}));
    assert_eq!(unstable(&overview).len(), 1, "{}", overview["findings"]);
}

/// Calls the Rust call graph used to miss, resolved across files: a method
/// on a closure parameter over the elements of what another file's
/// function returns, and methods passed as values (docs/plans/TEST-REACH-TARGET.md#remaining-gaps).
#[test]
fn closures_over_returned_elements_and_method_values_reach_their_targets() {
    let repo = Repo::new("rust-gaps", false);
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("src/lib.rs", "pub mod model;\npub mod run;\n");
    repo.write(
        "src/model.rs",
        "pub struct Dep {\n    x: i64,\n}\n\nimpl Dep {\n    pub fn introduced(&self) -> bool {\n        self.x > 0\n    }\n\n    pub fn finding(&self) -> i64 {\n        self.x\n    }\n}\n\npub fn make() -> Vec<Dep> {\n    Vec::new()\n}\n",
    );
    repo.write(
        "src/run.rs",
        "use crate::model::{make, Dep};\n\npub fn run() -> Vec<i64> {\n    make().iter().filter(|dep| dep.introduced()).map(Dep::finding).collect()\n}\n\npub fn other() -> Vec<i64> {\n    make().iter().map(crate::model::Dep::finding).collect()\n}\n",
    );
    repo.build();
    let store = GraphStore::open(db_path_for_build(&repo.0).expect("db")).expect("store");
    let (_, incoming) = store
        .get_edges_by_endpoints(&[
            "src/model.rs::Dep.introduced".to_string(),
            "src/model.rs::Dep.finding".to_string(),
        ])
        .expect("edges");
    let into = |target: &str| -> Vec<(String, String)> {
        let mut found: Vec<(String, String)> = incoming
            .get(target)
            .into_iter()
            .flatten()
            .filter(|edge| edge.kind != "CONTAINS")
            .map(|edge| (edge.kind.clone(), edge.source_qualified.clone()))
            .collect();
        found.sort();
        found
    };
    assert_eq!(
        into("src/model.rs::Dep.introduced"),
        [("CALLS".to_string(), "src/run.rs::run".to_string())]
    );
    assert_eq!(
        into("src/model.rs::Dep.finding"),
        [
            ("REFERENCES".to_string(), "src/run.rs::other".to_string()),
            ("REFERENCES".to_string(), "src/run.rs::run".to_string()),
        ]
    );
}

#[test]
fn a_test_that_uses_a_class_reaches_the_methods_the_runtime_calls() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    // dagayn: tests crates/dagayn-tools/src/findings.rs::implicit_caller
    let repo = Repo::new("implicit-methods", true);
    let models = |value: i64| {
        format!(
            "class Box:\n    def __init__(self):\n        self.value = {value}\n\n    @property\n    def size(self):\n        return self.value + {value}\n\n    def grow(self):\n        self.value += {value}\n"
        )
    };
    repo.write("models.py", &models(1));
    repo.write(
        "test_models.py",
        "from models import Box\n\n\ndef test_size():\n    assert Box().size == 2\n",
    );
    commit_all(&repo, "models");
    repo.build();
    repo.write("models.py", &models(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let untested: Vec<&str> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .flat_map(|f| f["targets"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    // Constructing `Box` calls `__init__`; reading `size` calls the
    // property. Nothing calls `grow`.
    assert_eq!(untested, ["models.py::Box.grow"], "{changes}");
}

#[test]
fn importing_a_module_reaches_what_its_import_runs() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    // dagayn: tests crates/dagayn-tools/src/findings.rs::implicit_caller
    let repo = Repo::new("import-reach", true);
    let specs = |value: i64| {
        format!(
            "class Spec:\n    def __init__(self):\n        self.value = {value}\n\n    def grow(self):\n        self.value += {value}\n\n\nclass Store:\n    def __init__(self):\n        self.items = [{value}]\n\n\ndef make_store():\n    return Store()\n\n\ndef unused():\n    return {value}\n\n\nDEFAULT = Spec()\nSTORE = make_store()\n"
        )
    };
    let lazy = |value: i64| format!("def __getattr__(name):\n    return {value}\n");
    repo.write("specs.py", &specs(1));
    repo.write("lazy/__init__.py", &lazy(1));
    repo.write(
        "test_specs.py",
        "import specs\nimport lazy\n\n\ndef test_default():\n    assert specs.DEFAULT and lazy.anything\n",
    );
    commit_all(&repo, "specs");
    repo.build();
    repo.write("specs.py", &specs(2));
    repo.write("lazy/__init__.py", &lazy(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let mut untested: Vec<&str> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .flat_map(|f| f["targets"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    untested.sort_unstable();
    // Importing `specs` builds `DEFAULT` (`Spec.__init__`) and, through
    // `make_store`, `STORE` (`Store.__init__`); an attribute of `lazy` runs
    // its `__getattr__`. Nothing calls `grow` or `unused`.
    assert_eq!(
        untested,
        ["specs.py::Spec.grow", "specs.py::unused"],
        "{changes}"
    );
}

#[test]
fn a_rust_test_any_number_of_hops_away_counts_and_a_python_one_does_not() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/findings.rs::caller_test_depth
    // dagayn: tests crates/dagayn-tools/src/findings.rs::untested_changes
    let repo = Repo::new("deep-chains", true);
    // `f0` calls `f1` ... `f6`; the test of `f0` is six hops from `f6`.
    let rust = |value: i64| {
        let mut source: String = (0..6)
            .map(|i| format!("pub fn f{i}() -> i64 {{\n    f{}()\n}}\n\n", i + 1))
            .collect();
        source.push_str(&format!("pub fn f6() -> i64 {{\n    {value}\n}}\n\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn chain() {{\n        assert!(super::f0() > 0);\n    }}\n}}\n"));
        source
    };
    let python = |value: i64| {
        let mut source: String = (0..6)
            .map(|i| format!("def f{i}():\n    return f{}()\n\n\n", i + 1))
            .collect();
        source.push_str(&format!("def f6():\n    return {value}\n"));
        source
    };
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("src/lib.rs", &rust(1));
    repo.write("chain.py", &python(1));
    repo.write(
        "test_chain.py",
        "from chain import f0\n\n\ndef test_chain():\n    assert f0()\n",
    );
    commit_all(&repo, "chains");
    repo.build();
    repo.write("src/lib.rs", &rust(2));
    repo.write("chain.py", &python(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let untested: Vec<&Value> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .collect();
    assert_eq!(untested.len(), 1, "{changes}");
    assert_eq!(untested[0]["targets"], json!(["chain.py::f6"]), "{changes}");
    assert_eq!(untested[0]["evidence"][1]["caller_depth"], 4, "{changes}");
    let reach = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "src/lib.rs::f6"}),
    );
    assert_eq!(reach["test_reach"]["hops"], 6, "{reach}");
    assert_eq!(reach["test_reach"]["counts_as_tested"], true, "{reach}");
    assert_eq!(reach["test_reach"]["hop_limit"], Value::Null, "{reach}");
}

#[test]
fn importing_a_module_reaches_its_top_level_calls_only() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    // dagayn: tests crates/dagayn-tools/src/findings.rs::runs_on_import
    let repo = Repo::new("import-time-calls", true);
    let tables = |value: i64| {
        format!(
            "def build_table():\n    return [{value}]\n\n\ndef helper():\n    return {value}\n\n\ndef fallback():\n    return {value}\n\n\ndef main():\n    return {value}\n\n\nTABLE = build_table()\nHANDLERS = {{\"a\": lambda: helper()}}\ntry:\n    import json\nexcept ImportError:\n    fallback()\n\nif __name__ == \"__main__\":\n    main()\n"
        )
    };
    repo.write("tables.py", &tables(1));
    repo.write(
        "test_tables.py",
        "import tables\n\n\ndef test_table():\n    assert tables.TABLE\n",
    );
    commit_all(&repo, "tables");
    repo.build();
    repo.write("tables.py", &tables(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let mut untested: Vec<&str> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .flat_map(|f| f["targets"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    untested.sort_unstable();
    // Importing `tables` runs `build_table()`; a lambda's body, an `except`
    // handler, and the `__main__` block do not run on import.
    assert_eq!(
        untested,
        [
            "tables.py::fallback",
            "tables.py::helper",
            "tables.py::main"
        ],
        "{changes}"
    );
}
