//! Rust call and import edges: `use` paths resolved to module files, typed
//! receivers, calls inside macro arguments, and the forms resolution across
//! files needs (`receiver_type`, `module_file`, `value_reference`).

use super::*;

fn repo(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let mut root = std::env::temp_dir();
    root.push(format!("dagayn-rust-edges-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

fn calls<'a>(edges: &'a [ParsedEdge], source: &str) -> Vec<&'a ParsedEdge> {
    edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.source.ends_with(source))
        .collect()
}

const LIB: &str = "pub mod util;\npub struct Store;\nimpl Store { pub fn open() -> Self { Store } pub fn save(&self) {} }\n";

#[test]
fn use_declarations_resolve_to_module_files() {
    let root = repo(
        "imports",
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/lib.rs", LIB),
            ("src/util/mod.rs", "mod text;\npub use text::*;\n"),
            ("src/util/text.rs", "pub fn node_text() {}\n"),
            (
                "src/main.rs",
                "use app::util::{node_text as text_of, self};\nuse crate::missing::x;\nuse std::collections::HashMap;\n",
            ),
        ],
    );
    let mut parser = RustOwnedParser::new();
    let source = std::fs::read(root.join("src/main.rs")).unwrap();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "src/main.rs", &source);
    let imports: Vec<(&str, &serde_json::Value)> = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| (edge.target.as_str(), &edge.extra))
        .collect();
    let util = imports
        .iter()
        .find(|(target, _)| *target == "src/util/mod.rs")
        .expect("util module file");
    assert_eq!(
        util.1["names"],
        serde_json::json!([["node_text", "text_of"]])
    );
    // The standard library: its crate, with the paths imported from it.
    let std = imports
        .iter()
        .find(|(target, _)| *target == "std")
        .expect("std import");
    assert_eq!(std.1["external"], true);
    assert_eq!(
        std.1["paths"],
        serde_json::json!(["std::collections::HashMap"])
    );

    let util_source = std::fs::read(root.join("src/util/mod.rs")).unwrap();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "src/util/mod.rs", &util_source);
    let re_export = edges
        .iter()
        .find(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "src/util/text.rs")
        .expect("re-export");
    assert_eq!(re_export.extra["re_export"], true);
    assert_eq!(re_export.extra["glob"], true);
}

#[test]
fn call_targets_carry_what_cross_file_resolution_needs() {
    let root = repo(
        "calls",
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/lib.rs", &format!("{LIB}mod run;\n")),
            ("src/util/mod.rs", "pub fn helper() {}\n"),
            (
                "src/run.rs",
                "use crate::util::helper as h;\nuse crate::Store;\nuse std::time::Instant;\n\
fn run(store: &Store, tx: Tx) {\n\
    h();\n\
    crate::util::helper();\n\
    store.save();\n\
    Store::open().save();\n\
    let s = Store::open();\n\
    s.save();\n\
    get_tx().commit();\n\
    Instant::now();\n\
    assert_eq!(compute(1), 2);\n\
    matches!(x, Some(_));\n\
}\n",
            ),
        ],
    );
    let mut parser = RustOwnedParser::new();
    let source = std::fs::read(root.join("src/run.rs")).unwrap();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "src/run.rs", &source);
    let calls = calls(&edges, "::run");
    let find = |target: &str| {
        calls
            .iter()
            .filter(|e| e.target == target)
            .collect::<Vec<_>>()
    };

    // `use ... as h`: the call names the original function.
    assert_eq!(find("helper")[0].extra["alias"], "h");
    // A module path records the module's file.
    assert!(
        find("helper")
            .iter()
            .any(|e| e.extra["module_file"] == "src/util/mod.rs")
    );
    // Typed receivers (a parameter, `Store::open().save()`, `let s = Store::open()`)
    // name the type.
    assert!(
        find("save")
            .iter()
            .all(|e| e.extra["receiver_type"] == "Store")
    );
    assert_eq!(find("save").len(), 3);
    assert_eq!(find("open")[0].extra["receiver_type"], "Store");
    // An untyped receiver never binds by name.
    assert_eq!(find("commit")[0].extra["receiver_unknown"], true);
    // A standard-library type (`use std::time::Instant`) and macro: the
    // `std` crate, with the name as written.
    let std_symbols = find("std")
        .iter()
        .map(|e| e.extra["external_symbol"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(std_symbols.contains(&"Instant::now"), "{std_symbols:?}");
    // Calls in macro arguments; macros are their own namespace.
    assert_eq!(find("compute")[0].extra["in_macro"], true);
    assert!(std_symbols.contains(&"matches!"), "{std_symbols:?}");
    assert!(find("matches").is_empty());
}

#[test]
fn value_references_skip_locals_and_bare_calls_stay_in_scope() {
    let source = br#"
fn edge_from_row() {}
struct Tier;
impl Tier { fn node(&self) {} }
mod tests { pub fn node() {} }
fn run(node: u8, rows: Rows) {
    rows.map(edge_from_row);
    consume(node);
    node();
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("src/lib.rs", source);
    let references: Vec<&str> = edges
        .iter()
        .filter(|edge| edge.kind == "REFERENCES" && edge.source == "src/lib.rs::run")
        .map(|edge| edge.target.as_str())
        .collect();
    assert_eq!(references, vec!["src/lib.rs::edge_from_row"]);
    // A bare `node()` reaches neither `Tier.node` (a method) nor `tests.node`.
    let node_call = edges
        .iter()
        .find(|edge| {
            edge.kind == "CALLS"
                && edge.source == "src/lib.rs::run"
                && edge.target.ends_with("node")
        })
        .expect("node call");
    assert_eq!(node_call.target, "node");
}

#[test]
fn type_references_reach_imported_and_macro_named_types() {
    let root = repo(
        "types",
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/lib.rs", "pub mod types;\nmod build;\n"),
            (
                "src/types.rs",
                "pub struct ParsedNode;\npub enum NodeKind { File }\npub struct Edge;\n",
            ),
            (
                "src/build.rs",
                "use crate::types::ParsedNode;\nuse super::types;\n\
fn build(edge: types::Edge) -> Vec<ParsedNode> {\n\
    let kind = crate::types::NodeKind::File;\n\
    vec![ParsedNode]\n\
}\n",
            ),
        ],
    );
    let mut parser = RustOwnedParser::new();
    let source = std::fs::read(root.join("src/build.rs")).unwrap();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "src/build.rs", &source);
    let mut references: Vec<(&str, &serde_json::Value)> = edges
        .iter()
        .filter(|edge| edge.kind == "REFERENCES" && edge.source == "src/build.rs::build")
        .map(|edge| (edge.target.as_str(), &edge.extra["module_file"]))
        .collect();
    references.sort_by_key(|(target, _)| *target);
    let types_rs = serde_json::json!("src/types.rs");
    assert_eq!(
        references,
        vec![
            ("Edge", &types_rs),
            ("NodeKind", &types_rs),
            ("ParsedNode", &types_rs)
        ]
    );
}

#[test]
fn supertraits_are_inherits() {
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file(
        "src/lib.rs",
        b"trait A: B + fmt::Display + 'static {}\ntrait C<T>: Into<T> {}\n",
    );
    let mut bases: Vec<(&str, &str)> = edges
        .iter()
        .filter(|edge| edge.kind == "INHERITS")
        .map(|edge| (edge.source.as_str(), edge.target.as_str()))
        .collect();
    bases.sort();
    assert_eq!(
        bases,
        vec![
            ("src/lib.rs::A", "B"),
            ("src/lib.rs::A", "Display"),
            ("src/lib.rs::C", "Into"),
        ]
    );
}

#[test]
fn standard_library_calls_target_their_crate() {
    let source = br#"use std::path::Path;
use std::collections::{HashMap, HashSet};

fn join(a: &str, b: &str) -> String { String::new() }

fn exists(root: &Path, rel: &str) -> bool {
    root.join(rel).is_file()
}

fn normalize(parts: Vec<&str>) -> String {
    let mut out = Vec::new();
    out.push(join("a", "b"));
    parts.join("/")
}

fn misc() {
    let m = HashMap::new();
    Some(1);
    drop(m);
    std::fs::read("p");
    core::mem::take(&mut 1);
    u32::from(1u8);
    format!("x");
    shout!("x");
}

macro_rules! shout { ($x:expr) => {}; }
"#;
    let (_, edges) = parse_rust("src/lib.rs", source);
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| {
            (
                edge.source.rsplit("::").next().unwrap_or_default(),
                edge.target.as_str(),
                edge.extra["external_symbol"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        ("exists", "std", "Path::join"),
        ("normalize", "std", "Vec::new"),
        ("normalize", "std", "Vec::push"),
        ("normalize", "src/lib.rs::join", ""),
        ("normalize", "std", "Vec::join"),
        ("misc", "std", "HashMap::new"),
        ("misc", "std", "Some"),
        ("misc", "std", "drop"),
        ("misc", "std", "std::fs::read"),
        ("misc", "core", "core::mem::take"),
        ("misc", "std", "u32::from"),
        ("misc", "std", "format!"),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    // A path, a `use`, or the prelude (no glob in the file) is certain; a
    // method of a variable typed by its binding is likely.
    let tier = |symbol: &str| {
        edges
            .iter()
            .find(|edge| edge.extra["external_symbol"] == symbol)
            .map(|edge| edge.extra["confidence_tier"].clone())
            .unwrap_or_else(|| panic!("no call to {symbol}"))
    };
    for symbol in [
        "std::fs::read",
        "HashMap::new",
        "Vec::new",
        "Some",
        "format!",
    ] {
        assert_eq!(tier(symbol), "HIGH", "{symbol}");
    }
    assert_eq!(tier("Vec::join"), "MEDIUM");
    assert_eq!(tier("Path::join"), "MEDIUM");
    // Only `join("a", "b")` is this file's `join`.
    assert_eq!(
        calls
            .iter()
            .filter(|(_, target, _)| *target == "src/lib.rs::join")
            .count(),
        1,
        "{calls:?}"
    );
    // A macro this file defines is not `std`'s.
    assert!(
        calls.iter().any(|(_, target, _)| *target == "shout!"),
        "{calls:?}"
    );

    let std_imports = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "std")
        .map(|edge| edge.extra["paths"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        std_imports,
        vec![
            serde_json::json!(["std::path::Path"]),
            serde_json::json!(["std::collections::HashMap", "std::collections::HashSet"]),
        ]
    );
}

#[test]
fn calls_into_dependencies_target_their_crate() {
    // `Cargo.toml` names the crates the package depends on; a path through
    // one, or through a name a `use` of one brought in, is a call into it.
    // `other` is a module of this crate, not a dependency.
    let root = repo(
        "dependencies",
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"app\"\n\n[dependencies]\nserde_json = \"1\"\ntree-sitter.workspace = true\n\n[dev-dependencies.pretty_assertions]\nversion = \"1\"\n",
            ),
            ("src/lib.rs", "mod other;\nmod run;\n"),
            ("src/other.rs", "pub fn go() {}\n"),
            (
                "src/run.rs",
                "use tree_sitter::Node;\nuse serde_json::{json, Value};\n\
fn run(node: Node) {\n\
    node.kind();\n\
    serde_json::to_string(&1);\n\
    json!({});\n\
    crate::other::go();\n\
    pretty_assertions::assert_eq!(1, 1);\n\
}\n",
            ),
        ],
    );
    let mut parser = RustOwnedParser::new();
    let source = std::fs::read(root.join("src/run.rs")).unwrap();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "src/run.rs", &source);
    let external = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.extra["external"] == true)
        .map(|edge| {
            (
                edge.target.as_str(),
                edge.extra["external_symbol"].as_str().unwrap_or_default(),
                edge.extra["confidence_tier"].as_str().unwrap_or_default(),
                edge.extra["stdlib"] == true,
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        ("tree_sitter", "Node::kind", "MEDIUM", false),
        ("serde_json", "serde_json::to_string", "HIGH", false),
        ("serde_json", "json!", "HIGH", false),
        (
            "pretty_assertions",
            "pretty_assertions::assert_eq!",
            "HIGH",
            false,
        ),
    ] {
        assert!(
            external.contains(&expected),
            "{expected:?} not in {external:?}"
        );
    }
    assert!(
        edges.iter().any(|edge| edge.kind == "CALLS"
            && edge.target == "go"
            && edge.extra["external"] != true),
        "{edges:?}"
    );
    let imports = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| (edge.target.as_str(), edge.extra["paths"].clone()))
        .collect::<Vec<_>>();
    assert!(
        imports.contains(&(
            "serde_json",
            serde_json::json!(["serde_json::json", "serde_json::Value"])
        )),
        "{imports:?}"
    );
    assert!(
        imports.contains(&("tree_sitter", serde_json::json!(["tree_sitter::Node"]))),
        "{imports:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn workspace_crates_are_not_dependencies() {
    // `app` depends on `core` by path: a crate of the repository, whose
    // calls resolution across files binds, never an external package.
    let root = repo(
        "workspace-dependency",
        &[
            ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
            ("crates/core/Cargo.toml", "[package]\nname = \"core-lib\"\n"),
            ("crates/core/src/lib.rs", "pub fn run() {}\n"),
            (
                "crates/app/Cargo.toml",
                "[package]\nname = \"app\"\n\n[dependencies]\ncore-lib = { path = \"../core\" }\nregex = \"1\"\n",
            ),
            (
                "crates/app/src/lib.rs",
                "use core_lib::run;\nfn go() {\n    run();\n    core_lib::run();\n    regex::Regex::new(\"x\");\n}\n",
            ),
        ],
    );
    let mut parser = RustOwnedParser::new();
    let source = std::fs::read(root.join("crates/app/src/lib.rs")).unwrap();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "crates/app/src/lib.rs", &source);
    let external = edges
        .iter()
        .filter(|edge| edge.extra["external"] == true)
        .map(|edge| edge.target.as_str())
        .collect::<Vec<_>>();
    assert!(external.contains(&"regex"), "{edges:?}");
    assert!(!external.contains(&"core_lib"), "{edges:?}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn types_record_what_they_dereference_to() {
    let source = b"use std::ops::Deref;\npub struct FilePath(String);\nimpl Deref for FilePath {\n    type Target = str;\n    fn deref(&self) -> &str { &self.0 }\n}\npub struct Names(Vec<String>);\nimpl std::ops::Deref for Names {\n    type Target = [String];\n    fn deref(&self) -> &[String] { &self.0 }\n}\n";
    let (nodes, _) = parse_rust("src/types.rs", source);
    let target = |name: &str| {
        nodes
            .iter()
            .find(|node| node.kind == "Class" && node.name == name)
            .map(|node| node.extra["deref_target"].clone())
            .unwrap_or_else(|| panic!("no {name}"))
    };
    assert_eq!(target("FilePath"), "str");
    assert_eq!(target("Names"), "slice");
}

#[test]
fn destructured_bindings_take_no_type_from_the_value() {
    // `let Some(callee) = node.child()`: `callee` is not an `Option`.
    let source = b"fn run(node: Node) {\n    let Some(callee) = Some(node) else { return };\n    callee.kind();\n    let value: Option<i32> = None;\n    value.is_some();\n}\n";
    let (_, edges) = parse_rust("src/lib.rs", source);
    let symbols = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .filter_map(|edge| edge.extra["external_symbol"].as_str())
        .collect::<Vec<_>>();
    assert!(!symbols.contains(&"Option::kind"), "{symbols:?}");
    assert!(symbols.contains(&"Option::is_some"), "{symbols:?}");
}

#[test]
fn types_written_with_their_crate_type_their_variables() {
    // `node: tree_sitter::Node<'_>` without a `use`: `node.kind()` is a
    // call into `tree_sitter`.
    let root = repo(
        "qualified-types",
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"app\"\n\n[dependencies]\ntree-sitter.workspace = true\n",
            ),
            ("src/lib.rs", "mod run;\n"),
            (
                "src/run.rs",
                "fn run(node: tree_sitter::Node<'_>, path: &std::path::Path) {\n    node.kind();\n    path.exists();\n}\n",
            ),
        ],
    );
    let mut parser = RustOwnedParser::new();
    let source = std::fs::read(root.join("src/run.rs")).unwrap();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "src/run.rs", &source);
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| {
            (
                edge.target.as_str(),
                edge.extra["external_symbol"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    assert!(calls.contains(&("tree_sitter", "Node::kind")), "{calls:?}");
    assert!(calls.contains(&("std", "Path::exists")), "{calls:?}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn receivers_record_the_call_they_came_from() {
    let source = b"fn run(p: &str) {\n    open_store(p)?.save();\n    let conn = connect(p).unwrap();\n    conn.execute();\n    make().finish();\n}\nfn open_store(p: &str) -> Result<Store> { todo!() }\n";
    let (nodes, edges) = parse_rust("src/lib.rs", source);
    let from = |method: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == method)
            .map(|edge| edge.extra["receiver_from"].clone())
            .unwrap_or_else(|| panic!("no {method} in {edges:?}"))
    };
    assert_eq!(
        from("save"),
        serde_json::json!({"call": "open_store", "line": 2, "unwrap": true})
    );
    assert_eq!(
        from("execute"),
        serde_json::json!({"call": "connect", "line": 3, "unwrap": true})
    );
    assert_eq!(
        from("finish"),
        serde_json::json!({"call": "make", "line": 5, "unwrap": false})
    );
    let open = nodes
        .iter()
        .find(|node| node.name == "open_store")
        .expect("open_store");
    assert_eq!(open.return_type.as_deref(), Some("Result<Store>"));
}

#[test]
fn calls_in_macro_arguments_record_the_call_their_receiver_came_from() {
    let source = b"fn run(repo: &str) {\n    let tree = parse(repo);\n    assert!(!tree.root_node().has_error());\n    println!(\"{}\", make(repo).display());\n}\n";
    let (_, edges) = parse_rust("src/lib.rs", source);
    let from = |method: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == method)
            .map(|edge| edge.extra["receiver_from"].clone())
            .unwrap_or_else(|| panic!("no {method} in {edges:?}"))
    };
    assert_eq!(
        from("root_node"),
        serde_json::json!({"call": "parse", "line": 2, "unwrap": false})
    );
    assert_eq!(
        from("has_error"),
        serde_json::json!({"call": "root_node", "line": 3, "unwrap": false})
    );
    assert_eq!(
        from("display"),
        serde_json::json!({"call": "make", "line": 4, "unwrap": false})
    );
}

#[test]
fn repeated_methods_in_a_chain_take_the_receiver_before_them() {
    let source = b"fn run() {\n    Build::new().flag(1).flag(2).file(3);\n}\n";
    let (_, edges) = parse_rust("build.rs", source);
    let from = |method: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == method)
            .map(|edge| edge.extra["receiver_from"]["call"].clone())
            .unwrap_or_else(|| panic!("no {method} in {edges:?}"))
    };
    assert_eq!(from("flag"), "new");
    assert_eq!(from("file"), "flag");
}

#[test]
fn closures_bound_by_let_are_functions_of_their_body() {
    let source = b"fn helper() {}\nfn run(items: Vec<i32>) {\n    let call = |x: i32| {\n        helper();\n    };\n    call(1);\n    let twice = move |x| call(x);\n    twice(2);\n}\n";
    let (nodes, edges) = parse_rust("src/lib.rs", source);
    let closure = nodes
        .iter()
        .find(|node| node.name == "call")
        .expect("closure node");
    assert_eq!(closure.kind, "Function");
    assert_eq!(closure.parent_name.as_deref(), Some("run"));
    assert_eq!(closure.extra["rust_kind"], "closure");
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| (edge.source.as_str(), edge.target.as_str()))
        .collect::<Vec<_>>();
    for expected in [
        ("src/lib.rs::run", "src/lib.rs::run.call"),
        ("src/lib.rs::run.call", "src/lib.rs::helper"),
        ("src/lib.rs::run", "src/lib.rs::run.twice"),
        ("src/lib.rs::run.twice", "src/lib.rs::run.call"),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
}

#[test]
fn enums_record_their_variants() {
    let source =
        b"pub enum Receiver {\n    Local(String),\n    Unknown { origin: u8 },\n    Known,\n}\n";
    let (nodes, _) = parse_rust("src/lib.rs", source);
    let receiver = nodes
        .iter()
        .find(|node| node.name == "Receiver")
        .expect("enum");
    assert_eq!(
        receiver.extra["variants"],
        serde_json::json!(["Local", "Unknown", "Known"])
    );
}
