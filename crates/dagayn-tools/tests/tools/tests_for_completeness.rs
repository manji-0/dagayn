//! What a `tests_for` reply with rows says it covers: the tests that use
//! the target as a value (#328, G10 H2), and rows chosen by name alone.

use serde_json::{Value, json};

use crate::common::{Repo, answer};

fn names<'a>(rows: &'a Value, key: &str) -> Vec<&'a str> {
    let mut names: Vec<&str> = rows
        .as_array()
        .unwrap_or_else(|| panic!("{rows}"))
        .iter()
        .filter_map(|row| row[key].as_str())
        .collect();
    names.sort_unstable();
    names
}

/// chi's `middleware/throttle.go` in small: two tests call `ThrottleWithOpts`,
/// two more (and a benchmark) call it through `ThrottleBacklog`, and a test
/// under `_examples/` (which `go test ./...` skips) calls `ThrottleBacklog`
/// too. `Recoverer` is passed as a value by three tests (`r.Use(Recoverer)`),
/// one of them named after it. `Parse` has a test named after it that never
/// calls it.
fn middleware(label: &str) -> Repo {
    // One label per test: two repos made in the same microsecond share a name.
    let repo = Repo::new(label, false);
    repo.write("go.mod", "module example.com/mw\n\ngo 1.22\n");
    repo.write(
        "middleware/throttle.go",
        "package middleware\n\n\
         // Throttle limits.\nfunc Throttle(limit int) func() int {\n\treturn ThrottleWithOpts(limit, 0)\n}\n\n\
         // ThrottleBacklog limits with a backlog.\n\
         func ThrottleBacklog(limit, backlog int) func() int {\n\treturn ThrottleWithOpts(limit, backlog)\n}\n\n\
         // ThrottleWithOpts limits with options.\n\
         func ThrottleWithOpts(limit, backlog int) func() int {\n\treturn func() int {\n\t\treturn limit + backlog\n\t}\n}\n",
    );
    repo.write(
        "middleware/throttle_test.go",
        "package middleware\n\nimport \"testing\"\n\n\
         func TestThrottleBacklog(t *testing.T) {\n\tThrottleBacklog(1, 2)()\n}\n\n\
         func TestThrottleMaximum(t *testing.T) {\n\tThrottleBacklog(3, 3)()\n}\n\n\
         func TestThrottleRetryAfter(t *testing.T) {\n\tThrottleWithOpts(1, 0)()\n}\n\n\
         func TestThrottleCustomStatusCode(t *testing.T) {\n\tThrottleWithOpts(2, 0)()\n}\n\n\
         func BenchmarkThrottle(b *testing.B) {\n\tThrottleBacklog(1, 1)()\n}\n",
    );
    repo.write(
        "middleware/recoverer.go",
        "package middleware\n\n\
         // Recoverer recovers.\nfunc Recoverer(next func()) func() {\n\treturn func() {\n\t\tdefer func() { _ = recover() }()\n\t\tnext()\n\t}\n}\n\n\
         // Parse parses.\nfunc Parse(s string) int {\n\treturn len(s)\n}\n",
    );
    repo.write(
        "middleware/recoverer_test.go",
        "package middleware\n\nimport \"testing\"\n\n\
         type router struct{ mws []func(func()) func() }\n\n\
         func (r *router) Use(mw func(func()) func()) {\n\tr.mws = append(r.mws, mw)\n}\n\n\
         func TestRecoverer(t *testing.T) {\n\tr := &router{}\n\tr.Use(Recoverer)\n}\n\n\
         func TestRecovererNoColor(t *testing.T) {\n\tr := &router{}\n\tr.Use(Recoverer)\n}\n\n\
         func TestRecovererAbortHandler(t *testing.T) {\n\tr := &router{}\n\tr.Use(Recoverer)\n}\n\n\
         func TestParse(t *testing.T) {\n\tif len(\"x\") != 1 {\n\t\tt.Fatal(\"len\")\n\t}\n}\n",
    );
    repo.write(
        "_examples/limits/main_test.go",
        "package main\n\nimport (\n\t\"testing\"\n\n\t\"example.com/mw/middleware\"\n)\n\n\
         func TestLimits(t *testing.T) {\n\tmiddleware.ThrottleBacklog(1, 1)()\n}\n",
    );
    repo.build();
    repo
}

/// G10 H2 (#328): a test that passes the target as a value is a row, with
/// evidence saying so, not only the test named after it.
#[test]
fn tests_for_lists_tests_that_use_the_target_as_a_value() {
    // dagayn: tests crates/dagayn-tools/src/coverage.rs::infer_tests_for_node
    let repo = middleware("go-tests-by-value-use");
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "middleware/recoverer.go::Recoverer"}),
    );
    assert_eq!(
        names(&reply["results"], "qualified_name"),
        [
            "middleware/recoverer_test.go::TestRecoverer",
            "middleware/recoverer_test.go::TestRecovererAbortHandler",
            "middleware/recoverer_test.go::TestRecovererNoColor",
        ],
        "{reply}"
    );
    for row in reply["results"].as_array().expect("results") {
        assert_eq!(row["coverage_source"], "value_use", "{reply}");
        assert_eq!(
            row["evidence"],
            json!(["test uses target as a value"]),
            "{reply}"
        );
    }
    assert_eq!(reply["results_complete"], true, "{reply}");
    assert!(
        reply["test_reach"].get("through_callers").is_none(),
        "{reply}"
    );
}

/// Rows chosen by name alone are not a complete answer: no edge links them.
#[test]
fn tests_for_by_name_alone_is_not_complete() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = middleware("go-tests-by-name");
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "middleware/recoverer.go::Parse"}),
    );
    assert_eq!(
        names(&reply["results"], "qualified_name"),
        ["middleware/recoverer_test.go::TestParse"],
        "{reply}"
    );
    assert_eq!(
        reply["results"][0]["coverage_source"], "heuristic",
        "{reply}"
    );
    assert_eq!(reply["results_complete"], false, "{reply}");
    let summary = reply["summary"].as_str().unwrap_or_default();
    assert!(summary.contains("chosen by name"), "{summary}");
}

/// An ambiguous name whose answers are chosen by name: each says it is not
/// complete, and none reads as cut, since every row is shown (#319).
#[test]
fn ambiguous_tests_for_by_name_alone_is_not_cut() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = middleware("go-tests-by-name-ambiguous");
    repo.write(
        "parse/parse.go",
        "package parse\n\n// Parse parses.\nfunc Parse(s string) int {\n\treturn 0\n}\n",
    );
    repo.write(
        "parse/parse_test.go",
        "package parse\n\nimport \"testing\"\n\nfunc TestParse(t *testing.T) {\n\tt.Log(\"x\")\n}\n",
    );
    repo.build();
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "Parse"}),
    );
    assert_eq!(reply["status"], "ambiguous", "{reply}");
    let answers = reply["answers"].as_array().expect("answers");
    assert_eq!(answers.len(), 2, "{reply}");
    for item in answers {
        assert_eq!(item["result_count"], 1, "{reply}");
        assert_eq!(item["results_complete"], false, "{reply}");
    }
    let summary = reply["summary"].as_str().unwrap_or_default();
    assert!(!summary.contains("cut"), "{summary}");
}
