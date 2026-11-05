use serde_json::json;

use crate::common::{Repo, answer, declines};

#[test]
fn ensure_graph_answers_only_when_there_is_nothing_to_prepare() {
    // dagayn: tests crates/dagayn-tools/src/ensure.rs::ensure_graph
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
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
    for gone in ["_hints", "next_tool_suggestions"] {
        assert!(ready.get(gone).is_none(), "{gone}");
    }

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
