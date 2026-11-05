use dagayn_build::db_path_for_build;
use serde_json::json;

use crate::common::{Repo, answer, declines};

#[test]
fn postprocess_reruns_the_steps_asked_for() {
    // dagayn: tests crates/dagayn-tools/src/postprocess.rs::run_postprocess
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("postprocess", true);
    repo.build();
    let context = repo.context();
    let all = answer(&context, "run_postprocess_tool", json!({}));
    assert_eq!(all["status"], "ok");
    assert_eq!(all["summary"], "Post-processing complete.");
    assert_eq!(all["signatures_updated"], true);
    assert!(all["fts_indexed"].as_i64().expect("fts") > 0, "{all}");
    assert!(all.get("flows_detected").is_none() && all.get("communities_detected").is_some());
    assert_eq!(all["warnings"], json!([]));
    assert_eq!(
        answer(&context, "run_postprocess_tool", json!({})),
        all,
        "a rerun gives the same counts"
    );
    let only = answer(
        &context,
        "run_postprocess_tool",
        json!({"communities": false, "fts": false}),
    );
    assert!(only.get("fts_indexed").is_none() && only.get("communities_detected").is_none());
    assert!(declines(
        &context,
        "run_postprocess_tool",
        json!({"flows": false})
    ));

    // A graph another writer holds is Python's to wait for.
    let db = db_path_for_build(&repo.0).expect("db");
    let _held =
        dagayn_build::GraphLock::acquire(&db, std::time::Duration::from_secs(5)).expect("lock");
    assert!(declines(&context, "run_postprocess_tool", json!({})));
    assert!(dagayn_tools::writes_graph("run_postprocess_tool"));
    assert!(!dagayn_tools::writes_graph("query_graph_tool"));
}
