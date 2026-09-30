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
    // Not in this repository: kept as written.
    assert!(
        imports
            .iter()
            .any(|(target, _)| *target == "std::collections::HashMap")
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
    // A type of another crate is left as written.
    assert_eq!(find("Instant::now").len(), 1);
    // Calls in macro arguments; macros are their own namespace.
    assert_eq!(find("compute")[0].extra["in_macro"], true);
    assert_eq!(find("matches!").len(), 1);
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
