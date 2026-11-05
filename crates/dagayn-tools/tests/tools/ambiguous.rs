//! `query_graph_tool` on a name that matches more than one node: at most
//! five are each answered as a direct query would answer them (issue #307).

use dagayn_tools::Context;
use serde_json::{Value, json};

use crate::common::{Repo, answer};

/// A repository where `helper` names three functions: `app.py::helper`
/// (called by `main`), `lib.py::helper` (called by 25 functions and a test, more than
/// an answer shows), and `idle.py::helper` (called by nothing).
fn three_helpers() -> Repo {
    let repo = Repo::new("ambiguous-answers", false);
    let mut lib = String::from("def helper():\n    return leaf()\n\n\ndef leaf():\n    pass\n");
    for index in 0..25 {
        lib.push_str(&format!(
            "\n\ndef caller_{index:02}():\n    return helper()\n"
        ));
    }
    repo.write("lib.py", &lib);
    repo.write("idle.py", "def helper():\n    pass\n");
    repo.write(
        "test_lib.py",
        "from lib import helper\n\n\ndef test_helper():\n    helper()\n",
    );
    repo.build();
    repo
}

/// Asserts that each of `reply`'s answers is what a direct
/// `detail_level="minimal"` query with that candidate's qualified name
/// answers: the same rows in the same order (as many as the answer shows),
/// the same count, and complete exactly when the direct answer is complete
/// and shows them all.
fn assert_answers_are_direct(context: &Context, call: &Value, reply: &Value) {
    let answers = reply["answers"].as_array().expect("answers");
    let candidates = reply["candidates"].as_array().expect("candidates");
    assert_eq!(answers.len(), candidates.len(), "{reply}");
    for (item, candidate) in answers.iter().zip(candidates) {
        assert_eq!(item["target"], candidate["qualified_name"]);
        let mut args = call.clone();
        args["target"] = item["target"].clone();
        args["detail_level"] = json!("minimal");
        let direct = answer(context, "query_graph_tool", args);
        assert_eq!(direct["status"], "ok", "{direct}");
        let rows = item["results"].as_array().expect("rows");
        let direct_rows = direct["results"].as_array().expect("direct rows");
        assert_eq!(
            rows[..],
            direct_rows[..rows.len()],
            "{call} {}",
            item["target"]
        );
        assert_eq!(item["result_count"], direct["result_count"], "{call}");
        let complete = direct["results_complete"] == json!(true)
            && direct["result_count"].as_u64() == Some(rows.len() as u64);
        assert_eq!(item["results_complete"], json!(complete), "{call} {item}");
    }
}

#[test]
fn each_candidate_is_answered_as_its_direct_query() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = three_helpers();
    let context = repo.context();
    for call in [
        json!({"pattern": "callers_of", "target": "helper"}),
        json!({"pattern": "callers_of", "target": "helper", "depth": 2}),
        json!({"pattern": "callees_of", "target": "helper"}),
        json!({"pattern": "tests_for", "target": "helper"}),
        json!({"pattern": "children_of", "target": "helper", "detail_level": "full"}),
    ] {
        let reply = answer(&context, "query_graph_tool", call.clone());
        assert_eq!(reply["status"], "ambiguous", "{call}");
        // Nothing reads as the answer for the name alone.
        assert_eq!(reply["result_count"], 0);
        assert_eq!(reply["results"], json!([]));
        assert_eq!(reply["candidates_truncated"], false);
        assert_eq!(
            reply["summary"]
                .as_str()
                .expect("summary")
                .split(';')
                .next(),
            Some("'helper' matches 3 nodes"),
            "{call}"
        );
        assert_answers_are_direct(&context, &call, &reply);
    }

    // `lib.py::helper` has more callers than an answer shows: its answer
    // says so, and `next` makes the full query.
    let call = json!({"pattern": "callers_of", "target": "helper", "depth": 2});
    let reply = answer(&context, "query_graph_tool", call.clone());
    let answers = reply["answers"].as_array().expect("answers");
    let lib = answers
        .iter()
        .find(|item| item["target"] == "lib.py::helper")
        .expect("lib.py::helper answered");
    assert_eq!(lib["results_complete"], false, "{lib}");
    // 25 callers and the test.
    assert_eq!(lib["result_count"], 26);
    assert_eq!(lib["results"].as_array().map(Vec::len), Some(20));
    let app = answers
        .iter()
        .find(|item| item["target"] == "app.py::helper")
        .expect("app.py::helper answered");
    assert_eq!(app["results_complete"], true, "{app}");
    assert_eq!(
        reply["next"],
        json!([{
            "tool": "query_graph_tool",
            "args": {"pattern": "callers_of", "target": "lib.py::helper", "depth": 2},
            "why": reply["next"][0]["why"],
        }]),
        "{}",
        reply["next"]
    );
    let full = answer(
        &context,
        "query_graph_tool",
        reply["next"][0]["args"].clone(),
    );
    assert_eq!(full["status"], "ok");
    assert_eq!(full["result_count"], 26);
    assert_eq!(full["results_complete"], true);
}

#[test]
fn every_candidate_answered_needs_no_retry() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = three_helpers();
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "callees_of", "target": "helper"}),
    );
    assert!(
        reply["answers"]
            .as_array()
            .expect("answers")
            .iter()
            .all(|item| item["results_complete"] == true),
        "{reply}"
    );
    assert_eq!(reply["next"], json!([]));
    assert_eq!(
        reply["summary"],
        "'helper' matches 3 nodes; each is answered below. Resend target as one \
         candidate's qualified_name."
    );
}

#[test]
fn the_summary_names_the_member_spelling_for_methods() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("ambiguous-methods", false);
    repo.write(
        "jobs.py",
        "class Build:\n    def run(self):\n        pass\n\n\nclass Deploy:\n    def run(self):\n        pass\n",
    );
    repo.build();
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "run"}),
    );
    assert_eq!(reply["status"], "ambiguous");
    assert_eq!(
        reply["summary"],
        "'run' matches 2 nodes; each is answered below. Resend target as one \
         candidate's qualified_name (path::Type.method)."
    );
}

/// A repository where `dup` names `count` functions, each with callers.
fn dups(count: usize, callers: usize) -> Repo {
    let repo = Repo::new("ambiguous-dups", false);
    for file in 0..count {
        let mut body = String::from("def dup():\n    pass\n");
        for index in 0..callers {
            body.push_str(&format!(
                "\n\ndef use_{file}_{index:02}():\n    return dup()\n"
            ));
        }
        repo.write(&format!("mod_{file}.py"), &body);
    }
    repo.build();
    repo
}

#[test]
fn five_candidates_are_all_listed_and_answered() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = dups(5, 1);
    let call = json!({"pattern": "callers_of", "target": "dup"});
    let reply = answer(&repo.context(), "query_graph_tool", call.clone());
    assert_eq!(reply["status"], "ambiguous");
    assert_eq!(reply["candidates"].as_array().map(Vec::len), Some(5));
    assert_eq!(reply["candidates_truncated"], false, "{reply}");
    assert_answers_are_direct(&repo.context(), &call, &reply);
}

#[test]
fn six_candidates_are_listed_with_their_retries() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = dups(6, 1);
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "dup"}),
    );
    assert_eq!(reply["status"], "ambiguous");
    assert_eq!(reply["candidates"].as_array().map(Vec::len), Some(5));
    assert_eq!(reply["candidates_truncated"], true);
    assert!(reply.get("answers").is_none(), "{reply}");
    assert_eq!(
        reply["summary"],
        "Multiple matches for 'dup'. Resend target as one candidate's qualified_name."
    );
    let next = reply["next"].as_array().expect("next");
    assert_eq!(next.len(), 3);
    for (call, candidate) in next.iter().zip(reply["candidates"].as_array().expect("c")) {
        assert_eq!(call["args"]["target"], candidate["qualified_name"]);
    }
}

#[test]
fn source_of_is_not_answered_per_candidate() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = three_helpers();
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "source_of", "target": "helper"}),
    );
    assert_eq!(reply["status"], "ambiguous");
    assert_eq!(reply["candidates_truncated"], false);
    assert!(reply.get("answers").is_none(), "{reply}");
    assert_eq!(reply["next"].as_array().map(Vec::len), Some(3));
}

#[test]
fn answers_fit_the_minimal_budget() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = dups(5, 25);
    let context = repo.context();
    let call = json!({"pattern": "callers_of", "target": "dup", "detail_level": "minimal"});
    let reply = answer(&context, "query_graph_tool", call.clone());
    // docs/plans/AGENT-WORKFLOW-TARGET.md#decisions-2026-10-07: 8K at minimal.
    let chars = serde_json::to_string(&reply).expect("json").len();
    assert!(chars <= 8000, "{chars} characters");
    assert_answers_are_direct(&context, &call, &reply);
    let answers = reply["answers"].as_array().expect("answers");
    assert!(answers.iter().all(|item| item["results_complete"] == false));
    assert!(
        answers
            .iter()
            .all(|item| !item["results"].as_array().expect("rows").is_empty())
    );
    // Three of the five cut answers fit in `next`, each the full query.
    let next = reply["next"].as_array().expect("next");
    assert_eq!(next.len(), 3);
    for (call, item) in next.iter().zip(answers) {
        assert_eq!(call["args"]["target"], item["target"]);
        assert_eq!(call["args"]["detail_level"], "minimal");
    }
}
