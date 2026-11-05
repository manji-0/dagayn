use std::process::Command;

use dagayn_tools::{Context, call};
use serde_json::{Value, json};

use crate::common::{Repo, answer, commit_all, declines};

#[test]
fn review_answers_affected_flows_from_the_worktree_and_explicit_files() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("review", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({"package": "dagayn", "pid": 1})),
        ..repo.context()
    };
    repo.write(
        "app.py",
        "def main():\n    return helper()\n\n\ndef helper():\n    return 1\n",
    );
    let auto = answer(&context, "review_tool", json!({"mode": "affected_flows"}));
    assert_eq!(auto["change_file_sources"]["unstaged"], json!(["app.py"]));
    assert_eq!(auto["changed_files"], json!(["app.py"]));
    // Only `helper`'s line changed; `main` reaches it, and `test_main`,
    // which calls `main`, is test code the search does not walk.
    assert_eq!(auto["changed_function_count"], 1);
    assert_eq!(
        auto["entry_points"],
        json!([{
            "entry_point": "app.py::main",
            "kind": "main",
            "hops": 1,
            "chain": ["app.py::main", "app.py::helper"],
            "file": "app.py",
            "line": 1,
        }])
    );
    assert!(auto.get("affected_flows").is_none());
    assert!(auto.get("_runtime").is_none());
    assert_eq!(auto["_repo"].as_object().map(|repo| repo.len()), Some(1));
    assert!(auto["next"].is_array());
    assert!(auto.get("_hints").is_none());
    let text = call(&context, "review_tool", &json!({"mode": "affected_flows"}))
        .expect("answered")
        .text;
    assert!(
        text.starts_with(
            r#"{"status":"ok","mode":"affected_flows","summary":"1 entry point(s) reach the 1 changed function(s) in 1 file(s)"#
        ),
        "{text}"
    );
    let verbose = answer(
        &context,
        "review_tool",
        json!({"mode": "affected_flows", "detail_level": "verbose"}),
    );
    assert_eq!(verbose["entry_points"], auto["entry_points"]);
    assert!(verbose.get("affected_flows").is_none() && verbose.get("total").is_none());
    let none = answer(
        &context,
        "review_tool",
        json!({"mode": "affected_flows", "changed_files": []}),
    );
    assert_eq!(none["summary"], "No changed files detected.");
    assert!(none.get("change_file_sources").is_none());
    let explicit = answer(
        &context,
        "review_tool",
        json!({"mode": "affected_flows", "changed_files": ["./app.py", "gone.py"]}),
    );
    assert_eq!(
        explicit["change_file_sources"],
        json!({"files": ["./app.py", "gone.py"], "explicit": ["./app.py", "gone.py"]})
    );
    assert_eq!(explicit["entry_points"][0]["entry_point"], "app.py::main");
}

#[test]
fn review_leaves_other_modes_and_unknowns_to_python() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("review-declines", true);
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    for arguments in [
        // The removed modes and their arguments.
        json!({"mode": "context"}),
        json!({"mode": "impact"}),
        json!({"mode": "changes", "max_depth": 2}),
        json!({"mode": "affected_flows", "detail_level": "full"}),
        json!({"mode": "affected_flows", "include_source": "yes"}),
        json!({"mode": "affected_flows", "changed_files": [1]}),
        json!({"mode": "affected_flows", "other": 1}),
    ] {
        assert!(
            declines(&context, "review_tool", arguments.clone()),
            "{arguments}"
        );
    }
    // Without the host's `_runtime`, nothing.
    assert!(declines(
        &repo.context(),
        "review_tool",
        json!({"mode": "affected_flows", "changed_files": ["app.py"]})
    ));
    assert!(!declines(
        &context,
        "review_tool",
        json!({"mode": "affected_flows", "changed_files": ["app.py"]})
    ));
}

#[test]
fn review_changes_scores_the_diff_against_base() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/context.rs::get_minimal_context
    let repo = Repo::new("changes", true);
    repo.build();
    repo.write(
        "app.py",
        "def main():\n    return helper()\n\n\ndef helper():\n    return 1\n\n\ndef auth_token():\n    return 2\n",
    );
    for args in [
        &["add", "-A"][..],
        &["commit", "-q", "--no-gpg-sign", "-m", "edit"],
    ] {
        let out = Command::new("git")
            .args(args)
            .current_dir(&repo.0)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .expect("git");
        assert!(out.status.success(), "{out:?}");
    }
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    assert_eq!(changes["mode"], "changes");
    assert!(changes.get("called_subtool").is_none());
    assert_eq!(changes["changed_files"], json!(["app.py"]));
    assert_eq!(changes["base"], "HEAD~1");
    assert!(changes.get("diff_parse_status").is_none());
    assert!(changes.get("risk_level").is_none());
    let names: Vec<&str> = changes["changed_functions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert!(names.contains(&"auth_token"), "{names:?}");
    let added = changes["changed_functions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["name"] == "auth_token")
        .expect("auth_token");
    assert_eq!(added["change_status"], "added");
    // No test reaches the new function: one untested_change for app.py.
    let findings = changes["findings"].as_array().expect("findings");
    let untested = findings
        .iter()
        .find(|f| f["kind"] == "untested_change")
        .expect("untested_change");
    assert_eq!(untested["file"], "app.py");
    assert!(
        untested["targets"]
            .as_array()
            .is_some_and(|t| t.contains(&json!("app.py::auth_token")))
    );
    assert!(
        changes["summary"]
            .as_str()
            .is_some_and(|s| s.starts_with("1 changed file(s), ") && s.contains("untested_change"))
    );
    assert!(changes.get("next_drill_downs").is_none());
    assert!(changes.get("_hints").is_none());
    assert!(
        changes["next"]
            .as_array()
            .is_some_and(|next| !next.is_empty())
    );

    let minimal = answer(
        &context,
        "review_tool",
        json!({"mode": "changes", "detail_level": "minimal"}),
    );
    assert_eq!(minimal["changed_file_count"], 1);
    assert!(minimal.get("changed_functions").is_none());
    assert!(minimal.get("review_priorities").is_none());
    assert_eq!(minimal["findings"], changes["findings"]);

    // verbose adds what changed at base, and the score-first fields are gone.
    let verbose_changes = answer(
        &context,
        "review_tool",
        json!({"mode": "changes", "detail_level": "verbose"}),
    );
    for gone in [
        "risk_score",
        "review_priority_score",
        "score_semantics",
        "review_priorities",
        "test_gaps",
        "test_gap_evidence",
        "changed_edges",
        "analysis_summary",
    ] {
        assert!(verbose_changes.get(gone).is_none(), "{gone}");
    }
    assert_eq!(
        verbose_changes["changed_functions"],
        changes["changed_functions"]
    );
    let rows = changes["changed_functions"].as_array().expect("rows");
    assert!(!rows.is_empty());
    for row in rows {
        assert!(row.get("risk_score").is_none(), "{row}");
        assert!(row.get("review_priority_score").is_none(), "{row}");
    }
    // verbose keeps the diagnosis the agent's envelope leaves out.
    assert_eq!(verbose_changes["called_subtool"], "detect_changes_func");
    assert!(verbose_changes["_runtime"].is_object());
    assert!(verbose_changes["_repo"]["db_path"].is_string());
    assert!(
        verbose_changes["symbol_delta"]["removed"]
            .as_array()
            .is_some()
    );

    let none = answer(&context, "review_tool", json!({"changed_files": []}));
    assert_eq!(none["summary"], "No changed files detected.");
    assert_eq!(none["findings"], json!([]));

    // An explicit list scopes the review: app.py's lines in the base diff
    // add none of its nodes when only another file is named.
    let scoped = answer(
        &context,
        "review_tool",
        json!({"changed_files": ["notes.txt"]}),
    );
    assert_eq!(scoped["changed_files"], json!(["notes.txt"]));
    assert_eq!(scoped["changed_functions"], json!([]));
    let scoped_context = answer(
        &context,
        "get_minimal_context_tool",
        json!({"changed_files": ["notes.txt"]}),
    );
    assert!(
        !scoped_context.to_string().contains("auth_token"),
        "{scoped_context}"
    );

    let sourced = answer(&context, "review_tool", json!({"include_source": true}));
    let functions = sourced["changed_functions"].as_array().unwrap();
    assert!(!functions.is_empty());
    for function in functions {
        let source = function["source"].as_str().unwrap();
        let first = format!("{}: ", function["line_start"]);
        assert!(source.starts_with(&first), "{source}");
    }

    // No HEAD~5 to diff against: the unresolved base is reported, without
    // hints.
    let unresolved = answer(
        &context,
        "review_tool",
        json!({"base": "HEAD~5", "changed_files": ["app.py"]}),
    );
    assert_eq!(unresolved["status"], "error");
    assert_eq!(unresolved["diff_parse_status"], "base_unresolved");
    assert!(
        unresolved["error"]
            .as_str()
            .is_some_and(|e| e.starts_with("Could not resolve the diff base 'HEAD~5' in "))
    );
    assert!(unresolved.get("_hints").is_none());
    let codes: Vec<&str> = unresolved["missingness"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["reason_code"].as_str())
        .collect();
    assert!(codes.contains(&"diff_base_unreachable"), "{codes:?}");

    // A ref Python rejects lists the working tree instead.
    let flows = answer(
        &context,
        "review_tool",
        json!({"mode": "affected_flows", "base": "bad ref"}),
    );
    assert_eq!(flows["summary"], "No changed files detected.");

    // No base: a clean tree reviews the last commit, a dirty one only its
    // work in progress.
    assert_eq!(changes["change_entity_summary"]["base"], "HEAD~1");
    repo.write(
        "app.py",
        "def main():\n    return helper()\n\n\ndef helper():\n    return 3\n\n\ndef auth_token():\n    return 2\n",
    );
    repo.build();
    let dirty = answer(&context, "review_tool", json!({}));
    assert_eq!(dirty["change_entity_summary"]["base"], "HEAD");
    let names: Vec<&str> = dirty["changed_functions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert_eq!(names, ["helper"]);
}

#[test]
fn a_test_that_uses_a_class_reaches_the_methods_the_runtime_calls() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    // dagayn: tests crates/dagayn-tools/src/findings.rs::implicit_caller
    let repo = Repo::new("implicit-methods", true);
    let models = |value: i64| {
        format!(
            "class Box:\n    def __init__(self):\n        self.value = {value}\n\n    @property\n    def size(self):\n        return self.value + {value}\n\n    def grow(self):\n        self.value += {value}\n"
        )
    };
    repo.write("models.py", &models(1));
    repo.write(
        "test_models.py",
        "from models import Box\n\n\ndef test_size():\n    assert Box().size == 2\n",
    );
    commit_all(&repo, "models");
    repo.build();
    repo.write("models.py", &models(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let untested: Vec<&str> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .flat_map(|f| f["targets"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    // Constructing `Box` calls `__init__`; reading `size` calls the
    // property. Nothing calls `grow`.
    assert_eq!(untested, ["models.py::Box.grow"], "{changes}");
}

#[test]
fn a_test_that_uses_a_pydantic_model_reaches_what_pydantic_calls() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    // dagayn: tests crates/dagayn-tools/src/findings.rs::implicit_caller
    // dagayn: tests crates/dagayn-tools/src/findings.rs::runs_on_import
    let repo = Repo::new("pydantic-reach", true);
    let types = |value: i64| {
        format!(
            "from typing import Annotated, Any\n\nfrom pydantic import AfterValidator\n\n\ndef check_even(v: int) -> int:\n    return v + {value}\n\n\nEvenInt = Annotated[int, AfterValidator(check_even)]\n\n\nclass Celsius(float):\n    @classmethod\n    def __get_pydantic_core_schema__(cls, source: Any, handler: Any) -> Any:\n        return handler(float) if {value} else None\n"
        )
    };
    let models = |value: i64| {
        format!(
            "from typing import Any, Generic, TypeVar\n\nfrom pydantic import BaseModel, field_validator\n\nfrom types_ import Celsius, EvenInt\n\nT = TypeVar(\"T\")\n\n\nclass Reading(BaseModel):\n    even: EvenInt\n    temp: Celsius\n\n    def model_post_init(self, context: Any) -> None:\n        self.even += {value}\n\n    def unused(self) -> int:\n        return {value}\n\n\nclass Box(BaseModel, Generic[T]):\n    item: T\n\n    @field_validator(\"item\")\n    @classmethod\n    def check_item(cls, v: Any) -> Any:\n        return v or {value}\n\n\nclass Parsed(BaseModel):\n    raw: str\n\n    @field_validator(\"raw\")\n    @classmethod\n    def strip_raw(cls, v: str) -> str:\n        return v.strip() + str({value})\n"
        )
    };
    repo.write("types_.py", &types(1));
    repo.write("models.py", &models(1));
    repo.write(
        "test_models.py",
        "from models import Box, Parsed, Reading\n\n\ndef test_models():\n    assert Reading(even=2, temp=1.5).temp == 1.5\n    assert Box[int](item=1).item == 1\n    assert Parsed.model_validate({\"raw\": \" a \"}).raw\n",
    );
    commit_all(&repo, "models");
    repo.build();
    repo.write("types_.py", &types(2));
    repo.write("models.py", &models(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let mut untested: Vec<&str> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .flat_map(|f| f["targets"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    untested.sort_unstable();
    // Validating a `Reading` runs the alias's validator, `Celsius`'s core
    // schema, and `model_post_init`; `Box[int](...)` and
    // `Parsed.model_validate(...)` run their validators. Nothing calls
    // `unused`.
    assert_eq!(untested, ["models.py::Reading.unused"], "{changes}");
}

#[test]
fn importing_a_module_reaches_what_its_import_runs() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    // dagayn: tests crates/dagayn-tools/src/findings.rs::implicit_caller
    let repo = Repo::new("import-reach", true);
    let specs = |value: i64| {
        format!(
            "class Spec:\n    def __init__(self):\n        self.value = {value}\n\n    def grow(self):\n        self.value += {value}\n\n\nclass Store:\n    def __init__(self):\n        self.items = [{value}]\n\n\ndef make_store():\n    return Store()\n\n\ndef unused():\n    return {value}\n\n\nDEFAULT = Spec()\nSTORE = make_store()\n"
        )
    };
    let lazy = |value: i64| format!("def __getattr__(name):\n    return {value}\n");
    repo.write("specs.py", &specs(1));
    repo.write("lazy/__init__.py", &lazy(1));
    repo.write(
        "test_specs.py",
        "import specs\nimport lazy\n\n\ndef test_default():\n    assert specs.DEFAULT and lazy.anything\n",
    );
    commit_all(&repo, "specs");
    repo.build();
    repo.write("specs.py", &specs(2));
    repo.write("lazy/__init__.py", &lazy(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let mut untested: Vec<&str> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .flat_map(|f| f["targets"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    untested.sort_unstable();
    // Importing `specs` builds `DEFAULT` (`Spec.__init__`) and, through
    // `make_store`, `STORE` (`Store.__init__`); an attribute of `lazy` runs
    // its `__getattr__`. Nothing calls `grow` or `unused`.
    assert_eq!(
        untested,
        ["specs.py::Spec.grow", "specs.py::unused"],
        "{changes}"
    );
}

#[test]
fn a_rust_test_that_uses_a_type_reaches_the_trait_methods_the_runtime_calls() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    // dagayn: tests crates/dagayn-tools/src/findings.rs::rust_trait_impl_type
    let repo = Repo::new("trait-impls", true);
    let rust = |value: i64| {
        format!(
            "pub struct Guard {{\n    pub value: i64,\n}}\n\nimpl Guard {{\n    pub fn new() -> Guard {{\n        Guard {{ value: {value} }}\n    }}\n\n    pub fn grow(&mut self) {{\n        self.value += {value};\n    }}\n}}\n\nimpl Drop for Guard {{\n    fn drop(&mut self) {{\n        self.value -= {value};\n    }}\n}}\n\nimpl PartialEq for Guard {{\n    fn eq(&self, other: &Guard) -> bool {{\n        self.value + {value} == other.value\n    }}\n}}\n\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn guard() {{\n        let _guard = super::Guard::new();\n    }}\n}}\n"
        )
    };
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("src/lib.rs", &rust(1));
    commit_all(&repo, "guard");
    repo.build();
    repo.write("src/lib.rs", &rust(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let mut untested: Vec<&str> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .flat_map(|f| f["targets"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    untested.sort_unstable();
    // Dropping the `Guard` the test builds runs `drop`; `==` and `grow` are
    // calls nothing makes.
    assert_eq!(
        untested,
        ["src/lib.rs::Guard.eq", "src/lib.rs::Guard.grow"],
        "{changes}"
    );
}

#[test]
fn a_rust_test_that_builds_a_derived_default_reaches_the_trait_methods_of_its_type() {
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    // dagayn: tests crates/dagayn-parser/src/rust_lang/mod.rs::rust_collect_type_references
    let repo = Repo::new("derived-default", true);
    let guard = |value: i64| {
        format!(
            "#[derive(Default)]\npub struct Guard {{\n    pub value: i64,\n}}\n\nimpl Guard {{\n    pub fn grow(&mut self) {{\n        self.value += {value};\n    }}\n}}\n\nimpl Drop for Guard {{\n    fn drop(&mut self) {{\n        self.value -= {value};\n    }}\n}}\n"
        )
    };
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "src/lib.rs",
        "mod guard;\n\npub fn make() -> i64 {\n    guard::Guard::default().value\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn make() {\n        assert_eq!(super::make(), 0);\n    }\n}\n",
    );
    repo.write("src/guard.rs", &guard(1));
    commit_all(&repo, "guard");
    repo.build();
    repo.write("src/guard.rs", &guard(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let mut untested: Vec<&str> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .flat_map(|f| f["targets"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    untested.sort_unstable();
    // `Guard::default()` builds a `Guard`, which `make` drops; nothing
    // calls `grow`.
    assert_eq!(untested, ["src/guard.rs::Guard.grow"], "{changes}");
}

#[test]
fn a_test_beyond_four_hops_counts_in_rust_and_typescript_and_not_in_python() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/findings.rs::caller_test_depth
    // dagayn: tests crates/dagayn-tools/src/findings.rs::untested_changes
    let repo = Repo::new("deep-chains", true);
    // `f0` calls `f1` ... `f6`; the test of `f0` is six hops from `f6`.
    let rust = |value: i64| {
        let mut source: String = (0..6)
            .map(|i| format!("pub fn f{i}() -> i64 {{\n    f{}()\n}}\n\n", i + 1))
            .collect();
        source.push_str(&format!("pub fn f6() -> i64 {{\n    {value}\n}}\n\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn chain() {{\n        assert!(super::f0() > 0);\n    }}\n}}\n"));
        source
    };
    let python = |value: i64| {
        let mut source: String = (0..6)
            .map(|i| format!("def f{i}():\n    return f{}()\n\n\n", i + 1))
            .collect();
        source.push_str(&format!("def f6():\n    return {value}\n"));
        source
    };
    let typescript = |value: i64| {
        let mut source: String = (0..6)
            .map(|i| {
                format!(
                    "export function f{i}(): number {{\n  return f{}();\n}}\n\n",
                    i + 1
                )
            })
            .collect();
        source.push_str(&format!(
            "export function f6(): number {{\n  return {value};\n}}\n"
        ));
        source
    };
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("src/lib.rs", &rust(1));
    repo.write("chain.py", &python(1));
    repo.write(
        "test_chain.py",
        "from chain import f0\n\n\ndef test_chain():\n    assert f0()\n",
    );
    repo.write("web/chain.ts", &typescript(1));
    repo.write(
        "web/chain.test.ts",
        "import { expect, test } from \"vitest\";\nimport { f0 } from \"./chain\";\n\ntest(\"chain\", () => {\n  expect(f0()).toBeGreaterThan(0);\n});\n",
    );
    commit_all(&repo, "chains");
    repo.build();
    repo.write("src/lib.rs", &rust(2));
    repo.write("chain.py", &python(2));
    repo.write("web/chain.ts", &typescript(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let untested: Vec<&Value> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .collect();
    assert_eq!(untested.len(), 1, "{changes}");
    assert_eq!(untested[0]["targets"], json!(["chain.py::f6"]), "{changes}");
    assert_eq!(untested[0]["evidence"][1]["caller_depth"], 4, "{changes}");
    let reach = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "src/lib.rs::f6"}),
    );
    assert_eq!(reach["test_reach"]["hops"], 6, "{reach}");
    assert_eq!(reach["test_reach"]["counts_as_tested"], true, "{reach}");
    assert_eq!(reach["test_reach"]["hop_limit"], Value::Null, "{reach}");
    let reach = answer(
        &context,
        "query_graph_tool",
        json!({"pattern": "tests_for", "target": "web/chain.ts::f6"}),
    );
    assert_eq!(reach["test_reach"]["hops"], 6, "{reach}");
    assert_eq!(reach["test_reach"]["counts_as_tested"], true, "{reach}");
    assert_eq!(reach["test_reach"]["hop_limit"], 8, "{reach}");
}

#[test]
fn importing_a_module_reaches_its_top_level_calls_only() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    // dagayn: tests crates/dagayn-tools/src/findings.rs::nearest_test
    // dagayn: tests crates/dagayn-tools/src/findings.rs::runs_on_import
    let repo = Repo::new("import-time-calls", true);
    let tables = |value: i64| {
        format!(
            "def build_table():\n    return [{value}]\n\n\ndef helper():\n    return {value}\n\n\ndef fallback():\n    return {value}\n\n\ndef main():\n    return {value}\n\n\nTABLE = build_table()\nHANDLERS = {{\"a\": lambda: helper()}}\ntry:\n    import json\nexcept ImportError:\n    fallback()\n\nif __name__ == \"__main__\":\n    main()\n"
        )
    };
    repo.write("tables.py", &tables(1));
    repo.write(
        "test_tables.py",
        "import tables\n\n\ndef test_table():\n    assert tables.TABLE\n",
    );
    commit_all(&repo, "tables");
    repo.build();
    repo.write("tables.py", &tables(2));
    commit_all(&repo, "edit");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let changes = answer(&context, "review_tool", json!({}));
    let mut untested: Vec<&str> = changes["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "untested_change")
        .flat_map(|f| f["targets"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    untested.sort_unstable();
    // Importing `tables` runs `build_table()`; a lambda's body, an `except`
    // handler, and the `__main__` block do not run on import.
    assert_eq!(
        untested,
        [
            "tables.py::fallback",
            "tables.py::helper",
            "tables.py::main"
        ],
        "{changes}"
    );
}

/// The `tests_to_run` findings of `review`, as (targets, command) pairs.
fn tests_to_run_of(review: &Value) -> Vec<(Vec<String>, String)> {
    review["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|finding| finding["kind"] == "tests_to_run")
        .map(|finding| {
            let targets = finding["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|target| target.as_str().map(str::to_string))
                .collect();
            (
                targets,
                finding["command"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// Go: `tests_to_run` names the `Test` and `Benchmark` that reach a changed
/// function through helpers of the test file, not the helpers, and runs the
/// benchmark with `-bench`; a `test_*.go` file is production code (#240,
/// #255).
#[test]
fn go_tests_to_run_names_the_tests_behind_helpers() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("go-tests-to-run", true);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    let math = |op: &str| {
        format!(
            "package mathx\n\nfunc Add(a, b int) int {{\n\treturn a {op} b\n}}\n\nfunc Mul(a, b int) int {{\n\treturn a * b {op} 0\n}}\n"
        )
    };
    let helpers = |op: &str| {
        format!(
            "package mathx\n\n// Sum is production code, though its file starts with test_.\nfunc Sum(xs ...int) int {{\n\ttotal := 0\n\tfor _, x := range xs {{\n\t\ttotal = total {op} x\n\t}}\n\treturn total\n}}\n"
        )
    };
    repo.write("mathx/math.go", &math("+"));
    repo.write("mathx/test_helpers.go", &helpers("+"));
    repo.write(
        "mathx/math_test.go",
        "package mathx\n\nimport \"testing\"\n\nfunc checkAdd(t *testing.T) {\n\tif Add(1, 2) != 3 {\n\t\tt.Fatal(\"Add\")\n\t}\n}\n\nfunc TestAdd(t *testing.T) {\n\tcheckAdd(t)\n}\n\nfunc benchMul(b *testing.B) {\n\tfor i := 0; i < b.N; i++ {\n\t\tMul(2, 3)\n\t}\n}\n\nfunc BenchmarkMul(b *testing.B) {\n\tbenchMul(b)\n}\n",
    );
    // A suite method: `suite.Run` calls it by reflection, so no test is seen
    // to reach it, and the package's tests are the command.
    repo.write(
        "mathx/suite_test.go",
        "package mathx\n\ntype mathSuite struct{}\n\nfunc (s *mathSuite) TestMul() {\n\t_ = Mul(2, 3)\n}\n",
    );
    commit_all(&repo, "go");
    repo.build();
    repo.write("mathx/math.go", &math("-"));
    repo.write("mathx/test_helpers.go", &helpers("-"));
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let review = answer(
        &context,
        "review_tool",
        json!({"mode": "changes", "base": "HEAD"}),
    );
    assert_eq!(
        tests_to_run_of(&review),
        vec![
            (
                vec![
                    "mathx/math_test.go::BenchmarkMul".to_string(),
                    "mathx/math_test.go::TestAdd".to_string(),
                ],
                "go test ./mathx -run '^TestAdd$' -bench '^BenchmarkMul$'".to_string(),
            ),
            (
                vec!["mathx/suite_test.go::mathSuite.TestMul".to_string()],
                "go test ./mathx".to_string(),
            ),
        ],
        "{review}"
    );
    let untested: Vec<&Value> = review["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|finding| finding["kind"] == "untested_change")
        .collect();
    assert_eq!(untested.len(), 1, "{review}");
    assert_eq!(untested[0]["qualified_name"], "mathx/test_helpers.go::Sum");
}

/// Rust: a function under `tests/` that the tests call is a helper, though
/// its path makes it a `Test` node; `tests_to_run` names the test calling
/// it, whose `cargo test` filter runs something.
#[test]
fn rust_tests_to_run_names_the_test_behind_an_integration_helper() {
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("rust-tests-to-run", true);
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "src/lib.rs",
        "pub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n",
    );
    repo.write(
        "tests/it.rs",
        "fn helper() -> i64 {\n    demo::add(1, 2)\n}\n\n#[test]\nfn adds() {\n    assert_eq!(helper(), 3);\n}\n",
    );
    commit_all(&repo, "rust");
    repo.build();
    repo.write(
        "src/lib.rs",
        "pub fn add(a: i64, b: i64) -> i64 {\n    b + a\n}\n",
    );
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let review = answer(
        &context,
        "review_tool",
        json!({"mode": "changes", "base": "HEAD"}),
    );
    assert_eq!(
        tests_to_run_of(&review),
        vec![(
            vec!["tests/it.rs::adds".to_string()],
            "cargo test -p demo --test it adds".to_string(),
        )],
        "{review}"
    );
}
