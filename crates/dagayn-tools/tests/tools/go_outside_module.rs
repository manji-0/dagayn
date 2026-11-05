//! Go rows where the Go tools building the target's module do not look
//! (`_examples/`, `.hidden/`, `testdata/`, another module's directory) are
//! listed in `outside_module`, apart from the answer (#325).

use serde_json::{Value, json};

use crate::common::{Repo, answer};

/// A module whose `lib.Do` is called from the module's own code and test,
/// from `_examples/` (in the module and in a module of its own), `.hidden/`
/// and `testdata/`; `lib.Helper` only from a test under `_examples/`.
fn module() -> Repo {
    let repo = Repo::new("go-outside-module", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "lib/lib.go",
        "package lib\n\n// Do does.\nfunc Do() int {\n\treturn 1\n}\n\n\
         // Helper helps.\nfunc Helper() int {\n\treturn 2\n}\n",
    );
    repo.write(
        "lib/lib_test.go",
        "package lib\n\nimport \"testing\"\n\nfunc TestDo(t *testing.T) {\n\tDo()\n}\n",
    );
    let caller = |package: &str, function: &str| {
        format!(
            "package {package}\n\nimport \"example.com/demo/lib\"\n\n\
             func {function}() int {{\n\treturn lib.Do()\n}}\n"
        )
    };
    repo.write("cmd/app/main.go", &caller("main", "run"));
    repo.write("_examples/hello/main.go", &caller("main", "hello"));
    repo.write(
        "_examples/hello/main_test.go",
        "package main\n\nimport (\n\t\"testing\"\n\n\t\"example.com/demo/lib\"\n)\n\n\
         func TestHello(t *testing.T) {\n\tlib.Do()\n\tlib.Helper()\n}\n",
    );
    repo.write(
        "_examples/rest/go.mod",
        "module example.com/rest\n\ngo 1.22\n\nrequire example.com/demo v0.0.0\n",
    );
    repo.write("_examples/rest/main.go", &caller("main", "rest"));
    repo.write("tools/go.mod", "module example.com/tools\n\ngo 1.22\n");
    repo.write("tools/gen/main.go", &caller("main", "generate"));
    repo.write(".hidden/hidden.go", &caller("hidden", "hidden"));
    repo.write("lib/testdata/fixture.go", &caller("fixture", "fixture"));
    repo.build();
    repo
}

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

#[test]
fn go_callers_outside_the_module_are_listed_apart() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = module();
    let context = repo.context();
    let query = |arguments: Value| answer(&context, "query_graph_tool", arguments);
    let callers = query(json!({"pattern": "callers_of", "target": "lib/lib.go::Do"}));
    assert_eq!(
        names(&callers["results"], "qualified_name"),
        ["cmd/app/main.go::run", "lib/lib_test.go::TestDo"],
        "{callers}"
    );
    assert_eq!(callers["result_count"], 2, "{callers}");
    let outside = &callers["outside_module"];
    let reasons: Vec<(&str, &str)> = outside
        .as_array()
        .expect("outside_module")
        .iter()
        .map(|row| {
            (
                row["qualified_name"].as_str().unwrap_or(""),
                row["reason"].as_str().unwrap_or(""),
            )
        })
        .collect();
    for expected in [
        ("_examples/hello/main.go::hello", "underscore_name"),
        ("_examples/hello/main_test.go::TestHello", "underscore_name"),
        (".hidden/hidden.go::hidden", "dot_name"),
        ("lib/testdata/fixture.go::fixture", "testdata"),
        // Under a go.mod of its own, inside `_examples/` or not.
        ("_examples/rest/main.go::rest", "other_module"),
        ("tools/gen/main.go::generate", "other_module"),
    ] {
        assert!(reasons.contains(&expected), "{expected:?}: {callers}");
    }
    assert_eq!(reasons.len(), 6, "{callers}");
    assert_eq!(callers["outside_module_count"], 6);
    assert!(outside[0]["lines"].is_array(), "{outside}");
    let summary = callers["summary"].as_str().unwrap_or_default();
    assert!(
        summary.contains(
            "6 callers are in directories the Go tools do not build for this module, listed in \
             outside_module and not counted in results: "
        ),
        "{summary}"
    );
    assert!(
        summary.contains("_examples/ (2, name starts with _)"),
        "{summary}"
    );
    assert!(
        summary.contains(".hidden/ (1, name starts with .)"),
        "{summary}"
    );
    assert!(summary.contains("lib/testdata/ (1, testdata)"), "{summary}");
    assert!(
        summary.contains("_examples/rest/ (1, its own module)"),
        "{summary}"
    );
    assert!(summary.contains("tools/ (1, its own module)"), "{summary}");
    // `next` reads the answer, never a row outside it.
    let next = callers["next"].as_array().expect("next");
    assert!(!next.is_empty(), "{callers}");
    for call in next {
        let target = call["args"]["target"].as_str().unwrap_or("");
        assert!(
            !target.starts_with('_') && !target.starts_with('.') && !target.contains("testdata"),
            "{callers}"
        );
    }

    // A walk splits its rows at every hop, and minimal lists them too.
    let walked = query(json!({
        "pattern": "callers_of", "target": "lib/lib.go::Do", "depth": 3,
        "detail_level": "minimal",
    }));
    assert_eq!(walked["result_count"], 2, "{walked}");
    assert!(
        walked["outside_module_count"].as_u64() >= Some(4),
        "{walked}"
    );

    // Importers of the package: the module's own, not the example's.
    let importers = query(json!({"pattern": "importers_of", "target": "lib"}));
    let files = names(&importers["results"], "file");
    assert!(files.contains(&"cmd/app/main.go"), "{importers}");
    assert!(
        files
            .iter()
            .all(|file| !file.starts_with('_') && !file.starts_with('.')),
        "{importers}"
    );
    assert!(
        names(&importers["outside_module"], "file").contains(&"_examples/hello/main.go"),
        "{importers}"
    );
}

#[test]
fn go_tests_outside_the_module_are_not_its_tests() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = module();
    let context = repo.context();
    let query = |arguments: Value| answer(&context, "query_graph_tool", arguments);
    let tests = query(json!({"pattern": "tests_for", "target": "lib/lib.go::Do"}));
    assert_eq!(
        names(&tests["results"], "qualified_name"),
        ["lib/lib_test.go::TestDo"],
        "{tests}"
    );
    assert_eq!(
        names(&tests["outside_module"], "qualified_name"),
        ["_examples/hello/main_test.go::TestHello"],
        "{tests}"
    );
    assert_eq!(
        tests["test_reach"]["nearest_test"],
        "lib/lib_test.go::TestDo"
    );

    // Only a test `go test ./...` never runs calls Helper: no test reaches
    // it, and the nearest test is not that one.
    let helper = query(json!({"pattern": "tests_for", "target": "lib/lib.go::Helper"}));
    assert_eq!(helper["result_count"], 0, "{helper}");
    assert_eq!(helper["outside_module_count"], 1, "{helper}");
    assert_eq!(
        helper["test_reach"]["nearest_test"],
        Value::Null,
        "{helper}"
    );
    assert_eq!(helper["test_reach"]["counts_as_tested"], false, "{helper}");
    assert_eq!(
        helper["zero_result_reason"], "outside_module_only",
        "{helper}"
    );
    let summary = helper["summary"].as_str().unwrap_or_default();
    assert!(
        summary.contains("1 test is in a directory the Go tools do not build"),
        "{summary}"
    );
    assert!(!summary.contains("no test in it reaches it"), "{summary}");
    let full = query(json!({
        "pattern": "tests_for", "target": "lib/lib.go::Helper", "detail_level": "full",
    }));
    assert_eq!(
        full["guidance"][0]["reason_codes"],
        json!(["zero_result", "outside_module_only"]),
        "{full}"
    );
}

#[test]
fn ambiguous_go_candidates_count_their_rows_outside_the_module() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = module();
    repo.write(
        "other/other.go",
        "package other\n\nfunc Do() int {\n\treturn 3\n}\n",
    );
    repo.build();
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "Do"}),
    );
    assert_eq!(reply["status"], "ambiguous", "{reply}");
    let answers = reply["answers"].as_array().expect("answers");
    let lib = answers
        .iter()
        .find(|answer| answer["target"] == "lib/lib.go::Do")
        .unwrap_or_else(|| panic!("{reply}"));
    assert_eq!(lib["result_count"], 2, "{reply}");
    assert_eq!(lib["outside_module_count"], 6, "{reply}");
}

#[test]
fn a_go_target_outside_the_module_keeps_its_neighbours() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = module();
    repo.write(
        "_examples/hello/util.go",
        "package main\n\nfunc greet() string {\n\treturn \"hi\"\n}\n\n\
         func caller() string {\n\treturn greet()\n}\n",
    );
    repo.build();
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "_examples/hello/util.go::greet"}),
    );
    assert_eq!(
        names(&reply["results"], "qualified_name"),
        ["_examples/hello/util.go::caller"],
        "{reply}"
    );
    assert!(reply.get("outside_module").is_none(), "{reply}");
}

#[test]
fn other_languages_keep_rows_in_underscore_directories() {
    // dagayn: tests crates/dagayn-tools/src/query/mod.rs::query_graph
    let repo = Repo::new("python-underscore-dirs", false);
    repo.write(
        "_examples/demo.py",
        "from app import helper\n\n\ndef run():\n    return helper()\n",
    );
    repo.build();
    let reply = answer(
        &repo.context(),
        "query_graph_tool",
        json!({"pattern": "callers_of", "target": "app.py::helper"}),
    );
    assert!(
        names(&reply["results"], "qualified_name").contains(&"_examples/demo.py::run"),
        "{reply}"
    );
    assert!(reply.get("outside_module").is_none(), "{reply}");
    assert!(reply.get("outside_module_count").is_none(), "{reply}");
}
