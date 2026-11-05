use dagayn_tools::Context;
use serde_json::json;

use crate::common::{Repo, answer, declines};

#[test]
fn flow_tool_answers_entry_points_and_declines_stored_flow_modes() {
    // dagayn: tests crates/dagayn-tools/src/flow.rs::flow
    let repo = Repo::new("flows", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let map = answer(&context, "flow_tool", json!({}));
    assert_eq!(map["status"], "ok");
    assert!(map.get("called_subtool").is_none());
    assert!(map.get("flows").is_none());
    assert_eq!(
        answer(&context, "flow_tool", json!({"mode": "entry_points"})),
        map
    );
    for arguments in [
        json!({"mode": "list"}),
        json!({"mode": "get", "flow_id": 1}),
        json!({"sort_by": "criticality"}),
        json!({"flow_name": "main"}),
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
fn flow_tool_walks_past_called_handlers_and_skips_test_only_code() {
    // dagayn: tests crates/dagayn-tools/src/flow.rs::flow
    // A `handle_*` helper that production code calls is a step, not the
    // way in; a function only its test calls is not a way in either.
    let repo = Repo::new("entry-handlers", true);
    repo.write(
        "worker.py",
        "def scheduled():\n    return handle_inbox_follow()\n\n\n\
         def handle_inbox_follow():\n    return make_id()\n\n\n\
         def make_id():\n    return 1\n\n\n\
         def lists_page():\n    return make_id()\n",
    );
    repo.write(
        "test_worker.py",
        "from worker import lists_page\n\n\ndef test_lists_page():\n    lists_page()\n",
    );
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let found = answer(
        &context,
        "flow_tool",
        json!({"mode": "entry_points", "target": "worker.py::make_id"}),
    );
    let entries: Vec<(&str, &str)> = found["entry_points"]
        .as_array()
        .expect("entry points")
        .iter()
        .map(|e| {
            (
                e["entry_point"].as_str().unwrap_or_default(),
                e["kind"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        entries,
        vec![("worker.py::scheduled", "uncalled")],
        "{found}"
    );
    assert_eq!(
        found["entry_points"][0]["chain"],
        json!([
            "worker.py::scheduled",
            "worker.py::handle_inbox_follow",
            "worker.py::make_id"
        ])
    );
}
