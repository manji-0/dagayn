use std::process::Command;

use dagayn_tools::{Context, call};
use serde_json::{Value, json};

use crate::common::{Repo, answer};

/// #192: community detection leaves nodes of small clusters without a
/// community after every full build, so they are not a stale graph; and
/// `graph_health` carries no unlabelled arrays.
#[test]
fn unassigned_communities_are_not_stale() {
    // dagayn: tests crates/dagayn-tools/src/answerability.rs::Answerability.compute
    let repo = Repo::new("unassigned", true);
    repo.build();
    let conn = rusqlite::Connection::open(repo.0.join(".dagayn/graph.db")).expect("open");
    conn.execute(
        "UPDATE nodes SET community_id = NULL WHERE qualified_name = 'app.py::helper'",
        [],
    )
    .expect("unassign");
    drop(conn);
    let context = repo.context();
    let reply = answer(
        &context,
        "get_minimal_context_tool",
        json!({"task": "review"}),
    );
    let health = &reply["graph_health"];
    assert_eq!(health["reason_codes"], json!([]), "{health}");
    assert_eq!(health["score"], 1.0, "{health}");
    for gone in ["parse", "answerability"] {
        assert!(health.get(gone).is_none(), "{gone}: {health}");
    }
    let full = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "app.py::helper", "detail_level": "full"}),
    );
    let answerability = &full["answerability"];
    assert_eq!(
        answerability["counts"]["unassigned_nodes"], 1,
        "{answerability}"
    );
    for gone in ["parse", "answerability"] {
        assert!(answerability.get(gone).is_none(), "{gone}: {answerability}");
    }
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
    // No reply, at any level, carries the next-step fields `next` replaced.
    for (tool, arguments) in &cases {
        let mut verbose = arguments.clone();
        verbose["detail_level"] = json!(if *tool == "query_graph_tool" {
            "full"
        } else {
            "verbose"
        });
        for args in [arguments, &verbose] {
            let Some(payload) = call(&context, tool, args) else {
                continue;
            };
            for gone in [
                "_hints",
                "next_action",
                "next_drill_downs",
                "next_tool_suggestions",
                "deprecated_fields",
            ] {
                if payload.value.get(gone).is_some() {
                    failures.push(format!("{tool} {args}: {gone}"));
                }
            }
            if payload.value["exactness"].get("next_action").is_some() {
                failures.push(format!("{tool} {args}: exactness.next_action"));
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

#[test]
fn a_dead_hook_update_reaches_every_reply_as_missingness() {
    // dagayn: tests crates/dagayn-tools/src/answerability.rs::Answerability
    let repo = Repo::new("hook-queue", true);
    repo.build();
    let context = repo.context();
    let codes = |reply: &Value| -> Vec<String> {
        reply["missingness"]
            .as_array()
            .expect("missingness")
            .iter()
            .filter_map(|item| item["reason_code"].as_str().map(str::to_string))
            .collect()
    };
    let args = json!({"pattern": "callers_of", "target": "app.py::helper"});
    assert!(
        !codes(&answer(&context, "query_graph_tool", args.clone()))
            .contains(&"hook_updates_stopped".to_string())
    );

    let conn = rusqlite::Connection::open(repo.0.join(".dagayn/task_queue.db")).expect("queue");
    conn.execute_batch(
        "CREATE TABLE tasks (id INTEGER PRIMARY KEY, kind TEXT NOT NULL, \
         priority INTEGER NOT NULL DEFAULT 0, payload TEXT NOT NULL DEFAULT '{}', \
         state TEXT NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0, \
         created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_error TEXT, \
         not_before REAL NOT NULL DEFAULT 0); \
         INSERT INTO tasks (kind, state, attempts, created_at, updated_at, last_error) \
         VALUES ('update', 'dead', 3, '2999-01-01T00:00:00+00:00', \
         '2999-01-01T00:00:00+00:00', 'budget exceeded: queue update ran past 120s');",
    )
    .expect("plant a dead update");
    drop(conn);

    let reply = answer(&context, "query_graph_tool", args);
    let item = reply["missingness"]
        .as_array()
        .expect("missingness")
        .iter()
        .find(|item| item["reason_code"] == "hook_updates_stopped")
        .unwrap_or_else(|| panic!("{reply}"));
    assert_eq!(item["severity"], "medium");
    let effect = item["claim_effect"].as_str().expect("claim_effect");
    assert!(effect.contains("dagayn update"), "{effect}");
    assert!(effect.contains(".dagayn/hook-skip"), "{effect}");
    let context_reply = answer(&context, "get_minimal_context_tool", json!({}));
    assert!(
        context_reply["graph_health"]["reason_codes"]
            .as_array()
            .expect("reason_codes")
            .contains(&json!("hook_updates_stopped")),
        "{context_reply}"
    );
}
