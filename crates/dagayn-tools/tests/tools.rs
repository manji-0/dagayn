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

    for arguments in [
        json!({"pattern": "callers_of", "target": "app.py::helper", "depth": 2}),
        json!({"pattern": "callers_of", "target": "app.py::helper", "detail_level": "full"}),
        json!({"pattern": "children_of", "target": "app.py"}),
        json!({"pattern": "callers_of", "target": "map"}),
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
