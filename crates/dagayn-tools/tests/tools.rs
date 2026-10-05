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

    // One commit: `HEAD~1` does not resolve, so no node changed, but the
    // risk is scored (zero) all the same.
    let risky = answer(
        &context,
        "get_minimal_context_tool",
        json!({"changed_files": ["app.py"]}),
    );
    assert_eq!(risky["risk"], "low");
    assert!(
        risky["summary"]
            .as_str()
            .expect("summary")
            .ends_with("Review priority: low (0.00).")
    );
    let against_head = answer(
        &context,
        "get_minimal_context_tool",
        json!({"changed_files": ["app.py"], "base": "HEAD"}),
    );
    // Every node of a changed file, not only those its hunks touch.
    assert_eq!(against_head["key_entities"], json!(["helper", "main"]));
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
    assert_eq!(
        observed["next_tool_suggestions"],
        json!([
            "ensure_graph_tool",
            "review_tool",
            "semantic_search_nodes_tool",
            "architecture_analysis_tool"
        ])
    );
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
    assert_eq!(
        observed["recommended_action"],
        "Call ensure_graph_tool to sync the graph."
    );
    assert_eq!(observed["why"], "sync.state=commit_drift");
    assert_eq!(
        observed["next_tool_suggestions"][0],
        json!("ensure_graph_tool")
    );
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
    assert_eq!(
        repaired["recommended_action"],
        "Graph repair is queued; call ensure_graph_tool only if you must wait for it."
    );
    assert_eq!(repaired["repair"]["kind"], "prepare");
    assert_eq!(repaired["_repo"]["source"], "explicit");
}

#[test]
fn minimal_context_queues_missing_local_embeddings() {
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
    assert_eq!(queued["recommended_action"], observed["recommended_action"]);
    assert_eq!(
        queued_tasks(&repo)[0].3,
        r#"{"local_embedding": "bge-m3", "keep_local_embedding_server": true, "budget_seconds": 300}"#
    );
}

#[test]
fn minimal_context_never_queues_outside_a_repository() {
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
fn query_graph_answers_callers_and_callees_of_exact_targets() {
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
    assert_eq!(callers["guidance"][0]["counts"]["result_count"], 1);
    assert_eq!(callers["results_complete"], true);
    assert_eq!(
        callers["answerability"].as_object().map(|map| map.len()),
        Some(3)
    );

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
    assert_eq!(
        missing["_hints"]["warnings"],
        json!(["not_found_in_current_graph"])
    );

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
fn a_missing_or_foreign_graph_goes_to_python() {
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
    let repo = Repo::new("search", false);
    repo.build();
    let context = repo.context();
    let found = answer(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "helper"}),
    );
    assert_eq!(found["search_mode"], "fts_only");
    assert_eq!(found["embedding_health"]["status"], "provider_unavailable");
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
    assert_eq!(auto["total"], 1);
    let flow = &auto["affected_flows"][0];
    assert_eq!(flow["steps"][0]["step_kind"], "entry");
    assert_eq!(flow["bridge_step_count"], 0);
    assert_eq!(flow["missing_step_count"], 0);
    assert_eq!(auto["_runtime"]["pid"], 1);
    assert_eq!(auto["_hints"]["next_steps"][0]["tool"], "review_tool");
    let text = call(&context, "review_tool", &json!({"mode": "affected_flows"}))
        .expect("answered")
        .text;
    assert!(
        text.starts_with(
            r#"{"status":"ok","mode":"affected_flows","called_subtool":"get_affected_flows_func","summary":"1 flow(s) affected"#
        ),
        "{text}"
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
    assert_eq!(explicit["total"], 1);
}

#[test]
fn review_leaves_other_modes_and_unknowns_to_python() {
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
    assert_eq!(small["called_subtool"], "get_impact_radius");
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
    assert_eq!(small["_hints"]["next_steps"][0]["tool"], "review_tool");

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
    assert!(
        text.contains(r#""guidance":[],"_truncation":{"#),
        "the trim record follows the payload"
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
    assert_eq!(changes["called_subtool"], "detect_changes_func");
    assert_eq!(changes["changed_files"], json!(["app.py"]));
    assert_eq!(changes["diff_parse_status"], "ok");
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
    assert!(
        changes["summary"]
            .as_str()
            .is_some_and(|s| s.starts_with("Analyzed 1 changed file(s):"))
    );
    let summary = &changes["analysis_summary"];
    assert!(summary["reason_codes"].as_array().is_some());
    assert_eq!(
        summary["next_drill_downs"]["flows"]["mode"],
        "affected_flows"
    );
    assert!(changes["_hints"]["next_steps"].as_array().is_some());

    let minimal = answer(
        &context,
        "review_tool",
        json!({"mode": "changes", "detail_level": "minimal"}),
    );
    assert_eq!(minimal["changed_file_count"], 1);
    assert!(minimal.get("changed_functions").is_none());
    assert!(
        minimal["review_priorities"]
            .as_array()
            .is_some_and(|p| p.len() <= 3)
    );

    let none = answer(&context, "review_tool", json!({"changed_files": []}));
    assert_eq!(none["summary"], "No changed files detected.");
    assert_eq!(none["risk_score"], 0.0);

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
}

#[test]
fn review_context_reads_contained_sources_and_caps_long_files() {
    let repo = Repo::new("context", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let review = |arguments: Value| answer(&context, "review_tool", arguments);
    let full =
        review(json!({"mode": "context", "changed_files": ["app.py", "../escape.py", "gone.py"]}));
    assert_eq!(full["called_subtool"], "get_review_context");
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
fn flow_tool_lists_and_reads_stored_flows() {
    let repo = Repo::new("flows", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let listed = answer(&context, "flow_tool", json!({}));
    assert_eq!(listed["called_subtool"], "list_flows");
    let flows = listed["flows"].as_array().expect("flows");
    assert!(!flows.is_empty());
    assert_eq!(flows[0]["missing_node_count"], 0);
    assert_eq!(listed["_hints"]["next_steps"][0]["tool"], "flow_tool");
    let id = flows[0]["id"].clone();

    let got = answer(
        &context,
        "flow_tool",
        json!({"mode": "get", "flow_id": id, "include_source": true}),
    );
    assert_eq!(got["called_subtool"], "get_flow");
    assert_eq!(got["status"], "ok");
    let steps = got["flow"]["steps"].as_array().expect("steps");
    assert_eq!(steps[0]["step_kind"], "entry");
    assert!(
        steps[0]["source"]
            .as_str()
            .is_some_and(|s| s.starts_with("1: def "))
    );

    let missing = answer(
        &context,
        "flow_tool",
        json!({"mode": "get", "flow_id": 999}),
    );
    assert_eq!(missing["status"], "not_found");
    assert_eq!(missing["_hints"]["next_steps"][0]["tool"], "flow_tool");

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
fn architecture_metrics_follow_the_requested_view() {
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
    let adp = arch(json!({"mode": "adp_violations"}));
    assert_eq!(adp["called_subtool"], "detect_adp_violations_func");
    assert_eq!(adp["count"], 1);
    assert_eq!(adp["violations"][0]["nodes"], json!(["<root>", "pkg"]));
    assert!(adp["answerability"].is_object());
    let sdp = arch(json!({"mode": "sdp_metrics", "granularity": "file", "top_n": 1}));
    assert_eq!(sdp["metrics"].as_array().map(Vec::len), Some(1));
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
    let hubs = arch(json!({"mode": "hubs", "artifact_scope": "all"}));
    assert_eq!(hubs["called_subtool"], "get_hub_nodes_func");
    assert!(hubs["hub_nodes"].as_array().is_some_and(|h| !h.is_empty()));
    assert_eq!(hubs["include_tests"], true);
    let bridges = arch(json!({"mode": "bridges", "top_n": 1}));
    assert!(
        bridges["bridge_nodes"]
            .as_array()
            .is_some_and(|b| b.len() <= 1)
    );
    let gaps = arch(json!({"mode": "knowledge_gaps"}));
    assert!(gaps["gaps"]["_meta"]["thresholds"].is_object());
    assert_eq!(gaps["_hints"]["next_steps"][0]["tool"], "refactor_tool");
    let surprising = arch(json!({"mode": "surprising_connections", "artifact_scope": "all"}));
    assert!(surprising["surprising_connections"].is_array());
    let overview = arch(json!({}));
    assert_eq!(overview["called_subtool"], "get_architecture_overview_func");
    assert_eq!(overview["architecture_health"]["status"], "ok");
    assert!(overview["stable_component_policy"]["counts"].is_object());
    let communities = arch(json!({"mode": "communities", "detail_level": "standard"}));
    assert_eq!(communities["called_subtool"], "list_communities_func");
    assert!(communities["answerability"].is_object());
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
        json!({"mode": "adp_violations", "dependency_profile": "bogus"}),
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
    assert_eq!(
        dead["_hints"]["next_steps"].as_array().map(Vec::is_empty),
        Some(false)
    );
    let suggest = answer(&context, "refactor_tool", json!({}));
    assert!(
        suggest["suggestions"]
            .as_array()
            .is_some_and(|s| s.iter().any(|x| x["type"] == "remove"))
    );
    assert!(suggest["work_packs"].is_array());
    let preview = answer(
        &context,
        "refactor_tool",
        json!({"mode": "rename", "old_name": "orphan", "new_name": "kept"}),
    );
    let id = preview["refactor_id"].as_str().expect("id").to_string();
    assert_eq!(preview["edits"][0]["source"], "definition");
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
    let repo = Repo::new("ensure-nogit", false);
    repo.build();
    // The graph alone marks the project root.
    std::fs::remove_dir_all(repo.0.join(".git")).expect("unmark");
    assert!(declines(&repo.context(), "ensure_graph_tool", json!({})));
}

#[test]
fn large_functions_rank_by_line_count() {
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
        json!({"query": "something that assists", "limit": 3}),
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
