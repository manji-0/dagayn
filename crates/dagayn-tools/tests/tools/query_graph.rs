use std::collections::HashSet;

use dagayn_build::db_path_for_build;
use dagayn_graph::GraphStore;
use dagayn_tools::Context;
use serde_json::{Value, json};

use crate::common::{Repo, answer};

#[test]
fn query_graph_counts_one_row_per_node_everywhere() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
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
fn a_closed_answer_says_it_is_the_whole_answer() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("query-closed", false);
    repo.write(
        "chain.py",
        "from app import helper\n\n\ndef one():\n    helper()\n\n\ndef two():\n    one()\n\n\ndef three():\n    two()\n",
    );
    repo.write("docs/base.md", "# Base\n\nText.\n");
    repo.write(
        "docs/plan.md",
        "# Plan\n\n<!-- constrained-by ./base.md -->\n\nSee [the base](./base.md).\n",
    );
    repo.write("pkg/mod.py", "def f():\n    return 1\n");
    repo.write("pkg/user.py", "from pkg.mod import f\n");
    repo.write(
        "pkg/__init__.py",
        "from importlib import import_module\n\n_LAZY_EXPORTS = {\"f\": (\".mod\", \"f\")}\n\n\n\
         def __getattr__(name):\n    module_name, attr = _LAZY_EXPORTS[name]\n    \
         return getattr(import_module(module_name, __name__), attr)\n",
    );
    repo.build();
    let context = repo.context();
    let summary = |arguments: Value| {
        answer(&context, "query_graph_tool", arguments)["summary"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    let direct = summary(json!({"pattern": "callers_of", "target": "app.py::helper"}));
    assert!(
        direct.ends_with(
            "No direct caller the graph records is missing; calls made through strings, \
             getattr, or registration are not edges"
        ),
        "{direct}"
    );
    let closed = summary(json!({"pattern": "callers_of", "target": "app.py::helper", "depth": 6}));
    assert!(
        closed.contains("within 6 hops. No caller the graph records"),
        "{closed}"
    );
    let importers = summary(json!({"pattern": "importers_of", "target": "app.py"}));
    assert!(
        importers.contains("No direct importer the graph records"),
        "{importers}"
    );

    // A lazy-export table's import is an edge, and its row says so.
    let lazy = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "importers_of", "target": "pkg/mod.py"}),
    );
    let kinds: Vec<&str> = lazy["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(|row| row["import_kind"].as_str())
        .collect();
    assert_eq!(kinds, ["lazy_export"], "{lazy}");
    assert!(
        lazy["summary"]
            .as_str()
            .is_some_and(|summary| summary.ends_with("(import_kind lazy_export)")),
        "{}",
        lazy["summary"]
    );

    // "statement" keeps the import statements and leaves the lazy export out.
    let statements = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "importers_of", "target": "pkg/mod.py", "import_kind": "statement"}),
    );
    let files: Vec<&str> = statements["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(|row| row["file"].as_str())
        .collect();
    assert_eq!(files, ["pkg/user.py"], "{statements}");
    assert!(
        statements["summary"]
            .as_str()
            .is_some_and(|summary| summary.ends_with("only import statements were followed")),
        "{}",
        statements["summary"]
    );

    // At minimal too, and a file that both links and declares a directive
    // names both kinds on its one row.
    let both = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "importers_of", "target": "docs/base.md", "detail_level": "minimal"}),
    );
    let plan: Vec<&Value> = both["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter(|row| row["file"] == "docs/plan.md")
        .collect();
    assert_eq!(plan.len(), 1, "{both}");
    assert_eq!(plan[0]["import_kind"], "directive,link", "{both}");

    // A Markdown answer names what its rows mix, not Python's dynamic imports.
    let docs = summary(json!({"pattern": "importers_of", "target": "docs/base.md"}));
    assert!(
        docs.ends_with(
            "No direct importer the graph records is missing; Markdown links and \
             dependency directives both count"
        ),
        "{docs}"
    );

    // Cut at the depth asked for, or with nothing found: no claim.
    let cut = summary(json!({"pattern": "callers_of", "target": "app.py::helper", "depth": 2}));
    assert!(!cut.contains("is missing"), "{cut}");
    let none = summary(json!({"pattern": "callers_of", "target": "chain.py::three"}));
    assert!(!none.contains("is missing"), "{none}");
}

#[test]
fn imports_of_follows_a_chain_forward_and_says_which_edges_are_directives() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("imports-forward", false);
    repo.write("docs/base.md", "# Base\n\nText.\n");
    repo.write(
        "docs/middle.md",
        "# Middle\n\n<!-- derived-from ./base.md -->\n",
    );
    repo.write(
        "docs/plan.md",
        "# Plan\n\n<!-- constrained-by ./middle.md -->\n\nSee [the notes](./notes.md).\n",
    );
    repo.write("docs/notes.md", "# Notes\n\nText.\n");
    repo.build();
    let context = repo.context();

    let direct = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "imports_of", "target": "docs/plan.md"}),
    );
    let kinds: Vec<(String, String)> = direct["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|row| {
            (
                row["import_target"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                row["import_kind"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(
        kinds.contains(&("docs/middle.md".into(), "directive".into())),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&("docs/notes.md".into(), "link".into())),
        "{kinds:?}"
    );

    let chain = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "imports_of", "target": "docs/plan.md", "depth": 6}),
    );
    assert_eq!(chain["reachability"]["state"], "complete", "{chain}");
    let reached: Vec<(&str, i64)> = chain["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|row| {
            (
                row["import_target"].as_str().unwrap_or_default(),
                row["depth"].as_i64().unwrap_or_default(),
            )
        })
        .collect();
    assert!(reached.contains(&("docs/middle.md", 1)), "{reached:?}");
    assert!(reached.contains(&("docs/base.md", 2)), "{reached:?}");
    assert!(
        reached.iter().any(|(t, _)| *t == "docs/notes.md"),
        "{reached:?}"
    );

    // The filter applies during the walk: directives only, no link.
    let directives = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "imports_of", "target": "docs/plan.md", "depth": 6, "import_kind": "directive"}),
    );
    let targets: Vec<&str> = directives["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(|row| row["import_target"].as_str())
        .collect();
    assert!(targets.contains(&"docs/base.md"), "{targets:?}");
    assert!(!targets.contains(&"docs/notes.md"), "{targets:?}");
    assert!(
        directives["summary"].as_str().is_some_and(
            |summary| summary.ends_with("only Markdown dependency directives were followed")
        ),
        "{}",
        directives["summary"]
    );
    let importers = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "importers_of", "target": "docs/notes.md", "import_kind": "directive"}),
    );
    assert_eq!(importers["result_count"], 0, "{importers}");

    for (arguments, message) in [
        (
            json!({"pattern": "imports_of", "target": "docs/plan.md", "import_kind": "anchor"}),
            "import_kind must be 'directive', 'link', 'lazy_export', 'path', or 'statement', \
             got 'anchor'.",
        ),
        (
            json!({"pattern": "callers_of", "target": "app.py::helper", "import_kind": "link"}),
            "import_kind applies only to ['importers_of', 'imports_of']; 'callers_of' does not \
             take it.",
        ),
    ] {
        let reply = answer(&context, "query_graph_tool", arguments.clone());
        assert_eq!(reply["error"], message, "{arguments}");
    }
}

#[test]
fn an_ambiguous_target_names_its_retries() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
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
    // Two candidates: each is answered whole, so nothing is left to retry.
    let targets: Vec<&Value> = query["answers"]
        .as_array()
        .expect("answers")
        .iter()
        .map(|item| &item["target"])
        .collect();
    assert_eq!(targets.len(), 2, "{}", query["answers"]);
    assert!(targets.contains(&&json!("app.py::helper")), "{targets:?}");
    assert!(targets.contains(&&json!("lib.py::helper")), "{targets:?}");
    assert_eq!(query["next"], json!([]));
    // `source_of` is not answered per candidate: it names the retries.
    let source = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "source_of", "target": "helper"}),
    );
    assert_eq!(source["status"], "ambiguous");
    let targets: Vec<&Value> = source["next"]
        .as_array()
        .expect("next")
        .iter()
        .map(|call| &call["args"]["target"])
        .collect();
    assert_eq!(targets.len(), 2, "{}", source["next"]);
    assert!(targets.contains(&&json!("app.py::helper")), "{targets:?}");
    assert_eq!(source["next"][0]["args"]["pattern"], "source_of");

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

/// The qualified name `query_graph_tool` resolved `target` to, or the
/// reply's status when it did not resolve.
fn resolved(context: &Context, target: &str) -> String {
    let reply = answer(
        context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": target}),
    );
    match reply["status"].as_str() {
        Some("ok") => reply["target"].as_str().unwrap_or("").to_string(),
        status => format!("{status:?} {}", reply["candidates"]),
    }
}

#[test]
fn a_go_method_resolves_in_go_spelling() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    // dagayn: tests crates/dagayn-tools/src/flow.rs::flow
    // #275: a G8 agent wrote `(*File).OpenRaw`; `Writer.Copy` and
    // `readTestFile` mention File and OpenRaw, so a name search finds them too.
    let repo = Repo::new("go-receiver", false);
    repo.write("go.mod", "module example.com/zip\n\ngo 1.21\n");
    repo.write(
        "zip/reader.go",
        "package zip\n\ntype File struct{}\n\nfunc (f *File) OpenRaw() error { return nil }\n",
    );
    repo.write(
        "zip/writer.go",
        "package zip\n\ntype Writer struct{}\n\nfunc (w *Writer) Copy(f *File) error { return f.OpenRaw() }\n",
    );
    repo.write(
        "zip/reader_test.go",
        "package zip\n\nimport \"testing\"\n\nfunc readTestFile(t *testing.T, f *File) { f.OpenRaw() }\n",
    );
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    for target in [
        "File.OpenRaw",
        "(*File).OpenRaw",
        "(File).OpenRaw",
        "*File.OpenRaw",
        "zip/reader.go::(*File).OpenRaw",
        "zip/reader.go::(File).OpenRaw",
        "zip/reader.go::*File.OpenRaw",
    ] {
        assert_eq!(
            resolved(&context, target),
            "zip/reader.go::File.OpenRaw",
            "{target}"
        );
    }
    let callers = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "(*File).OpenRaw"}),
    );
    assert_eq!(callers["original_target"], "(*File).OpenRaw");
    let names: Vec<&str> = callers["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(|row| row["qualified_name"].as_str())
        .collect();
    assert!(names.contains(&"zip/writer.go::Writer.Copy"), "{names:?}");

    let flow = answer(
        &context,
        "flow_tool",
        json!({"mode": "entry_points", "target": "(*File).OpenRaw"}),
    );
    assert_eq!(flow["target"], "zip/reader.go::File.OpenRaw", "{flow}");
}

#[test]
fn a_member_named_by_type_is_ambiguous_only_among_its_namesakes() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("member-targets", false);
    repo.write("go.mod", "module example.com/c\n\ngo 1.21\n");
    for package in ["flate", "gzip"] {
        repo.write(
            &format!("{package}/writer.go"),
            &format!(
                "package {package}\n\ntype Writer struct{{}}\n\nfunc (w *Writer) Reset() {{}}\n\nfunc use(w *Writer) {{ w.Reset() }}\n"
            ),
        );
    }
    repo.write(
        "store.rs",
        "pub struct Store;\n\nimpl Store {\n    pub fn get(&self) {}\n}\n\npub fn read(store: &Store) {\n    store.get();\n}\n",
    );
    repo.write(
        "repo.py",
        "class Repo:\n    def load(self):\n        pass\n\n\ndef run(repo: Repo):\n    repo.load()\n",
    );
    repo.build();
    let context = repo.context();

    let reset = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "(*Writer).Reset"}),
    );
    assert_eq!(reset["status"], "ambiguous");
    let mut candidates: Vec<&str> = reset["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|node| node["qualified_name"].as_str())
        .collect();
    candidates.sort_unstable();
    assert_eq!(
        candidates,
        [
            "flate/writer.go::Writer.Reset",
            "gzip/writer.go::Writer.Reset"
        ]
    );

    for (target, node) in [
        ("Store.get", "store.rs::Store.get"),
        ("Store::get", "store.rs::Store.get"),
        ("crate::Store::get", "store.rs::Store.get"),
        ("Repo.load", "repo.py::Repo.load"),
    ] {
        assert_eq!(resolved(&context, target), node, "{target}");
    }
}

#[test]
fn query_graph_answers_callers_and_callees_of_exact_targets() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
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
            "depth applies only to ['callers_of', 'importers_of', 'imports_of']; 'callees_of' \
             returns direct relationships only.",
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
fn call_rows_say_which_edges_a_scip_index_settled() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("query-scip", false);
    repo.write(
        "app.py",
        "def main():\n    helper()\n    return helper()\n\n\ndef other():\n    return helper()\n\
         \n\ndef helper():\n    pass\n",
    );
    repo.build();
    // What `dagayn build --scip` records on an edge the index settled: every
    // call of `other`, and one of `main`'s two.
    let conn = rusqlite::Connection::open(repo.0.join(".dagayn/graph.db")).expect("open");
    let settled = conn
        .execute(
            "UPDATE edges SET extra = json_set(COALESCE(extra, '{}'), '$.resolved_by', 'scip') \
             WHERE kind = 'CALLS' AND target_qualified = 'app.py::helper' \
               AND (source_qualified = 'app.py::other' \
                    OR (source_qualified = 'app.py::main' AND line = 2))",
            [],
        )
        .expect("settle");
    assert_eq!(settled, 2);
    drop(conn);
    let context = repo.context();

    for detail in ["standard", "minimal"] {
        let callers = answer(
            &context,
            "query_graph_tool",
            json!({"pattern": "callers_of", "target": "app.py::helper", "detail_level": detail}),
        );
        let row = |name: &str| {
            callers["results"]
                .as_array()
                .expect("rows")
                .iter()
                .find(|row| row["qualified_name"] == name)
                .cloned()
                .unwrap_or_else(|| panic!("{detail}: no row for {name}: {callers}"))
        };
        assert_eq!(row("app.py::other")["resolved_by"], "scip", "{detail}");
        // One of `main`'s calls was inferred: the merged row does not claim
        // the index settled both.
        assert!(row("app.py::main").get("resolved_by").is_none(), "{detail}");
    }
    let callees = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callees_of", "target": "app.py::other"}),
    );
    assert_eq!(callees["results"][0]["resolved_by"], "scip");
    let full = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "app.py::helper", "detail_level": "full"}),
    );
    let edges = full["edges"].as_array().expect("edges");
    let by_line = |line: i64| {
        edges
            .iter()
            .find(|edge| edge["line"] == line)
            .unwrap_or_else(|| panic!("no edge on line {line}: {full}"))
    };
    assert_eq!(by_line(2)["resolved_by"], "scip");
    assert!(by_line(3).get("resolved_by").is_none());
    assert_eq!(by_line(7)["resolved_by"], "scip");
}

#[test]
fn query_graph_answers_trimmed_unread_and_dotted_targets() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
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

/// #195: a node the graph has, with no edge of the kind asked for, is not
/// a node the graph lacks.
#[test]
fn a_node_with_no_edges_is_not_reported_missing() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("no-edges", false);
    repo.build();
    let context = repo.context();
    for (pattern, target, says) in [
        (
            "callers_of",
            "test_app.py::test_main",
            "nothing in it calls",
        ),
        ("callees_of", "app.py::helper", "calls nothing"),
        ("importers_of", "test_app.py", "nothing in it imports"),
        ("tests_for", "test_app.py::test_main", "no test"),
    ] {
        for detail in ["minimal", "standard"] {
            let reply = answer(
                &context,
                "query_graph_tool",
                json!({"pattern": pattern, "target": target, "detail_level": detail}),
            );
            assert_eq!(reply["exact_match_count"], 1, "{pattern} {reply}");
            assert_eq!(reply["result_count"], 0, "{pattern} {reply}");
            assert_eq!(
                reply["zero_result_reason"], "node_has_no_matching_edges",
                "{pattern} {reply}"
            );
            let summary = reply["summary"].as_str().unwrap_or_default();
            assert!(summary.contains(says), "{pattern}: {summary}");
            assert!(!reply.to_string().contains("refresh the graph"), "{reply}");
        }
    }
    let missing = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "no_such_symbol"}),
    );
    assert_eq!(missing["status"], "not_found");
    assert_eq!(missing["zero_result_reason"], "target_not_found_in_graph");
}

#[test]
fn a_cut_minimal_list_says_so_and_offers_the_standard_call() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("query-cut", false);
    repo.write("core.py", "def f():\n    return 1\n");
    for index in 0..150 {
        repo.write(
            &format!("users/user_module_with_a_long_name_{index:03}.py"),
            "from core import f\n",
        );
    }
    repo.build();
    let context = repo.context();
    let cut = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "importers_of", "target": "core.py", "depth": 6, "detail_level": "minimal"}),
    );
    assert_eq!(cut["results_complete"], false, "{cut}");
    let summary = cut["summary"].as_str().unwrap_or_default();
    assert!(
        summary.contains("of 150 rows; detail_level=\"standard\" lists more in one call"),
        "{summary}"
    );
    assert!(
        summary.ends_with("all 150 lie in 150 files (first 60 in files)"),
        "{summary}"
    );
    assert_eq!(cut["files"].as_array().map(Vec::len), Some(60), "{cut}");
    assert_eq!(
        cut["next"][0]["args"],
        json!({"pattern": "importers_of", "target": "core.py", "detail_level": "standard", "depth": 6}),
        "{}",
        cut["next"]
    );
}

/// A list cut at `standard` names the call for the rows after the cut, and
/// following it lists every row once; a walk stopped at its row limit says
/// so (G8 defect 4: 63 of 505 rows, and `next` offered only `source_of`).
#[test]
fn a_cut_list_pages_to_its_last_row() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("query-pages", false);
    let callers: String = (0..600)
        .map(|i| format!("def caller_with_a_long_descriptive_name_{i:03}():\n    return g()\n\n\n"))
        .collect();
    repo.write(
        "mod.py",
        &format!("def f():\n    return 1\n\n\ndef g():\n    return f()\n\n\n{callers}"),
    );
    repo.build();
    let context = repo.context();
    let first = json!({"pattern": "callers_of", "target": "mod.py::f", "depth": 6});
    let mut reply = answer(&context, "query_graph_tool", first.clone());
    assert_eq!(reply["results_complete"], false, "{reply}");
    let summary = reply["summary"].as_str().unwrap_or_default();
    assert!(
        summary.contains("The walk stopped at 500 rows"),
        "{summary}"
    );
    let total = reply["_truncation"]["results"]["total"].as_u64().unwrap();
    assert_eq!(total, 501, "{reply}");
    let mut names: Vec<String> = Vec::new();
    let mut pages = 0;
    loop {
        pages += 1;
        let rows = reply["results"].as_array().expect("results");
        names.extend(
            rows.iter()
                .filter_map(|row| row["qualified_name"].as_str().map(str::to_string)),
        );
        let page = reply["next"]
            .as_array()
            .expect("next")
            .iter()
            .find(|call| call["args"].get("offset").is_some())
            .cloned();
        let Some(page) = page else { break };
        let mut expected = first.clone();
        expected["offset"] = json!(names.len());
        assert_eq!(page["args"], expected, "{}", reply["next"]);
        assert!(pages < 20, "{reply}");
        reply = answer(&context, "query_graph_tool", page["args"].clone());
    }
    let distinct: HashSet<&String> = names.iter().collect();
    assert_eq!(names.len(), 501, "pages {pages}");
    assert_eq!(distinct.len(), 501);
    let summary = reply["summary"].as_str().unwrap_or_default();
    assert!(summary.contains("Showing rows 409-501 of 501"), "{summary}");
    assert_eq!(reply["results_complete"], false, "{reply}");
}

/// `tests_for` with no direct test does not say that no test reaches the
/// node when tests reach it through its callers; `next` lists them (G8
/// defect 4: T5 said "no test in it reaches it" with `next: []`).
#[test]
fn tests_for_without_a_direct_test_names_the_callers_walk() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("go-tests-through-callers", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "mock/mock.go",
        "package mock\n\nfunc diff() int {\n\treturn 1\n}\n\n\
         func Called() int {\n\treturn diff()\n}\n\nfunc unused() int {\n\treturn 2\n}\n",
    );
    repo.write(
        "mock/mock_test.go",
        "package mock\n\nimport \"testing\"\n\nfunc TestCalled(t *testing.T) {\n\tCalled()\n}\n",
    );
    repo.build();
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "mock/mock.go::diff"}),
    );
    assert_eq!(reply["result_count"], 0, "{reply}");
    let summary = reply["summary"].as_str().unwrap_or_default();
    assert!(!summary.contains("no test in it reaches it"), "{summary}");
    assert!(
        summary.contains("mock/mock_test.go::TestCalled"),
        "{summary}"
    );
    assert_eq!(
        reply["next"][0]["args"],
        json!({"pattern": "callers_of", "target": "mock/mock.go::diff", "depth": 6}),
        "{}",
        reply["next"]
    );
    let lone = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "mock/mock.go::unused"}),
    );
    let summary = lone["summary"].as_str().unwrap_or_default();
    assert!(summary.contains("no test in it reaches it"), "{summary}");
    assert_eq!(lone["next"], json!([]), "{lone}");
}

#[test]
fn rust_calls_through_a_field_and_paths_in_code_reach_other_files() {
    // dagayn: tests crates/dagayn-graph/src/postprocess/returned.rs::resolve_field_receivers
    let repo = Repo::new("rust-fields", false);
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fields\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "src/lib.rs",
        "pub mod model;\npub mod types;\npub mod user;\npub mod uses;\n",
    );
    repo.write(
        "src/types.rs",
        "pub struct Path(String);\n\nimpl Path {\n    pub fn as_str(&self) -> &str {\n        &self.0\n    }\n}\n\npub struct Other;\n\nimpl Other {\n    pub fn as_str(&self) -> &str {\n        \"o\"\n    }\n}\n",
    );
    repo.write(
        "src/model.rs",
        "use crate::types::Path;\n\npub struct Node {\n    pub path: Path,\n}\n",
    );
    repo.write(
        "src/user.rs",
        "use crate::model::Node;\n\npub fn name(node: &Node) -> usize {\n    node.path.as_str().len()\n}\n\npub fn shown(node: &Node) -> String {\n    format!(\"{}\", node.path.as_str())\n}\n",
    );
    repo.write(
        "src/uses.rs",
        "pub fn make() -> crate::types::Other {\n    crate::types::Other\n}\n",
    );
    repo.build();
    let context = repo.context();

    // `node.path.as_str()`: `Node` is declared in model.rs, its `path` a
    // `Path` of types.rs, in a call and in a macro's arguments.
    let callers = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "src/types.rs::Path.as_str"}),
    );
    let names: Vec<&str> = callers["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(|row| row["qualified_name"].as_str())
        .collect();
    assert_eq!(
        names,
        ["src/user.rs::name", "src/user.rs::shown"],
        "{callers}"
    );
    assert!(
        callers["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("calls on closure parameters, loop variables")),
        "{}",
        callers["summary"]
    );
    let other = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "src/types.rs::Other.as_str"}),
    );
    assert_eq!(other["result_count"], 0, "{other}");

    // `crate::types::Other` in code, no `use`: a dependency of kind `path`.
    let importers = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "importers_of", "target": "src/types.rs"}),
    );
    let rows: Vec<(&str, Option<&str>)> = importers["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|row| {
            (
                row["file"].as_str().unwrap_or(""),
                row["import_kind"].as_str(),
            )
        })
        .collect();
    assert!(rows.contains(&("src/uses.rs", Some("path"))), "{importers}");
    assert!(rows.contains(&("src/model.rs", None)), "{importers}");
    let statements = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "importers_of", "target": "src/types.rs", "import_kind": "statement"}),
    );
    assert!(
        statements["results"]
            .as_array()
            .expect("results")
            .iter()
            .all(|row| row["file"] != "src/uses.rs"),
        "{statements}"
    );
}

#[test]
fn typescript_untyped_parameters_are_typed_by_the_members_they_use() {
    // dagayn: tests crates/dagayn-graph/src/postprocess/returned.rs::resolve_shape_receivers
    let repo = Repo::new("ts-shape", false);
    repo.write(
        "src/req.ts",
        "export class Req {\n  text(): string {\n    return ''\n  }\n}\n",
    );
    repo.write(
        "src/ctx.ts",
        "import { Req } from './req'\n\nexport class Context {\n  get req(): Req {\n    return new Req()\n  }\n  get(key: string): string {\n    return key\n  }\n  render(): string {\n    return ''\n  }\n}\n",
    );
    repo.write(
        "src/other.ts",
        "export class Resp {\n  text(): string {\n    return ''\n  }\n  get(key: string): string {\n    return key\n  }\n}\n",
    );
    repo.write(
        "src/app.ts",
        "export const body = (c) => c.req.text()\n\nexport const raw = (req) => req.text()\n\nexport const header = (res) => res.headers.get('x')\n\nexport const both = (c) => {\n  c.render()\n  return c.get('k')\n}\n",
    );
    repo.write(
        "test/app.ts",
        "export const outside = (c) => c.req.text()\n",
    );
    repo.build();
    let context = repo.context();
    let callers = |target: &str, prefix: Option<&str>| -> Vec<String> {
        let mut arguments = json!({"pattern": "callers_of", "target": target});
        if let Some(prefix) = prefix {
            arguments["path_prefix"] = json!(prefix);
        }
        let reply = answer(&context, "query_graph_tool", arguments);
        let mut names: Vec<String> = reply["results"]
            .as_array()
            .expect("results")
            .iter()
            .filter_map(|row| row["qualified_name"].as_str().map(str::to_string))
            .collect();
        names.sort();
        names
    };
    // `c.req.text()`: only `Context` has `req`, and `req` returns a `Req`.
    assert_eq!(
        callers("src/req.ts::Req.text", None),
        ["src/app.ts::body", "test/app.ts::outside"]
    );
    // `path_prefix` keeps the rows under it.
    assert_eq!(
        callers("src/req.ts::Req.text", Some("src/")),
        ["src/app.ts::body"]
    );
    // `req.text()` alone, `res.headers.get(..)`: names the standard objects
    // have too, so neither binds; `c.render()` with `c.get(..)` is a
    // `Context`.
    assert!(callers("src/other.ts::Resp.text", None).is_empty());
    assert_eq!(
        callers("src/ctx.ts::Context.get", None),
        ["src/app.ts::both"]
    );
    assert!(callers("src/other.ts::Resp.get", None).is_empty());
}

/// A call or base class named only by its bare name never reaches a node
/// of another language family, even when that node is the only one with
/// the name: Rust's `tally()` is not TypeScript's, and Python's `Base` is
/// not a Rust struct.
#[test]
fn bare_names_do_not_reach_another_language() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("bare-family", false);
    repo.write(
        "rust/src/run.rs",
        "pub fn run() -> i64 {\n    tally()\n}\n\npub struct Base;\n",
    );
    repo.write(
        "web/count.ts",
        "function tally(): number {\n  return 1;\n}\n",
    );
    repo.write("py/handler.py", "class Handler(Base):\n    pass\n");
    repo.build();
    let context = repo.context();
    let sources = |pattern: &str, target: &str| -> Vec<String> {
        let reply = answer(
            &context,
            "query_graph_tool",
            json!({"pattern": pattern, "target": target}),
        );
        reply["results"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row["qualified_name"].as_str().map(str::to_string))
            .collect()
    };
    let callers = sources("callers_of", "web/count.ts::tally");
    assert!(callers.is_empty(), "{callers:?}");
    let inheritors = sources("inheritors_of", "rust/src/run.rs::Base");
    assert!(inheritors.is_empty(), "{inheritors:?}");
    // The graph's own view; the repository text check after it still keeps
    // `tally` out of the report, since the word appears in `run.rs`.
    let store = GraphStore::open(db_path_for_build(&repo.0).expect("db")).expect("store");
    let candidates: Value = serde_json::from_str(
        &dagayn_tools::graph_dead_code_candidates_json(&store, None, None).expect("candidates"),
    )
    .expect("json");
    let names: Vec<&str> = candidates
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["qualified_name"].as_str())
        .collect();
    assert!(names.contains(&"web/count.ts::tally"), "{names:?}");
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
fn go_packages_are_imported_through_their_files() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("go-package-imports", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "mathx/math.go",
        "package mathx\n\n// Add adds.\nfunc Add(a, b int) int {\n\treturn a + b\n}\n",
    );
    repo.write(
        "mathx/twice.go",
        "package mathx\n\n// Twice doubles.\nfunc Twice(a int) int {\n\treturn Add(a, a)\n}\n",
    );
    repo.write(
        "mathx/math_test.go",
        "package mathx_test\n\nimport (\n\t\"testing\"\n\n\t\"example.com/demo/mathx\"\n)\n\n\
         func TestTwice(t *testing.T) {\n\tif mathx.Twice(2) != 4 {\n\t\tt.Fatal(\"Twice\")\n\t}\n}\n",
    );
    repo.write(
        "cmd/demo/main.go",
        "package main\n\nimport \"example.com/demo/mathx\"\n\nfunc main() {\n\t_ = mathx.Twice(21)\n}\n",
    );
    repo.build();
    let context = repo.context();
    let query = |pattern: &str, target: &str| {
        answer(
            &context,
            "query_graph_tool",
            json!({"pattern": pattern, "target": target}),
        )
    };
    let importers = |reply: &Value| -> Vec<String> {
        let mut files: Vec<String> = reply["results"]
            .as_array()
            .expect("results")
            .iter()
            .filter_map(|row| row["file"].as_str().map(str::to_string))
            .collect();
        files.sort();
        files
    };
    // The package's directory and each of its files: one row per importer,
    // the external test package among them.
    for target in ["mathx", "mathx/", "mathx/twice.go"] {
        let reply = query("importers_of", target);
        assert_eq!(
            importers(&reply),
            ["cmd/demo/main.go", "mathx/math_test.go"],
            "{target}: {reply}"
        );
    }
    // A call through the import binds, so the external test reaches Twice.
    let tests = query("tests_for", "mathx/twice.go::Twice");
    assert_eq!(
        tests["test_reach"]["nearest_test"], "mathx/math_test.go::TestTwice",
        "{tests}"
    );
}

/// A Go method stored in a field is called through the field, which is no
/// edge: `callers_of` names the place that stores it instead of an empty
/// answer, and the test that runs that place reaches the method.
#[test]
fn go_callers_of_a_method_used_as_a_value_name_where_it_is_stored() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("go-method-values", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "flate/deflate.go",
        "package flate\n\ntype compressor struct{ step func(*compressor) }\n\n\
         func (d *compressor) storeFast() {}\n\n\
         func (d *compressor) init() {\n\td.step = (*compressor).storeFast\n}\n\n\
         func (d *compressor) write() {\n\td.step(d)\n\tflush(d)\n}\n\n\
         func (d *compressor) reset() {\n\td.step = (*compressor).storeFast\n\td.storeFast()\n}\n\n\
         func flush(d *compressor) {}\n",
    );
    repo.write(
        "flate/deflate_test.go",
        "package flate\n\nimport \"testing\"\n\n\
         func TestInit(t *testing.T) {\n\td := &compressor{}\n\td.init()\n}\n",
    );
    repo.write(
        "README.md",
        "# Flate\n\nSee [flush](flate/deflate.go#flush).\n",
    );
    repo.build();
    let context = repo.context();
    let query = |args: Value| answer(&context, "query_graph_tool", args);
    let store_fast = "flate/deflate.go::compressor.storeFast";
    let relations = |reply: &Value| -> Vec<(String, Value)> {
        let mut rows: Vec<(String, Value)> = reply["results"]
            .as_array()
            .expect("results")
            .iter()
            .map(|row| {
                (
                    row["qualified_name"].as_str().unwrap_or("").to_string(),
                    row["relation"].clone(),
                )
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    };
    let expected = vec![
        (
            "flate/deflate.go::compressor.init".to_string(),
            json!("references"),
        ),
        (
            "flate/deflate.go::compressor.reset".to_string(),
            json!("calls,references"),
        ),
    ];
    let callers = query(json!({"pattern": "callers_of", "target": store_fast}));
    assert_eq!(relations(&callers), expected, "{callers}");
    let summary = callers["summary"].as_str().expect("summary");
    assert!(summary.contains("2 use it as a value"), "{summary}");
    assert!(!summary.contains("is missing"), "{summary}");
    let minimal =
        query(json!({"pattern": "callers_of", "target": store_fast, "detail_level": "minimal"}));
    assert_eq!(relations(&minimal), expected, "{minimal}");
    // The test that calls `init` does not call `storeFast`.
    let deep = query(json!({"pattern": "callers_of", "target": store_fast, "depth": 2}));
    assert_eq!(relations(&deep), expected, "{deep}");
    // A Markdown link to a function is no use of it as a value.
    let flush = query(json!({"pattern": "callers_of", "target": "flate/deflate.go::flush"}));
    assert_eq!(
        relations(&flush),
        [(
            "flate/deflate.go::compressor.write".to_string(),
            Value::Null
        )],
        "{flush}"
    );
    let tests = query(json!({"pattern": "tests_for", "target": store_fast}));
    assert_eq!(
        tests["test_reach"]["nearest_test"], "flate/deflate_test.go::TestInit",
        "{tests}"
    );
}

/// A Go function stored in a struct field (`New: gzkp.NewWriter`) is called
/// through the field, which is no edge: `callers_of` names the place that
/// stores it, bound through the import path to that package's function.
#[test]
fn go_callers_of_a_function_used_as_a_value_name_where_it_is_stored() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("go-function-values", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    for package in ["gzkp", "zstdkp"] {
        repo.write(
            &format!("writer/{package}/{package}.go"),
            &format!("package {package}\n\nfunc NewWriter(level int) int {{ return level }}\n"),
        );
    }
    repo.write(
        "writer/gzkp/gzkp_test.go",
        "package gzkp\n\nimport \"testing\"\n\n\
         func TestNewWriter(t *testing.T) {\n\tNewWriter(1)\n}\n",
    );
    repo.write(
        "compress.go",
        "package demo\n\nimport (\n\t\"example.com/demo/writer/gzkp\"\n\t\"example.com/demo/writer/zstdkp\"\n)\n\n\
         type factory struct{ New func(int) int }\n\n\
         type config struct{ gzip, zstd factory }\n\n\
         func NewWrapper() config {\n\treturn config{\n\t\tgzip: factory{New: gzkp.NewWriter},\n\t\tzstd: factory{New: zstdkp.NewWriter},\n\t}\n}\n\n\
         func (c config) write() int {\n\treturn c.gzip.New(1)\n}\n\n\
         func Handler() int {\n\treturn NewWrapper().write()\n}\n",
    );
    repo.build();
    let context = repo.context();
    let query = |args: Value| answer(&context, "query_graph_tool", args);
    let relations = |reply: &Value| -> Vec<(String, Value)> {
        let mut rows: Vec<(String, Value)> = reply["results"]
            .as_array()
            .expect("results")
            .iter()
            .map(|row| {
                (
                    row["qualified_name"].as_str().unwrap_or("").to_string(),
                    row["relation"].clone(),
                )
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    };
    let gzkp = "writer/gzkp/gzkp.go::NewWriter";
    let expected = vec![
        ("compress.go::NewWrapper".to_string(), json!("references")),
        (
            "writer/gzkp/gzkp_test.go::TestNewWriter".to_string(),
            Value::Null,
        ),
    ];
    let callers = query(json!({"pattern": "callers_of", "target": gzkp}));
    assert_eq!(relations(&callers), expected, "{callers}");
    let summary = callers["summary"].as_str().expect("summary");
    assert!(summary.contains("1 use it as a value"), "{summary}");
    // `Handler` calls the place that stores the function, not the function.
    let deep = query(json!({"pattern": "callers_of", "target": gzkp, "depth": 2}));
    assert_eq!(relations(&deep), expected, "{deep}");
    let zstd = query(json!({
        "pattern": "callers_of",
        "target": "writer/zstdkp/zstdkp.go::NewWriter",
    }));
    assert_eq!(
        relations(&zstd),
        [("compress.go::NewWrapper".to_string(), json!("references"))],
        "{zstd}"
    );
}

/// A closed Go `callers_of` says what it checked: with every call of the
/// method's name bound, it claims the whole package (tests included) and
/// drops the run-time caveat; a call left unbound, here one through an
/// interface, keeps the caveat and names where it is.
#[test]
fn go_callers_of_says_whether_a_call_of_its_name_is_left_unbound() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("go-unbound-calls", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "gzip/gzip.go",
        "package gzip\n\ntype Writer struct{ level int }\n\n\
         func (z *Writer) init(level int) { z.level = level }\n\n\
         func (z *Writer) Reset() { z.init(1) }\n",
    );
    repo.write(
        "gzip/gzip_test.go",
        "package gzip\n\nimport \"testing\"\n\n\
         func TestInit(t *testing.T) {\n\tw := &Writer{}\n\tw.init(2)\n}\n",
    );
    // Another package's `init` calls on values of no known type cannot
    // reach the unexported `gzip.Writer.init`.
    repo.write(
        "zstd/dec.go",
        "package zstd\n\ntype initer interface{ init(int) }\n\n\
         func start(items []initer) {\n\tfor _, item := range items {\n\t\titem.init(3)\n\t}\n}\n",
    );
    repo.write(
        "suite/suite.go",
        "package suite\n\ntype TestingSuite interface{ SetT(t int) }\n\n\
         type Suite struct{ t int }\n\n\
         func (s *Suite) SetT(t int) { s.t = t }\n\n\
         func (s *Suite) Setup() { s.SetT(0) }\n\n\
         func Run(s TestingSuite) { s.SetT(1) }\n",
    );
    repo.build();
    let context = repo.context();
    let callers = |target: &str| {
        answer(
            &context,
            "query_graph_tool",
            json!({"pattern": "callers_of", "target": target}),
        )
    };
    let init = callers("gzip/gzip.go::Writer.init");
    let summary = init["summary"].as_str().expect("summary");
    assert!(
        summary.contains(
            "No direct caller the graph records is missing; every call named init in \
             package gzip, _test.go files included, is bound to a definition, a method call \
             by its receiver's static type"
        ),
        "{summary}"
    );
    assert!(!summary.contains("run time"), "{summary}");
    let set_t = callers("suite/suite.go::Suite.SetT");
    let summary = set_t["summary"].as_str().expect("summary");
    assert!(
        summary.contains(
            "Every other call named SetT in the repository, _test.go files included, is bound \
             to a definition; the 1 that goes to no definition is at suite/suite.go:11 in Run, \
             through interface TestingSuite (suite/suite.go:3); a call through an interface \
             reaches Suite.SetT only where the value's dynamic type is Suite or embeds it"
        ),
        "{summary}"
    );
    assert!(!summary.contains("is missing;"), "{summary}");
    assert!(!summary.contains("read "), "{summary}");
    assert_eq!(
        set_t["next"][0]["args"],
        json!({"pattern": "source_of", "target": "suite/suite.go::TestingSuite"}),
        "{set_t}"
    );
}

/// A few unbound calls are listed line by line, grouped by what the graph
/// knows of them (through an interface the repository declares, or on a
/// value of unknown type) and by the function they are in; only the lines
/// on a value of unknown type are left to read. Past the cap the summary
/// counts them by kind and names callers to read.
#[test]
fn go_callers_of_lists_the_lines_of_a_few_unbound_calls() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("go-unbound-call-lines", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "p/p.go",
        "package p\n\ntype closer interface {\n\tClose()\n\tFlush()\n}\n\n\
         type File struct{}\n\n\
         func (f *File) Close() {}\n\n\
         func (f *File) Flush() {}\n\n\
         func Shut(c closer) {\n\tc.Close()\n\tc.Close()\n}\n\n\
         func Drain(c closer) {\n\tc.Flush()\n\tc.Flush()\n\tc.Flush()\n\tc.Flush()\n\
         \tc.Flush()\n\tc.Flush()\n\tc.Flush()\n}\n",
    );
    repo.write(
        "p/q.go",
        "package p\n\nfunc Done(m map[string]interface{ Close() }) {\n\
         \tfor _, c := range m {\n\t\tc.Close()\n\t}\n}\n\n\
         func Direct(f *File) {\n\tf.Close()\n\tf.Flush()\n}\n",
    );
    repo.build();
    let context = repo.context();
    let callers = |target: &str| {
        answer(
            &context,
            "query_graph_tool",
            json!({"pattern": "callers_of", "target": target}),
        )
    };
    let close = callers("p/p.go::File.Close");
    let summary = close["summary"].as_str().expect("summary");
    assert!(
        summary.contains(
            "the 3 that go to no definition are at p/p.go:15 and :16 in Shut, through \
             interface closer (p/p.go:3); and at p/q.go:5 in Done, on a value whose type the \
             graph does not know; a call through an interface reaches File.Close only where \
             the value's dynamic type is File or embeds it; read the line on a value of \
             unknown type to decide whether it calls Close"
        ),
        "{summary}"
    );
    assert_eq!(
        close["next"][0]["args"],
        json!({"pattern": "source_of", "target": "p/q.go::Done"}),
        "{close}"
    );
    assert_eq!(
        close["next"][0]["why"],
        json!("the calls named Close at p/q.go:5 go to no definition and may call it"),
        "{close}"
    );
    let flush = callers("p/p.go::File.Flush");
    let summary = flush["summary"].as_str().expect("summary");
    assert!(
        summary.contains(
            "7 calls named Flush go to no definition in the repository, all through \
             interface closer (p/p.go:3); a call through an interface reaches File.Flush only \
             where the value's dynamic type is File or embeds it, so callers may be missing: \
             read p/p.go::Drain"
        ),
        "{summary}"
    );
}

/// When every Go call of the name that goes to no definition lies in a
/// function the reply already lists (here the bare-name rows of a method
/// called only through an interface), the list holds every caller: the
/// summary says so and says what those calls are, and neither it nor
/// `next` sends the agent to read more callers or to re-read a row.
#[test]
fn go_callers_of_says_no_caller_is_missing_when_unbound_calls_are_in_its_rows() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("go-unbound-calls-in-rows", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "z/enc.go",
        "package z\n\ntype encoder interface{ CRC() uint32 }\n\n\
         type fastBase struct{ n uint32 }\n\n\
         func (f *fastBase) CRC() uint32 { return f.n }\n\n\
         type fast struct{ fastBase }\n\n\
         func Write(enc encoder) uint32 {\n\ta := enc.CRC()\n\treturn a + enc.CRC()\n}\n\n\
         func Close(enc encoder) uint32 {\n\treturn enc.CRC()\n}\n",
    );
    repo.build();
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "z/enc.go::fastBase.CRC"}),
    );
    let rows: Vec<&str> = reply["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(|row| row["qualified_name"].as_str())
        .collect();
    assert_eq!(rows.len(), 2, "{reply}");
    let summary = reply["summary"].as_str().expect("summary");
    assert!(
        summary.contains(
            "Every call named CRC in the repository, _test.go files included, is bound to a \
             definition or lies in a function listed here, so no caller is missing from the \
             list; the rows are matched by name (match \"bare_name\"), not bound by type: their \
             3 calls of CRC that go to no definition are all through interface encoder \
             (z/enc.go:3)"
        ),
        "{summary}"
    );
    assert!(!summary.contains("may be missing"), "{summary}");
    assert!(!summary.contains("read "), "{summary}");
    let next = reply["next"].as_array().expect("next");
    assert_eq!(
        next[0]["args"],
        json!({"pattern": "source_of", "target": "z/enc.go::encoder"}),
        "{reply}"
    );
    let targets: Vec<&str> = next
        .iter()
        .filter_map(|call| call["args"]["target"].as_str())
        .collect();
    let distinct: HashSet<&str> = targets.iter().copied().collect();
    assert_eq!(distinct.len(), targets.len(), "{reply}");
}

/// A call on a value of another package's interface (`m demo.Marker`, here
/// the module's root package) is named as a call through that interface,
/// found by the import path its package is known by.
#[test]
fn go_callers_of_names_an_interface_of_another_package() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("go-unbound-calls-other-package", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "demo.go",
        "package demo\n\ntype Marker interface{ Mark() }\n",
    );
    repo.write(
        "b/b.go",
        "package b\n\ntype T struct{}\n\nfunc (T) Mark() {}\n\nfunc Direct(t T) {\n\tt.Mark()\n}\n",
    );
    repo.write(
        "cmd/use.go",
        "package main\n\nimport \"example.com/demo\"\n\n\
         func Use(m demo.Marker) {\n\tm.Mark()\n}\n",
    );
    repo.build();
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "b/b.go::T.Mark"}),
    );
    let summary = reply["summary"].as_str().expect("summary");
    assert!(
        summary.contains("is at cmd/use.go:6 in Use, through interface Marker (demo.go:3)"),
        "{summary}"
    );
    assert!(!summary.contains("read "), "{summary}");
}

/// A call through a standard-library interface (`w.Write(p)` on `w
/// io.Writer`) goes to the standard library, not to the method it may
/// reach: an exported `Write` is not claimed to have every call bound.
#[test]
fn go_callers_of_counts_calls_through_a_standard_library_interface() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("go-std-interface-calls", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "gz/gz.go",
        "package gz\n\nimport (\n\t\"bytes\"\n\t\"io\"\n)\n\ntype Writer struct{ n int }\n\n\
         func (z *Writer) Write(p []byte) (int, error) {\n\tz.n += len(p)\n\treturn len(p), nil\n}\n\n\
         func (z *Writer) Equal(p []byte) bool {\n\treturn bytes.Equal(p, nil)\n}\n\n\
         func Direct(z *Writer) {\n\tz.Write(nil)\n\tz.Equal(nil)\n}\n\n\
         func Copy(gz *Writer, p []byte) {\n\tvar w io.Writer = gz\n\tw.Write(p)\n}\n",
    );
    repo.build();
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "gz/gz.go::Writer.Write"}),
    );
    let summary = reply["summary"].as_str().expect("summary");
    assert!(!summary.contains("is missing;"), "{summary}");
    assert!(
        summary.contains("the 1 that goes to no definition"),
        "{summary}"
    );
    assert!(
        summary.contains(
            "is at gz/gz.go:26 in Copy, on the standard-library type io.Writer; a call on a \
             library type, if that type is an interface, reaches Writer.Write only where the \
             value's dynamic type is Writer or embeds it"
        ),
        "{summary}"
    );
    // The standard library's function `bytes.Equal` is no method call.
    let equal = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "gz/gz.go::Writer.Equal"}),
    );
    let summary = equal["summary"].as_str().expect("summary");
    assert!(
        summary.contains("every call named Equal in the repository"),
        "{summary}"
    );
}

/// A call on what a standard-library function returned (`zw :=
/// gzip.NewWriter(&buf); zw.Reset(&buf)` in an external test package) is the
/// standard library's (#286): it is not counted among the calls that go to
/// no definition, and the summary does not send the agent to read it. A call
/// on a value of unknown origin still is.
#[test]
fn go_callers_of_leaves_out_calls_on_what_a_standard_library_function_returned() {
    // dagayn: tests crates/dagayn-graph/src/postprocess/returned.rs::resolve_returned_receivers
    let repo = Repo::new("go-std-returned-calls", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "gzip/gzip.go",
        "package gzip\n\nimport \"io\"\n\ntype Writer struct{ w io.Writer }\n\n\
         func (z *Writer) Reset(w io.Writer) {\n\tz.w = w\n}\n\n\
         func Direct(w io.Writer) {\n\tz := &Writer{}\n\tz.Reset(w)\n}\n\n\
         func Loop(m map[string]interface{ Reset(io.Writer) }) {\n\
         \tfor _, v := range m {\n\t\tv.Reset(nil)\n\t}\n}\n",
    );
    repo.write(
        "gzip/example_test.go",
        "package gzip_test\n\nimport (\n\t\"bytes\"\n\t\"compress/gzip\"\n)\n\n\
         func ExampleWriter() {\n\tvar buf bytes.Buffer\n\tzw := gzip.NewWriter(&buf)\n\
         \tzw.Reset(&buf)\n\tzr, err := gzip.NewReader(&buf)\n\tif err != nil {\n\t\treturn\n\t}\n\
         \tzr.Reset(&buf)\n}\n",
    );
    repo.build();
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "gzip/gzip.go::Writer.Reset"}),
    );
    let summary = reply["summary"].as_str().expect("summary");
    assert!(
        summary.contains("the 1 that goes to no definition"),
        "{summary}"
    );
    assert!(
        summary.contains("is at gzip/gzip.go:18 in Loop"),
        "{summary}"
    );
    assert!(!summary.contains("ExampleWriter"), "{summary}");
    let next = reply["next"].to_string();
    assert!(!next.contains("ExampleWriter"), "{next}");
}

/// A generic standard-library function or type returns a value of the
/// module's own type (`best := slices.MaxFunc(nodes, cmp); best.Visit()`,
/// `c := p.Load()` on `p atomic.Pointer[Node]`), and a dependency's
/// function may (`lo.MaxBy(nodes, by)`): a call on it may call the module's
/// method, so it stays among the calls that go to no definition.
#[test]
fn go_callers_of_keeps_calls_on_what_a_generic_or_dependency_api_returned() {
    // dagayn: tests crates/dagayn-graph/src/postprocess/returned.rs::resolve_returned_receivers
    let repo = Repo::new("go-std-generic-returned-calls", false);
    repo.write(
        "go.mod",
        "module example.com/demo\n\ngo 1.22\n\nrequire github.com/samber/lo v1.47.0\n",
    );
    repo.write(
        "tree/node.go",
        "package tree\n\ntype Node struct{ n int }\n\n\
         func (x *Node) Visit() {\n\tx.n++\n}\n",
    );
    repo.write(
        "tree/use.go",
        "package tree\n\nimport (\n\t\"slices\"\n\t\"sync/atomic\"\n\n\t\"github.com/samber/lo\"\n)\n\n\
         func UseMax(nodes []*Node, by func(a, b *Node) int) {\n\
         \tbest := slices.MaxFunc(nodes, by)\n\tbest.Visit()\n}\n\n\
         func UseLoad(p *atomic.Pointer[Node]) {\n\tc := p.Load()\n\tc.Visit()\n}\n\n\
         func UseDep(nodes []*Node, by func(a, b *Node) bool) {\n\
         \tbest := lo.MaxBy(nodes, by)\n\tbest.Visit()\n}\n",
    );
    repo.build();
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "tree/node.go::Node.Visit"}),
    );
    let summary = reply["summary"].as_str().expect("summary");
    let rows = reply["results"].to_string();
    for function in ["UseMax", "UseLoad", "UseDep"] {
        assert!(
            summary.contains(&format!("in {function}")) || rows.contains(function),
            "{function} is neither a caller nor an unbound call: {summary}"
        );
    }
}

/// Go calls bound across the files of a package are `HIGH`, so a
/// `callers_of` made only of them answers with `confidence: "high"`: a
/// plain call of the package's function, and method calls on a receiver
/// typed by a parameter, by a struct field declared in another file, and by
/// what `New` returns (`assert := New(t); assert.JSONEq(..)`).
#[test]
fn go_calls_bound_within_a_package_are_high_confidence() {
    // dagayn: tests crates/dagayn-graph/src/postprocess/bare_names.rs::bind_bare_call_targets
    let repo = Repo::new("go-package-bound-tiers", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "assert/assertions.go",
        "package assert\n\ntype TestingT interface{ Errorf(string, ...any) }\n\n\
         func JSONEq(t TestingT, expected, actual string) bool {\n\treturn expected == actual\n}\n",
    );
    repo.write(
        "assert/forward_assertions.go",
        "package assert\n\ntype Assertions struct{ t TestingT }\n\n\
         func New(t TestingT) *Assertions {\n\treturn &Assertions{t: t}\n}\n",
    );
    repo.write(
        "assert/assertion_forward.go",
        "package assert\n\nfunc (a *Assertions) JSONEq(expected, actual string) bool {\n\
         \treturn JSONEq(a.t, expected, actual)\n}\n",
    );
    repo.write(
        "assert/holder.go",
        "package assert\n\ntype holder struct{ checks *Assertions }\n\n\
         func check(a *Assertions) {\n\ta.JSONEq(\"1\", \"1\")\n}\n",
    );
    repo.write(
        "assert/runner.go",
        "package assert\n\nfunc (h *holder) run() {\n\th.checks.JSONEq(\"2\", \"2\")\n}\n",
    );
    repo.write(
        "assert/forward_test.go",
        "package assert\n\nimport \"testing\"\n\n\
         func TestJSONEq(t *testing.T) {\n\tassert := New(t)\n\tassert.JSONEq(\"{}\", \"{}\")\n}\n",
    );
    repo.build();
    let context = repo.context();
    let callers = |target: &str| {
        answer(
            &context,
            "query_graph_tool",
            json!({"pattern": "callers_of", "target": target}),
        )
    };
    let tiers = |reply: &Value| {
        let mut rows: Vec<(String, String)> = reply["results"]
            .as_array()
            .expect("results")
            .iter()
            .map(|row| {
                (
                    row["qualified_name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    row["confidence_tier"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        rows.sort();
        rows
    };
    let method = callers("assert/assertion_forward.go::Assertions.JSONEq");
    assert_eq!(
        tiers(&method),
        vec![
            (
                "assert/forward_test.go::TestJSONEq".to_string(),
                "HIGH".to_string()
            ),
            ("assert/holder.go::check".to_string(), "HIGH".to_string()),
            (
                "assert/runner.go::holder.run".to_string(),
                "HIGH".to_string()
            ),
        ],
        "{method}"
    );
    assert_eq!(method["confidence"], "high", "{method}");
    let function = callers("assert/assertions.go::JSONEq");
    assert_eq!(
        tiers(&function),
        vec![(
            "assert/assertion_forward.go::Assertions.JSONEq".to_string(),
            "HIGH".to_string()
        )],
        "{function}"
    );
    assert_eq!(function["confidence"], "high", "{function}");
}

/// A test double's method in a `_test.go` file is test code, not a test:
/// `tests_for` on `CollectT.Errorf` names the test that calls it, not the
/// doubles' methods that share its name.
#[test]
fn go_tests_for_a_method_skips_test_doubles_of_its_name() {
    // dagayn: tests crates/dagayn-tools/src/coverage.rs::infer_tests_for_node
    let repo = Repo::new("go-tests-for-doubles", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "assert/assertions.go",
        "package assert\n\n// CollectT collects errors.\ntype CollectT struct{ n int }\n\n\
         // Errorf records an error.\nfunc (c *CollectT) Errorf(format string, args ...any) {\n\tc.n++\n}\n",
    );
    repo.write(
        "assert/assertions_test.go",
        "package assert\n\nimport \"testing\"\n\ntype mockT struct{ n int }\n\n\
         func (m *mockT) Errorf(format string, args ...any) {\n\tm.n++\n}\n\n\
         func TestCollect(t *testing.T) {\n\tc := &CollectT{}\n\tc.Errorf(\"x\")\n}\n",
    );
    repo.build();
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "assert/assertions.go::CollectT.Errorf"}),
    );
    let rows: Vec<&str> = reply["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(|row| row["qualified_name"].as_str())
        .collect();
    assert_eq!(rows, ["assert/assertions_test.go::TestCollect"], "{reply}");
}

/// A test double's method under a Rust crate's `tests/` is test code, not
/// a test: `tests_for` on `Args.new` names the test that calls it, not the
/// double's `Repo.new` (#262).
#[test]
fn rust_tests_for_a_method_skips_test_doubles_of_its_name() {
    // dagayn: tests crates/dagayn-parser/src/rust_lang/mod.rs::rust_walk_children
    let repo = Repo::new("rust-tests-for-doubles", false);
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"crates/app\"]\n");
    repo.write("crates/app/Cargo.toml", "[package]\nname = \"app\"\n");
    repo.write(
        "crates/app/src/lib.rs",
        "pub struct Args;\n\nimpl Args {\n    pub fn new() -> Args {\n        Args\n    }\n}\n",
    );
    repo.write(
        "crates/app/tests/cli.rs",
        "use app::Args;\n\nstruct Repo;\n\nimpl Repo {\n    fn new() -> Repo {\n        Repo\n    }\n}\n\n\
         #[test]\nfn args_build() {\n    let _repo = Repo::new();\n    let _args = Args::new();\n}\n",
    );
    repo.build();
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "crates/app/src/lib.rs::Args.new"}),
    );
    let rows: Vec<&str> = reply["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(|row| row["qualified_name"].as_str())
        .collect();
    assert_eq!(rows, ["crates/app/tests/cli.rs::args_build"], "{reply}");
}

/// Tests at the same hop tie, and `tests_for` names the first by qualified
/// name on every call rather than whichever the walk met first.
#[test]
fn tests_for_breaks_a_nearest_test_tie_by_name() {
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    let repo = Repo::new("nearest-test-tie", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    let names = ["Zeta", "Eta", "Kappa", "Alpha", "Omicron", "Beta"];
    let mut lib = String::from(
        "package demo\n\n// Leaf is what the tests reach.\nfunc Leaf() int {\n\treturn 1\n}\n\n\
         // Mid is called by one helper per test.\nfunc Mid() int {\n\treturn 2\n}\n",
    );
    let mut tests = String::from("package demo\n\nimport \"testing\"\n");
    for name in names {
        lib.push_str(&format!(
            "\n// Via{name} calls Mid.\nfunc Via{name}() int {{\n\treturn Mid()\n}}\n"
        ));
        tests.push_str(&format!(
            "\nfunc Test{name}(t *testing.T) {{\n\tif Leaf()+Via{name}() != 3 {{\n\t\tt.Fatal(\"{name}\")\n\t}}\n}}\n"
        ));
    }
    repo.write("demo.go", &lib);
    repo.write("demo_test.go", &tests);
    repo.build();
    let context = repo.context();
    for (target, hops) in [("demo.go::Leaf", 0), ("demo.go::Mid", 1)] {
        for _ in 0..16 {
            let reply = answer(
                &context,
                "query_graph_tool",
                json!({"pattern": "tests_for", "target": target}),
            );
            let reach = &reply["test_reach"];
            assert_eq!(
                reach["nearest_test"], "demo_test.go::TestAlpha",
                "{target}: {reply}"
            );
            assert_eq!(reach["hops"], hops, "{target}: {reply}");
        }
    }
}
