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

#[test]
fn records_rust_ffi_exports() {
    let source = br#"use pyo3::prelude::*;

#[pyfunction]
fn fast_sum(xs: Vec<f64>) -> f64 { 0.0 }

#[pyfunction]
#[pyo3(name = "total")]
fn total_impl() {}

#[pyclass(name = "GraphStore")]
struct PyGraphStore {}

#[pymethods]
impl PyGraphStore {
    #[new]
    fn new() -> Self { Self {} }
    fn commit(&self) {}
}

#[unsafe(no_mangle)]
pub extern "C" fn c_sum() -> f64 { 0.0 }

#[export_name = "renamed_symbol"]
pub extern "C" fn exported() {}

fn private_helper() {}
"#;
    let (nodes, _) = parse_rust("src/lib.rs", source);
    let export = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .get("ffi_export")
            .cloned()
    };
    let json = |abi: &str, kind: &str, name: &str| {
        Some(serde_json::json!({"abi": abi, "kind": kind, "name": name}))
    };
    assert_eq!(export("fast_sum"), json("pyo3", "function", "fast_sum"));
    assert_eq!(export("total_impl"), json("pyo3", "function", "total"));
    assert_eq!(export("PyGraphStore"), json("pyo3", "class", "GraphStore"));
    assert_eq!(export("new"), json("pyo3", "method", "__new__"));
    assert_eq!(export("commit"), json("pyo3", "method", "commit"));
    assert_eq!(export("c_sum"), json("c", "function", "c_sum"));
    assert_eq!(export("exported"), json("c", "function", "renamed_symbol"));
    assert_eq!(export("private_helper"), None);
}

#[test]
fn records_wasm_bindgen_exports() {
    let source = br#"use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn fast_sum(xs: &[f64]) -> f64 { 0.0 }

#[wasm_bindgen(js_name = meanOf)]
pub fn mean_of(xs: &[f64]) -> f64 { 0.0 }

#[wasm_bindgen(js_name = "Acc")]
pub struct Accumulator { total: f64 }

#[wasm_bindgen]
impl Accumulator {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Accumulator { Accumulator { total: 0.0 } }
    pub fn push(&mut self, x: f64) {}
    fn internal(&self) {}
}
"#;
    let (nodes, _) = parse_rust("src/lib.rs", source);
    let export = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .get("ffi_export")
            .cloned()
    };
    let json = |kind: &str, name: &str| {
        Some(serde_json::json!({"abi": "wasm", "kind": kind, "name": name}))
    };
    assert_eq!(export("fast_sum"), json("function", "fast_sum"));
    assert_eq!(export("mean_of"), json("function", "meanOf"));
    assert_eq!(export("Accumulator"), json("class", "Acc"));
    assert_eq!(export("new"), json("method", "constructor"));
    assert_eq!(export("push"), json("method", "push"));
    // Only `pub` methods cross the boundary.
    assert_eq!(export("internal"), None);
}
