use std::path::Path;

use dagayn_tools::call;
use serde_json::json;

use crate::common::{Repo, answer, declines};

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
    for gone in ["_hints", "next_tool_suggestions"] {
        assert!(stats.get(gone).is_none(), "{gone}");
    }
    let text = call(&repo.context(), "list_graph_stats_tool", &json!({}))
        .expect("answered")
        .text;
    assert!(
        text.starts_with(r#"{"status":"ok","summary":"Graph stats for "#),
        "{text}"
    );
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
