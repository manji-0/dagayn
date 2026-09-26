use super::*;

#[test]
fn parses_rust_items_imports_and_calls() {
    let source = br#"pub use dagayn_graph::{GraphStore};
use std::fs;

#[derive(Serialize, Deserialize)]
pub struct Foo {
    value: i32,
}

pub enum Mode {
    Fast,
}

impl Foo {
    pub fn new() -> Self {
        Self { value: 1 }
    }

    fn load(&self) {
        fs::read("path");
        consume(helper);
        helper();
    }
}

fn consume(_f: fn()) {}
fn helper() {}
"#;
    let (nodes, edges) = parse_rust("src/lib.rs", source);
    let node_names = nodes
        .iter()
        .map(|node| {
            (
                node.kind.as_str(),
                node.name.as_str(),
                node.parent_name.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert!(node_names.contains(&("File", "src/lib.rs", None)));
    assert!(node_names.contains(&("Class", "Foo", None)));
    assert!(node_names.contains(&("Class", "Mode", None)));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Foo"
            && node.extra["type_role"] == "struct"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
            && node.modifiers.as_deref() == Some("pub")
            && node.extra["derive_traits"] == serde_json::json!(["Serialize", "Deserialize"])
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Mode"
            && node.extra["type_role"] == "enum"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(node_names.contains(&("Function", "new", Some("Foo"))));
    assert!(node_names.contains(&("Function", "load", Some("Foo"))));
    assert!(node_names.contains(&("Function", "helper", None)));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "src/lib.rs" && edge.target == "std::fs"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "src/lib.rs"
            && edge.target == "dagayn_graph::GraphStore"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/lib.rs::Foo.load"
            && edge.target == "src/lib.rs::helper"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "src/lib.rs::Foo.load"
            && edge.target == "src/lib.rs::helper"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "src/lib.rs::Foo.load"
            && edge.target == "path"
    }));
}
