use dagayn_build::db_path_for_build;
use dagayn_graph::GraphStore;
use dagayn_tools::Context;
use serde_json::{Value, json};

use crate::common::{Repo, answer, commit_all, declines};

#[test]
fn architecture_metrics_follow_the_requested_view() {
    // dagayn: tests crates/dagayn-tools/src/arch_tool.rs::architecture
    // dagayn: tests crates/dagayn-tools/src/refactor.rs::refactor
    let repo = Repo::new("arch", true);
    repo.write(
        "pkg/core.py",
        "from app import main\n\n\nclass Base:\n    pass\n",
    );
    repo.write(
        "app.py",
        "from pkg.core import Base\n\n\ndef main():\n    pass\n",
    );
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let arch = |arguments: Value| answer(&context, "architecture_analysis_tool", arguments);
    let overview = arch(json!({}));
    assert!(overview.get("called_subtool").is_none());
    assert!(overview["units"].is_array());
    assert!(overview.get("architecture_health").is_none());
    // `app.py` and `pkg/core.py` import each other at module level.
    let cycle = &overview["findings"][0];
    assert_eq!(cycle["kind"], "import_cycle");
    assert_eq!(cycle["targets"], json!(["app.py", "pkg/core.py"]));
    assert_eq!(cycle["cut"].as_array().map(Vec::len), Some(1));
    // verbose is the map with the graph-wide answerability; the community
    // health report is gone.
    let verbose = arch(json!({"detail_level": "verbose"}));
    assert_eq!(verbose["findings"], overview["findings"]);
    assert!(verbose["answerability"].is_object());
    for gone in [
        "architecture_health",
        "stable_component_policy",
        "communities",
        "cross_community_coupling",
        "warnings",
        "_hints",
        "next_tool_suggestions",
        "next_action",
        "next_drill_downs",
        "deprecated_fields",
    ] {
        assert!(verbose.get(gone).is_none(), "{gone}");
    }
    for arguments in [
        // Removed after measurement (docs/plans/OPEN-MEASUREMENTS.md#communities).
        json!({"mode": "communities"}),
        json!({"mode": "community", "community_id": 1}),
        json!({"top_n": 5}),
        // Removed in favour of the overview's map and findings.
        json!({"mode": "hubs"}),
        json!({"mode": "adp_violations"}),
        json!({"mode": "sdp_metrics"}),
        json!({"mode": "sdp_violations"}),
        json!({"mode": "sap_metrics"}),
        json!({"mode": "sap_violations"}),
        json!({"dependency_profile": "implementation"}),
        json!({"mode": "import_cycles", "unit_filter": "pkg"}),
    ] {
        assert!(
            declines(&context, "architecture_analysis_tool", arguments.clone()),
            "{arguments}"
        );
    }
}

#[test]
fn import_cycles_lists_every_cycle_between_directories() {
    // dagayn: tests crates/dagayn-tools/src/cycles.rs::import_cycles
    let repo = Repo::new("arch-cycles", true);
    repo.write(
        "a/one.py",
        "from b.two import g\n\n\ndef f():\n    return g()\n",
    );
    repo.write(
        "b/two.py",
        "def g():\n    from c.three import h\n    return h()\n",
    );
    repo.write(
        "c/three.py",
        "from a.one import f\n\n\ndef h():\n    return 1\n",
    );
    repo.write("b/back.py", "from a.one import f\n");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let arch = |arguments: Value| answer(&context, "architecture_analysis_tool", arguments);
    let cycles = arch(json!({"mode": "import_cycles", "scope_kind": "directory"}));
    assert_eq!(
        cycles["cycles"],
        json!([["a", "b"], ["a", "b", "c"]]),
        "{cycles}"
    );
    assert_eq!(cycles["cycle_count"], 2, "{cycles}");
    assert_eq!(cycles["results_complete"], true, "{cycles}");
    assert!(
        cycles["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("This is every simple cycle")),
        "{cycles}"
    );

    // standard names one import behind each hop.
    let standard = arch(json!({
        "mode": "import_cycles",
        "scope_kind": "directory",
        "unit_filter": ["a", "b"],
        "detail_level": "standard",
    }));
    assert_eq!(
        standard["cycles"],
        json!([{"scopes": ["a", "b"], "imports": [
            {"from": "a/one.py", "to": "b/two.py", "line": 1, "imports": 1},
            {"from": "b/back.py", "to": "a/one.py", "line": 1, "imports": 1},
        ]}]),
        "{standard}"
    );

    let files = arch(json!({"mode": "import_cycles", "scope_kind": "file"}));
    assert_eq!(
        files["cycles"],
        json!([["a/one.py", "b/two.py", "c/three.py"]]),
        "{files}"
    );
    assert!(declines(
        &context,
        "architecture_analysis_tool",
        json!({"mode": "import_cycles", "import_kind": 3})
    ));
    let bad = arch(json!({"mode": "import_cycles", "import_kind": "anchor"}));
    assert_eq!(bad["status"], "error", "{bad}");
}

/// A Cargo workspace in layers: `kernel` (used by `feature` and `app`),
/// `feature`, `app`, and `plugins`, which depends on two leaf crates.
fn layered_workspace(repo: &Repo) {
    let crate_manifest = |name: &str, deps: &[&str]| {
        let mut text =
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        if !deps.is_empty() {
            text.push_str("\n[dependencies]\n");
            for dep in deps {
                text.push_str(&format!("{dep} = {{ path = \"../{dep}\" }}\n"));
            }
        }
        text
    };
    repo.write(".gitignore", ".dagayn/\n");
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"kernel\", \"feature\", \"app\", \"plugins\", \"extras\", \"thirdparty\"]\n",
    );
    for (name, deps) in [
        ("kernel", &[][..]),
        ("feature", &["kernel"][..]),
        ("app", &["feature", "kernel"][..]),
        ("plugins", &["extras", "thirdparty"][..]),
        ("extras", &[][..]),
        ("thirdparty", &[][..]),
    ] {
        repo.write(&format!("{name}/Cargo.toml"), &crate_manifest(name, deps));
    }
    repo.write("kernel/src/lib.rs", "pub fn base() -> i64 {\n    1\n}\n");
    repo.write(
        "feature/src/lib.rs",
        "use kernel::base;\n\npub fn feature() -> i64 {\n    base() + 1\n}\n",
    );
    repo.write(
        "app/src/lib.rs",
        "use feature::feature;\nuse kernel::base;\n\npub fn run() -> i64 {\n    feature() + base()\n}\n",
    );
    repo.write(
        "plugins/src/lib.rs",
        "use extras::extra;\nuse thirdparty::vendored;\n\npub fn plugin() -> i64 {\n    extra() + vendored()\n}\n",
    );
    repo.write("extras/src/lib.rs", "pub fn extra() -> i64 {\n    2\n}\n");
    repo.write(
        "thirdparty/src/lib.rs",
        "pub fn vendored() -> i64 {\n    3\n}\n",
    );
}

/// A TypeScript import of a workspace package by name (`@ws/a`) reaches the
/// package's files: its calls, type annotations, and `implements` are edges
/// to real nodes, and the architecture map weighs the dependency.
#[test]
fn workspace_package_imports_reach_the_package_files() {
    // dagayn: tests crates/dagayn-tools/src/arch_tool.rs::architecture
    let repo = Repo::new("ts-workspace", false);
    repo.write("pnpm-workspace.yaml", "packages:\n  - packages/*\n");
    repo.write("package.json", r#"{ "name": "root", "private": true }"#);
    repo.write(
        "packages/a/package.json",
        r#"{ "name": "@ws/a", "exports": { ".": "./src/index.ts" } }"#,
    );
    repo.write(
        "packages/a/src/index.ts",
        "export * from \"./greet\";\nexport type { Greeter } from \"./types\";\n",
    );
    repo.write(
        "packages/a/src/greet.ts",
        "export function greet(name: string): string {\n  return name;\n}\n",
    );
    repo.write(
        "packages/a/src/types.ts",
        "export interface Greeter {\n  hi(): string;\n}\n",
    );
    repo.write(
        "packages/b/package.json",
        r#"{ "name": "@ws/b", "dependencies": { "@ws/a": "workspace:*" } }"#,
    );
    repo.write(
        "packages/b/src/main.ts",
        "import { greet } from \"@ws/a\";\nimport type { Greeter } from \"@ws/a\";\n\n\
         export class Loud implements Greeter {\n  hi(): string {\n    return greet(\"hi\");\n  }\n}\n\n\
         export function run(g: Greeter): string {\n  return g.hi();\n}\n",
    );
    repo.build();

    let store = GraphStore::open(db_path_for_build(&repo.0).expect("db")).expect("store");
    let edges = store.get_all_edges().expect("edges");
    for (kind, source, target) in [
        (
            "IMPORTS_FROM",
            "packages/b/src/main.ts",
            "packages/a/src/index.ts",
        ),
        (
            "CALLS",
            "packages/b/src/main.ts::Loud.hi",
            "packages/a/src/greet.ts::greet",
        ),
        (
            "REFERENCES",
            "packages/b/src/main.ts::run",
            "packages/a/src/types.ts::Greeter",
        ),
        (
            "IMPLEMENTS",
            "packages/b/src/main.ts::Loud",
            "packages/a/src/types.ts::Greeter",
        ),
    ] {
        assert!(
            edges.iter().any(|edge| edge.kind == kind
                && edge.source_qualified == source
                && edge.target_qualified == target),
            "{kind} {source} -> {target}"
        );
    }
    assert!(
        !edges
            .iter()
            .any(|edge| edge.target_qualified.starts_with("@ws/")),
        "no edge is left on the package name"
    );

    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let overview = answer(&context, "architecture_analysis_tool", json!({}));
    let edge = overview["unit_edges"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|edge| edge["from"] == "@ws/b" && edge["to"] == "@ws/a")
        .unwrap_or_else(|| panic!("{}", overview["unit_edges"]));
    for kind in ["calls", "imports", "references", "inherits"] {
        assert!(edge[kind].as_u64().unwrap_or(0) >= 1, "{kind}: {edge}");
    }
}

/// A Rust file compiled only under test (`#[cfg(test)] mod helpers_tests;`)
/// is test code: a crate that uses another only there has no production
/// dependency on it, and the map draws no unit edge between them.
#[test]
fn a_dependency_only_cfg_test_files_use_is_no_unit_edge() {
    // dagayn: tests crates/dagayn-tools/src/arch_tool.rs::architecture
    let repo = Repo::new("cfg-test-files", false);
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"kernel\", \"app\", \"tool\"]\n",
    );
    repo.write("kernel/Cargo.toml", "[package]\nname = \"kernel\"\n");
    repo.write("kernel/src/lib.rs", "pub fn base() -> i64 {\n    1\n}\n");
    // As dagayn-graph has dagayn-parser: a dev-dependency, which the
    // manifest does not declare as a unit edge.
    for (name, section) in [("app", "dev-dependencies"), ("tool", "dependencies")] {
        repo.write(
            &format!("{name}/Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\n\n[{section}]\nkernel = {{ path = \"../kernel\" }}\n"
            ),
        );
    }
    repo.write(
        "app/src/lib.rs",
        "pub fn run() -> i64 {\n    2\n}\n\n#[cfg(test)]\nmod helpers_tests;\n",
    );
    repo.write(
        "app/src/helpers_tests.rs",
        "use kernel::base;\n\npub struct Harness;\n\nfn expected() -> i64 {\n    base() + 1\n}\n\n#[test]\nfn runs() {\n    assert_eq!(crate::run(), expected());\n}\n",
    );
    repo.write(
        "tool/src/lib.rs",
        "use kernel::base;\n\npub fn tool() -> i64 {\n    base()\n}\n",
    );
    repo.build();

    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let overview = answer(&context, "architecture_analysis_tool", json!({}));
    let edges: Vec<(&str, &str)> = overview["unit_edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|edge| Some((edge["from"].as_str()?, edge["to"].as_str()?)))
        .collect();
    assert!(edges.contains(&("tool", "kernel")), "{edges:?}");
    assert!(
        !edges.contains(&("app", "kernel")),
        "{}",
        overview["unit_edges"]
    );
}

/// docs/plans/STABILITY-FINDING-TARGET.md: the overview names every unit
/// that depends on a less stable one; a review names the ones the change
/// introduced, and not a dependency that predates it.
#[test]
fn unstable_dependencies_are_found_where_they_are_introduced() {
    // dagayn: tests crates/dagayn-tools/src/arch_tool.rs::architecture
    // dagayn: tests crates/dagayn-tools/src/review.rs::review
    let repo = Repo::new("stability", true);
    layered_workspace(&repo);
    commit_all(&repo, "layers");
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let unstable = |reply: &Value| -> Vec<Value> {
        reply["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|finding| finding["kind"] == "unstable_dependency")
            .cloned()
            .collect()
    };

    // Layered as SDP asks: nothing to report.
    let overview = answer(&context, "architecture_analysis_tool", json!({}));
    assert!(unstable(&overview).is_empty(), "{}", overview["findings"]);

    // An unstable crate starting to use another is no finding either.
    repo.write(
        "app/src/lib.rs",
        "use feature::feature;\nuse kernel::base;\nuse plugins::plugin;\n\npub fn run() -> i64 {\n    feature() + base() + plugin()\n}\n",
    );
    repo.build();
    let review = answer(&context, "review_tool", json!({"base": "HEAD"}));
    assert!(unstable(&review).is_empty(), "{}", review["findings"]);
    commit_all(&repo, "app uses plugins");

    // The stable kernel starts using plugins: the review and the overview
    // both name it, at the import.
    repo.write(
        "kernel/src/lib.rs",
        "use plugins::plugin;\n\npub fn base() -> i64 {\n    1 + plugin()\n}\n",
    );
    repo.build();
    let review = answer(&context, "review_tool", json!({"base": "HEAD"}));
    let found = unstable(&review);
    assert_eq!(found.len(), 1, "{}", review["findings"]);
    let finding = &found[0];
    assert_eq!(finding["targets"], json!(["kernel", "plugins"]));
    assert_eq!(finding["file"], "kernel/src/lib.rs");
    assert_eq!(finding["line"], 1);
    let evidence = &finding["evidence"];
    assert!(
        evidence["target"]["instability"].as_f64() > evidence["source"]["instability"].as_f64(),
        "{evidence}"
    );
    assert!(evidence["delta"].as_f64() > Some(0.1), "{evidence}");
    assert!(
        review["next"].as_array().is_some_and(|next| next
            .iter()
            .any(|call| call["args"]["target"] == "kernel/src/lib.rs")),
        "{}",
        review["next"]
    );
    let overview = answer(&context, "architecture_analysis_tool", json!({}));
    assert_eq!(unstable(&overview).len(), 1, "{}", overview["findings"]);

    // Once committed, editing another line of kernel does not introduce it
    // again; the overview still reports it.
    commit_all(&repo, "kernel uses plugins");
    repo.write(
        "kernel/src/lib.rs",
        "use plugins::plugin;\n\npub fn base() -> i64 {\n    2 + plugin()\n}\n",
    );
    repo.build();
    let review = answer(&context, "review_tool", json!({"base": "HEAD"}));
    assert!(unstable(&review).is_empty(), "{}", review["findings"]);
    let overview = answer(&context, "architecture_analysis_tool", json!({}));
    assert_eq!(unstable(&overview).len(), 1, "{}", overview["findings"]);
}

/// docs/SAP-METRICS.md#which-types-count-toward-nt-and-na: a struct counts
/// toward a unit's types, so a crate of one trait and one struct is half
/// abstract, not wholly.
#[test]
fn abstractness_counts_structs_and_enums_as_types() {
    // dagayn: tests crates/dagayn-tools/src/architecture.rs::sap_metrics
    let repo = Repo::new("sap-structs", true);
    layered_workspace(&repo);
    repo.write(
        "kernel/src/lib.rs",
        "use plugins::plugin;\n\npub fn base() -> i64 {\n    1 + plugin()\n}\n",
    );
    repo.write(
        "plugins/src/lib.rs",
        "use extras::extra;\nuse thirdparty::vendored;\n\npub trait Plugin {\n    fn run(&self) -> i64;\n}\n\npub struct Builtin;\n\npub enum Kind {\n    Fast,\n    Slow,\n}\n\npub fn plugin() -> i64 {\n    extra() + vendored()\n}\n",
    );
    repo.build();
    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let overview = answer(&context, "architecture_analysis_tool", json!({}));
    let finding = overview["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|finding| finding["kind"] == "unstable_dependency")
        .unwrap_or_else(|| panic!("{}", overview["findings"]));
    let sap = &finding["evidence"]["target"]["sap"];
    assert_eq!(sap["abstractness"].as_f64(), Some(0.3333), "{sap}");
}

/// A Go package is a unit, named by its import path; an import of a package
/// of another module of the repository is a declared dependency when the
/// importing module's `go.mod` requires that module.
#[test]
fn go_packages_are_units_and_required_modules_are_declared() {
    // dagayn: tests crates/dagayn-tools/src/arch_tool.rs::architecture
    let repo = Repo::new("go-package-units", false);
    repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
    repo.write(
        "mathx/math.go",
        "package mathx\n\n// Add adds.\nfunc Add(a, b int) int {\n\treturn a + b\n}\n",
    );
    repo.write(
        "mathx/math_test.go",
        "package mathx_test\n\nimport (\n\t\"testing\"\n\n\t\"example.com/demo/report\"\n)\n\n\
         func TestLine(t *testing.T) {\n\t_ = report.Line(1)\n}\n",
    );
    repo.write(
        "report/report.go",
        "package report\n\nimport \"example.com/demo/mathx\"\n\n\
         // Line formats a sum.\nfunc Line(a int) int {\n\treturn mathx.Add(a, a)\n}\n",
    );
    repo.write(
        "tools/go.mod",
        "module example.com/demo/tools\n\ngo 1.22\n\nrequire example.com/demo v0.0.0\n\n\
         replace example.com/demo => ../\n",
    );
    repo.write(
        "tools/gen/gen.go",
        "package main\n\nimport \"example.com/demo/report\"\n\nfunc main() {\n\t_ = report.Line(2)\n}\n",
    );
    repo.write("other/go.mod", "module example.com/other\n\ngo 1.22\n");
    repo.write(
        "other/use/use.go",
        "package use\n\nimport \"example.com/demo/mathx\"\n\nfunc Use() int {\n\treturn mathx.Add(1, 2)\n}\n",
    );
    repo.build();

    let context = Context {
        runtime: Some(json!({})),
        ..repo.context()
    };
    let overview = answer(&context, "architecture_analysis_tool", json!({}));
    let mut units: Vec<(&str, &str)> = overview["units"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|unit| Some((unit["name"].as_str()?, unit["kind"].as_str()?)))
        .collect();
    units.sort_unstable();
    // The root holds no Go package; the helper's app.py is the module's.
    assert_eq!(
        units,
        [
            ("example.com/demo", "go_module"),
            ("example.com/demo/mathx", "go_package"),
            ("example.com/demo/report", "go_package"),
            ("example.com/demo/tools/gen", "go_package"),
            ("example.com/other/use", "go_package"),
        ],
        "{}",
        overview["units"]
    );
    let edges: Vec<(&str, &str, bool)> = overview["unit_edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|edge| {
            Some((
                edge["from"].as_str()?,
                edge["to"].as_str()?,
                edge["declared"].as_bool().unwrap_or(false),
            ))
        })
        .collect();
    assert!(
        edges.contains(&("example.com/demo/report", "example.com/demo/mathx", false)),
        "{edges:?}"
    );
    assert!(
        edges.contains(&(
            "example.com/demo/tools/gen",
            "example.com/demo/report",
            true
        )),
        "{edges:?}"
    );
    // The external test package's import is no dependency of the package,
    // and closes no cycle between packages.
    assert!(
        !edges
            .iter()
            .any(|(from, _, _)| *from == "example.com/demo/mathx"),
        "{edges:?}"
    );
    for scope_kind in ["package", "directory"] {
        let cycles = answer(
            &context,
            "architecture_analysis_tool",
            json!({"mode": "import_cycles", "scope_kind": scope_kind}),
        );
        assert_eq!(cycles["cycle_count"], 0, "{scope_kind}: {cycles}");
    }
    // example.com/other requires nothing of this repository: the call still
    // binds, but no manifest declares the dependency.
    assert!(
        edges.contains(&("example.com/other/use", "example.com/demo/mathx", false)),
        "{edges:?}"
    );
}
