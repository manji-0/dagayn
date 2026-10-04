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
    // An unknown section's error listing is Python's.
    assert!(declines(
        &context,
        "get_docs_section_tool",
        json!({"section_name": "nope"})
    ));
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

    // Risk analysis, a local embedding refresh, and special case folding are
    // Python's.
    assert!(declines(
        &context,
        "get_minimal_context_tool",
        json!({"changed_files": ["app.py"]})
    ));
    assert!(declines(
        &context,
        "get_minimal_context_tool",
        json!({"task": "Straße"})
    ));
    let mut embedding = repo.context();
    embedding.local_embedding = Some("bge-m3".to_string());
    assert!(declines(&embedding, "get_minimal_context_tool", json!({})));
}

#[test]
fn minimal_context_leaves_an_unbuilt_graph_to_python() {
    let repo = Repo::new("unbuilt", true);
    assert!(declines(
        &repo.context(),
        "get_minimal_context_tool",
        json!({})
    ));
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

    for arguments in [
        json!({"pattern": "callers_of", "target": "app.py::helper", "depth": 0}),
        json!({"pattern": "callees_of", "target": "app.py::helper", "depth": 2}),
        json!({"pattern": "nope", "target": "app.py::helper"}),
    ] {
        assert!(
            declines(&context, "query_graph_tool", arguments.clone()),
            "{arguments}"
        );
    }
}

#[test]
fn a_missing_or_foreign_graph_goes_to_python() {
    let repo = Repo::new("nograph", false);
    let context = repo.context();
    assert!(declines(&context, "list_graph_stats_tool", json!({})));
    assert!(declines(
        &Context::default(),
        "list_graph_stats_tool",
        json!({})
    ));
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
        json!({"mode": "affected_flows", "base": "bad ref"}),
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

    for arguments in [
        json!({"include_source": true}),
        json!({"detail_level": "verbose"}),
        // No HEAD~5 to diff against: Python reports the unresolved base.
        json!({"base": "HEAD~5", "changed_files": ["app.py"]}),
    ] {
        assert!(
            declines(&context, "review_tool", arguments.clone()),
            "{arguments}"
        );
    }
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
    let sap_v = arch(json!({"mode": "sap_violations", "min_distance": 0.0}));
    assert!(sap_v["violations"].is_array());
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
    for arguments in [
        json!({"mode": "community"}),
        json!({"mode": "sdp_violations", "min_delta": 0.00001}),
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
    for arguments in [
        json!({"mode": "rename", "old_name": "", "new_name": "b"}),
        json!({"mode": "rename", "old_name": "a", "new_name": "\u{e9}"}),
    ] {
        assert!(
            declines(&context, "refactor_tool", arguments.clone()),
            "{arguments}"
        );
    }
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
