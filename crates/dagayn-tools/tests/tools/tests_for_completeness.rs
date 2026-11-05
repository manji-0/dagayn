//! What a `tests_for` reply with rows says it covers: the tests that run the
//! target through its callers (#327, G10 H1), the tests that use it as a
//! value (#328, G10 H2), and rows chosen by name alone.

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
/// one of them named after it, and calls `printPrettyStack`, which calls
/// `prettyStack.decorate` (G10 H2's chain, where it is `parse`). `Parse` has
/// a test named after it that never calls it.
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
         // Recoverer recovers.\nfunc Recoverer(next func()) func() {\n\treturn func() {\n\t\tdefer func() {\n\t\t\tif recover() != nil {\n\t\t\t\tprintPrettyStack(nil)\n\t\t\t}\n\t\t}()\n\t\tnext()\n\t}\n}\n\n\
         type prettyStack struct{}\n\n\
         func (s prettyStack) decorate(debugStack []byte) int {\n\treturn len(debugStack)\n}\n\n\
         func printPrettyStack(debugStack []byte) {\n\tvar s prettyStack\n\ts.decorate(debugStack)\n}\n\n\
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

/// G10 H1 (#327): with direct tests, the reply still says which tests reach
/// the target through its callers, names the nearest, and `next` lists them.
#[test]
fn tests_for_with_direct_tests_counts_the_tests_through_callers() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    // dagayn: tests crates/dagayn-tools/src/findings.rs::tests_reaching
    let repo = middleware("go-tests-through-callers");
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "middleware/throttle.go::ThrottleWithOpts"}),
    );
    assert_eq!(
        names(&reply["results"], "qualified_name"),
        [
            "middleware/throttle_test.go::TestThrottleCustomStatusCode",
            "middleware/throttle_test.go::TestThrottleRetryAfter",
        ],
        "{reply}"
    );
    // The rows are every direct test, so the list is complete for what it
    // covers; the summary says what that is and what lies past it.
    assert_eq!(reply["results_complete"], true, "{reply}");
    let through = &reply["test_reach"]["through_callers"];
    assert_eq!(through["count"], 3, "{reply}");
    assert_eq!(
        names(&through["tests"], "qualified_name"),
        [
            "middleware/throttle_test.go::BenchmarkThrottle",
            "middleware/throttle_test.go::TestThrottleBacklog",
            "middleware/throttle_test.go::TestThrottleMaximum",
        ],
        "{reply}"
    );
    for test in through["tests"].as_array().expect("tests") {
        assert_eq!(test["hops"], 1, "{reply}");
        assert_eq!(
            test["via"], "middleware/throttle.go::ThrottleBacklog",
            "{reply}"
        );
    }
    let summary = reply["summary"].as_str().unwrap_or_default();
    assert!(summary.contains("3 more test(s)"), "{summary}");
    assert!(
        summary.contains("middleware/throttle_test.go::BenchmarkThrottle"),
        "{summary}"
    );
    assert!(!summary.contains("_examples"), "{summary}");
    assert_eq!(
        reply["next"][0]["args"],
        json!({"pattern": "tests_for", "target": "middleware/throttle.go::ThrottleBacklog"}),
        "{}",
        reply["next"]
    );
    // The call in `next` lists them.
    let listed = answer(
        &context,
        "query_graph_tool",
        reply["next"][0]["args"].clone(),
    );
    assert_eq!(
        names(&listed["results"], "qualified_name"),
        [
            "middleware/throttle_test.go::BenchmarkThrottle",
            "middleware/throttle_test.go::TestThrottleBacklog",
            "middleware/throttle_test.go::TestThrottleMaximum",
        ],
        "{listed}"
    );
    // A row is not counted again among the tests through callers.
    assert!(!summary.contains("TestThrottleRetryAfter ("), "{summary}");
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

/// G10 H2 (#326): with no direct test, the reply lists the tests that reach
/// the target through its callers, says which hop is a value use, and
/// `next` is a call that returns them, not a depth `callers_of`.
#[test]
fn tests_for_without_direct_tests_lists_the_tests_through_callers() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    // dagayn: tests crates/dagayn-tools/src/findings.rs::tests_reaching
    let repo = middleware("go-zero-row-tests-through-callers");
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "middleware/recoverer.go::prettyStack.decorate"}),
    );
    assert_eq!(reply["result_count"], 0, "{reply}");
    assert_eq!(
        reply["test_reach"]["nearest_test"], "middleware/recoverer_test.go::TestRecoverer",
        "{reply}"
    );
    let through = &reply["test_reach"]["through_callers"];
    assert_eq!(through["count"], 3, "{reply}");
    assert_eq!(
        names(&through["tests"], "qualified_name"),
        [
            "middleware/recoverer_test.go::TestRecoverer",
            "middleware/recoverer_test.go::TestRecovererAbortHandler",
            "middleware/recoverer_test.go::TestRecovererNoColor",
        ],
        "{reply}"
    );
    for test in through["tests"].as_array().expect("tests") {
        assert_eq!(test["hops"], 3, "{reply}");
        assert_eq!(test["via"], "middleware/recoverer.go::Recoverer", "{reply}");
        assert_eq!(
            test["value_use"],
            json!({"user": test["qualified_name"], "value": "middleware/recoverer.go::Recoverer"}),
            "{reply}"
        );
    }
    let summary = reply["summary"].as_str().unwrap_or_default();
    assert!(!summary.contains("callers_of with depth"), "{summary}");
    assert!(
        summary.contains("3 test(s) reach it through its callers"),
        "{summary}"
    );
    assert!(
        summary.contains("uses middleware/recoverer.go::Recoverer as a value"),
        "{summary}"
    );
    let next = &reply["next"][0];
    assert_eq!(
        next["args"],
        json!({"pattern": "tests_for", "target": "middleware/recoverer.go::Recoverer"}),
        "{}",
        reply["next"]
    );
    // The call in `next` returns them.
    let listed = answer(&context, "query_graph_tool", next["args"].clone());
    assert_eq!(
        names(&listed["results"], "qualified_name"),
        names(&through["tests"], "qualified_name"),
        "{listed}"
    );
}

/// G10 H2 (#326): a depth `callers_of` lists, at every hop, the places that
/// use a caller as a value, as its first hop does, and does not say that
/// no caller is missing while it does not walk past them.
#[test]
fn callers_of_with_depth_lists_value_uses_past_the_first_hop() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = middleware("go-depth-value-uses");
    let context = repo.context();
    let reply = answer(
        &context,
        "query_graph_tool",
        json!({
            "pattern": "callers_of",
            "target": "middleware/recoverer.go::prettyStack.decorate",
            "depth": 6,
        }),
    );
    let mut rows: Vec<(String, Value, Value, Value)> = reply["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|row| {
            (
                row["qualified_name"].as_str().unwrap_or("").to_string(),
                row["depth"].clone(),
                row["relation"].clone(),
                row["via"].clone(),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let value_use = |test: &str| {
        (
            format!("middleware/recoverer_test.go::{test}"),
            json!(3),
            json!("references"),
            json!("middleware/recoverer.go::Recoverer"),
        )
    };
    assert_eq!(
        rows,
        [
            (
                "middleware/recoverer.go::Recoverer".to_string(),
                json!(2),
                Value::Null,
                json!("middleware/recoverer.go::printPrettyStack"),
            ),
            (
                "middleware/recoverer.go::printPrettyStack".to_string(),
                json!(1),
                Value::Null,
                Value::Null,
            ),
            value_use("TestRecoverer"),
            value_use("TestRecovererAbortHandler"),
            value_use("TestRecovererNoColor"),
        ],
        "{reply}"
    );
    let summary = reply["summary"].as_str().unwrap_or_default();
    assert!(!summary.contains("is missing"), "{summary}");
    assert!(
        summary.contains("3 use it or a caller as a value"),
        "{summary}"
    );
    assert!(summary.contains("does not go past"), "{summary}");
}

/// G10 C13 (#326): among tests at the same hop, a `Test` function is the
/// nearest before an `Example` or a `Benchmark`; equals still tie by name.
#[test]
fn tests_for_prefers_a_test_function_to_an_example_or_benchmark() {
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    let repo = Repo::new("go-nearest-test-over-example", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "index.go",
        "package demo\n\n// Load is what the tests reach.\nfunc Load() int {\n\treturn 1\n}\n\n\
         // Seek calls Load.\nfunc Seek() int {\n\treturn Load()\n}\n",
    );
    repo.write(
        "index_test.go",
        "package demo\n\nimport \"testing\"\n\n\
         func ExampleSeek() {\n\tSeek()\n}\n\n\
         func BenchmarkSeek(b *testing.B) {\n\tSeek()\n}\n\n\
         func TestZeta(t *testing.T) {\n\tSeek()\n}\n\n\
         func TestSeeking(t *testing.T) {\n\tSeek()\n}\n",
    );
    repo.build();
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "index.go::Load"}),
    );
    let reach = &reply["test_reach"];
    assert_eq!(
        reach["nearest_test"], "index_test.go::TestSeeking",
        "{reply}"
    );
    assert_eq!(reach["hops"], 1, "{reply}");
    // Every one is listed, the test functions first.
    let listed: Vec<&str> = reach["through_callers"]["tests"]
        .as_array()
        .expect("tests")
        .iter()
        .filter_map(|test| test["qualified_name"].as_str())
        .collect();
    assert_eq!(
        listed,
        [
            "index_test.go::TestSeeking",
            "index_test.go::TestZeta",
            "index_test.go::BenchmarkSeek",
            "index_test.go::ExampleSeek",
        ],
        "{reply}"
    );
}

/// G10 H2 run 004 (#326): a `depth` above the maximum is walked at the
/// maximum, and the reply says so.
#[test]
fn callers_of_says_when_depth_is_capped() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = middleware("go-depth-capped");
    let context = repo.context();
    let walk = |depth: i64| {
        answer(
            &context,
            "query_graph_tool",
            json!({
                "pattern": "callers_of",
                "target": "middleware/throttle.go::ThrottleWithOpts",
                "depth": depth,
            }),
        )
    };
    let reply = walk(10);
    assert_eq!(reply["depth"], 6, "{reply}");
    assert_eq!(reply["depth_requested"], 10, "{reply}");
    let summary = reply["summary"].as_str().unwrap_or_default();
    assert!(
        summary.contains("depth 10 is above the maximum, 6"),
        "{summary}"
    );
    // At the maximum, nothing is said.
    let six = walk(6);
    assert!(six.get("depth_requested").is_none(), "{six}");
    let summary = six["summary"].as_str().unwrap_or_default();
    assert!(!summary.contains("maximum"), "{summary}");
}
