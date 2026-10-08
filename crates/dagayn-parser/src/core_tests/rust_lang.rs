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
        edge.kind == "IMPORTS_FROM"
            && edge.source == "src/lib.rs"
            && edge.target == "std"
            && edge.extra["paths"] == serde_json::json!(["std::fs"])
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

#[test]
fn records_node_addon_exports() {
    let source = br#"use napi_derive::napi;

#[napi]
pub fn fast_sum(xs: Vec<f64>) -> f64 { 0.0 }

#[napi(js_name = "meanOf")]
pub fn mean(xs: Vec<f64>) -> f64 { 0.0 }

#[napi]
pub struct Accumulator { total: f64 }

#[napi(object)]
pub struct Options { pub strict: bool }

#[napi]
impl Accumulator {
    #[napi(constructor)]
    pub fn new() -> Self { Accumulator { total: 0.0 } }
    #[napi]
    pub fn add_value(&mut self, x: f64) {}
}

#[neon::export]
fn add_one(n: f64) -> f64 { n + 1.0 }

#[neon::export(name = "helloSync")]
fn hello() -> String { String::new() }

fn legacy_hello(mut cx: FunctionContext) -> JsResult<JsString> { todo!() }

#[neon::main]
fn main(mut cx: ModuleContext) -> NeonResult<()> {
    cx.export_function("legacyHello", legacy_hello)?;
    Ok(())
}
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("native/src/lib.rs", source);
    let node = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
    };
    let export = |name: &str| node(name).extra.get("ffi_export").cloned();
    let json = |kind: &str, name: &str| {
        Some(serde_json::json!({"abi": "napi", "kind": kind, "name": name}))
    };
    assert_eq!(export("fast_sum"), json("function", "fastSum"));
    assert_eq!(export("mean"), json("function", "meanOf"));
    assert_eq!(export("Accumulator"), json("class", "Accumulator"));
    assert_eq!(export("Options"), None);
    assert_eq!(export("new"), json("method", "constructor"));
    assert_eq!(export("add_value"), json("method", "addValue"));
    assert_eq!(export("add_one"), json("function", "addOne"));
    assert_eq!(export("hello"), json("function", "helloSync"));
    assert_eq!(
        node("legacy_hello").extra.get("ffi_exports").cloned(),
        Some(serde_json::json!([{"abi": "napi", "kind": "function", "name": "legacyHello"}]))
    );
}

#[test]
fn wasm_bindgen_extern_declarations_are_imports_not_exports() {
    let source = br#"use wasm_bindgen::prelude::*;
#[wasm_bindgen(module = "/js/util.js")]
extern "C" {
    fn format_date(ms: f64) -> String;
    #[wasm_bindgen(js_name = parseDate)]
    fn parse_date(s: &str) -> f64;
    #[wasm_bindgen(method)]
    fn render(this: &Widget);
}
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("wasm/src/lib.rs", source);
    let node = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
    };
    for name in ["format_date", "parse_date", "render", "log"] {
        assert_eq!(node(name).extra.get("ffi_export"), None, "{name}");
    }
    let import = |name: &str| node(name).extra.get("ffi_import").cloned();
    assert_eq!(
        import("format_date"),
        Some(serde_json::json!({"abi": "wasm", "module": "/js/util.js", "name": "format_date"}))
    );
    assert_eq!(
        import("parse_date"),
        Some(serde_json::json!({"abi": "wasm", "module": "/js/util.js", "name": "parseDate"}))
    );
    assert_eq!(import("render"), None);
    assert_eq!(import("log"), None);
}

#[test]
fn c_extern_declarations_record_the_symbol_they_link() {
    let source = br#"#[link(name = "fastsum")]
extern "C" {
    fn fast_sum(xs: *const f64, n: usize) -> f64;
    #[link_name = "real_name"]
    fn alias();
}
unsafe extern "C" { pub safe fn abs(x: i32) -> i32; }
extern "system" { fn GetTickCount() -> u32; }
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("src/lib.rs", source);
    let import = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .get("ffi_import")
            .cloned()
    };
    assert_eq!(
        import("fast_sum"),
        Some(serde_json::json!({"abi": "c", "name": "fast_sum", "library": "fastsum"}))
    );
    assert_eq!(
        import("alias"),
        Some(serde_json::json!({"abi": "c", "name": "real_name", "library": "fastsum"}))
    );
    assert_eq!(
        import("abs"),
        Some(serde_json::json!({"abi": "c", "name": "abs"}))
    );
    assert_eq!(
        import("GetTickCount"),
        Some(serde_json::json!({"abi": "c", "name": "GetTickCount"}))
    );
}

#[test]
fn cxx_bridge_declarations_record_imports_and_exports() {
    let source = br#"#[cxx::bridge(namespace = "org::blobstore")]
mod ffi {
    extern "Rust" {
        type MultiBuf;
        fn next_chunk(buf: &mut MultiBuf) -> &[u8];
    }
    unsafe extern "C++" {
        include!("demo/include/blobstore.h");
        type BlobstoreClient;
        fn new_blobstore_client() -> UniquePtr<BlobstoreClient>;
        fn put(self: Pin<&mut BlobstoreClient>, parts: &mut MultiBuf) -> u64;
        fn tag(self: &BlobstoreClient, blobid: u64, tag: &str);
    }
}
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("src/main.rs", source);
    let extra = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .clone()
    };
    assert_eq!(
        extra("next_chunk").get("ffi_export").cloned(),
        Some(serde_json::json!({"abi": "cxx", "kind": "function", "name": "next_chunk"}))
    );
    assert_eq!(
        extra("new_blobstore_client").get("ffi_import").cloned(),
        Some(serde_json::json!({"abi": "cxx", "name": "new_blobstore_client"}))
    );
    assert_eq!(
        extra("put").get("ffi_import").cloned(),
        Some(serde_json::json!({"abi": "cxx", "name": "put", "class": "BlobstoreClient"}))
    );
    assert_eq!(
        extra("tag").get("ffi_import").cloned(),
        Some(serde_json::json!({"abi": "cxx", "name": "tag", "class": "BlobstoreClient"}))
    );
}

#[test]
fn records_uniffi_exports_and_namespace() {
    let source = br#"uniffi::setup_scaffolding!("mathcore");

#[uniffi::export]
pub fn fast_sum(xs: Vec<f64>) -> f64 { 0.0 }

#[derive(uniffi::Object)]
pub struct Accumulator { total: f64 }

#[uniffi::export]
impl Accumulator {
    #[uniffi::constructor]
    pub fn new() -> Self { todo!() }
}

pub fn internal() {}
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("rust/src/lib.rs", source);
    let export = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .get("ffi_export")
            .cloned()
    };
    assert_eq!(
        nodes[0].extra.get("uniffi_namespace").cloned(),
        Some(serde_json::json!("mathcore"))
    );
    assert_eq!(
        export("fast_sum"),
        Some(serde_json::json!({"abi": "uniffi", "kind": "function", "name": "fast_sum"}))
    );
    assert_eq!(
        export("Accumulator"),
        Some(serde_json::json!({"abi": "uniffi", "kind": "class", "name": "Accumulator"}))
    );
    assert_eq!(
        export("new"),
        Some(serde_json::json!({"abi": "uniffi", "kind": "method", "name": "new"}))
    );
    assert_eq!(export("internal"), None);
}

#[test]
fn records_wasm_host_loads_and_export_lookups() {
    let source = br#"fn run(engine: &Engine, store: &mut Store<()>) {
    let module = Module::from_file(engine, "guest.wasm").unwrap();
    let bytes = include_bytes!("plugin.wasm");
    let add = instance.get_typed_func::<(i32, i32), i32>(&mut *store, "add").unwrap();
    let mul = instance.exports.get_function("mul").unwrap();
    let other = lookup("add");
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("host/src/main.rs", source);
    let bridges: Vec<(&str, &str)> = edges
        .iter()
        .filter(|edge| edge.kind == "CROSS_ARTIFACT")
        .map(|edge| {
            (
                edge.extra["relationship_role"].as_str().unwrap_or_default(),
                edge.target.as_str(),
            )
        })
        .collect();
    assert_eq!(
        bridges,
        vec![
            ("loads_wasm_module", "guest.wasm"),
            ("loads_wasm_module", "plugin.wasm"),
            ("calls_wasm_export", "add"),
            ("calls_wasm_export", "mul"),
        ]
    );
}

#[test]
fn records_component_model_guest_exports_host_imports_and_calls() {
    let guest = br#"wit_bindgen::generate!({ world: "calculator", path: "wit" });
struct Component;
impl exports::example::calc::ops::Guest for Component {
    fn add(a: i32, b: i32) -> i32 { a + b }
}
impl Guest for Component {
    fn run() {}
}
impl std::fmt::Display for Component {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }
}
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("guest/src/lib.rs", guest);
    let export = |nodes: &[crate::core::types::ParsedNode], name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .get("ffi_export")
            .cloned()
    };
    assert_eq!(
        export(&nodes, "add"),
        Some(serde_json::json!({
            "abi": "wit", "kind": "function", "interface": "example::calc::ops", "name": "add"
        }))
    );
    assert_eq!(
        export(&nodes, "run"),
        Some(serde_json::json!({"abi": "wit", "kind": "function", "interface": "", "name": "run"}))
    );
    assert_eq!(export(&nodes, "fmt"), None);

    let host = br#"wasmtime::component::bindgen!({ world: "calculator" });
struct State;
impl example::calc::logging::Host for State {
    fn log(&mut self, msg: String) {}
}
fn run(bindings: &Calculator, store: &mut Store<State>) {
    bindings.example_calc_ops().call_add(&mut *store, 1, 2).unwrap();
}
"#;
    let (nodes, edges) = parser.parse_file("host/src/main.rs", host);
    assert_eq!(
        export(&nodes, "log"),
        Some(serde_json::json!({
            "abi": "wit_host", "kind": "function", "interface": "example::calc::logging",
            "name": "log"
        }))
    );
    let call = edges
        .iter()
        .find(|edge| edge.extra["relationship_role"] == "calls_component_export")
        .expect("component call");
    assert_eq!(call.target, "add");
    assert_eq!(call.extra["interface_hint"], "example_calc_ops");

    // Without component bindings a `call_*` method is an ordinary call.
    let (_, edges) = parser.parse_file("app/src/lib.rs", b"fn f(c: &C) { c.call_api(); }\n");
    assert!(!edges.iter().any(|edge| edge.kind == "CROSS_ARTIFACT"));
}

#[test]
fn marks_rust_impls_no_repository_type_can_own() {
    let source = br#"use std::sync::Arc;
use core::slice;

struct Option;
struct Local;

impl<T: Clone> Parse for T { fn parse() {} }
impl From<u8> for String {}
impl Tr for Arc<Local> {}
impl<'a, T> Tr for slice::IterMut<'a, T> {}
impl Tr for serde_json::Value {}
impl Tr for Vec<u8> { fn tr() {} }
impl Tr for Option {}
impl Tr for Local {}
impl Tr for crate::model::Remote {}
impl Tr for Remote {}
"#;
    let (nodes, edges) = parse_rust("src/lib.rs", source);
    let target_of = |owner: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "IMPLEMENTS" && edge.source == format!("src/lib.rs::{owner}"))
            .unwrap_or_else(|| panic!("no IMPLEMENTS from {owner}"))
            .extra
            .get("impl_target")
            .and_then(|target| target.as_str())
            .map(str::to_string)
    };
    assert_eq!(target_of("T").as_deref(), Some("generic"));
    assert_eq!(target_of("String").as_deref(), Some("external"));
    assert_eq!(target_of("Arc").as_deref(), Some("external"));
    assert_eq!(target_of("IterMut").as_deref(), Some("external"));
    // Without a repository, `serde_json` is not known to be a dependency.
    assert_eq!(target_of("Value"), None);
    assert_eq!(target_of("Vec").as_deref(), Some("external"));
    // The file declares its own `Option`.
    assert_eq!(target_of("Option"), None);
    assert_eq!(target_of("Local"), None);
    assert_eq!(target_of("Remote"), None);
    let member_target = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .and_then(|node| node.extra.get("impl_target"))
            .and_then(|target| target.as_str())
            .map(str::to_string)
    };
    assert_eq!(member_target("parse").as_deref(), Some("generic"));
    assert_eq!(member_target("tr").as_deref(), Some("external"));
}

#[test]
fn a_test_declares_what_it_exercises_in_a_comment() {
    let source = br#"//! Mentions `dagayn: tests src/x.rs::example` in prose: no edge.

#[test]
fn dispatches_by_name() {
    // dagayn: tests crates/tools/src/review.rs::review
    /// dagayn: implements docs/spec.md#Review
    run("review_tool");
    // a note that says dagayn: tests src/y.rs::other is no directive
    let fixture = "
    // dagayn: implements docs/fixture.md#Inside
    ";
}
"#;
    let (_, edges) = parse_rust("tests/tools.rs", source);
    let directives: Vec<(&str, &str, &str)> = edges
        .iter()
        .filter(|edge| edge.kind == "CROSS_ARTIFACT")
        .map(|edge| {
            (
                edge.source.as_str(),
                edge.target.as_str(),
                edge.extra["relationship_role"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        directives,
        [
            (
                "tests/tools.rs::dispatches_by_name",
                "crates/tools/src/review.rs::review",
                "tests"
            ),
            (
                "tests/tools.rs::dispatches_by_name",
                "docs/spec.md::review",
                "implements_contract"
            ),
        ]
    );
    let tests = edges
        .iter()
        .find(|edge| edge.extra["relationship_role"] == "tests")
        .expect("tests edge");
    assert_eq!(tests.extra["bridge_kind"], "test");
    assert_eq!(tests.line, 5);
}

#[test]
fn calls_through_closures_values_and_macro_fields_have_edges() {
    let source = br#"
pub(super) struct Directive {
    pub kind: Kind,
}
pub(super) enum Kind { A }
impl Kind {
    fn role(self) -> &'static str { "a" }
}
pub struct Dep { x: i64 }
impl Dep {
    pub(crate) fn finding(&self) -> i64 { self.x }
    pub(crate) fn introduced(&self) -> bool { true }
}
fn in_macro(directive: &Directive) {
    println!("{}", directive.kind.role());
}
fn make() -> Vec<Dep> { Vec::new() }
fn run() -> Vec<i64> {
    make().iter().filter(|dep| dep.introduced()).map(Dep::finding).collect()
}
fn other() -> Vec<i64> {
    make().iter().map(crate::model::Other::score).collect()
}
fn mentions(text: &str) -> bool {
    let word = |c: char| c.is_alphanumeric();
    text.chars().next().is_some_and(word)
}
"#;
    let (_, edges) = parse_rust("src/x.rs", source);
    let edge = |kind: &str, source: &str, target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == kind && edge.source == source && edge.target == target)
    };
    // A field's declared type types the receiver inside a macro too.
    assert!(edge("CALLS", "src/x.rs::in_macro", "src/x.rs::Kind.role").is_some());
    // A closure over the elements of what `make` returned: resolution takes
    // `Dep` out of `Vec<Dep>`.
    let introduced = edge("CALLS", "src/x.rs::run", "introduced").expect("introduced");
    assert_eq!(
        introduced.extra["receiver_from"],
        serde_json::json!({"call": "make", "line": 19, "unwrap": false, "element": true})
    );
    // Functions passed as values.
    assert!(edge("REFERENCES", "src/x.rs::run", "src/x.rs::Dep.finding").is_some());
    let score = edge("REFERENCES", "src/x.rs::other", "score").expect("score");
    assert_eq!(score.extra["value_reference"], true);
    assert_eq!(score.extra["receiver_type"], "Other");
    assert!(
        edge(
            "REFERENCES",
            "src/x.rs::mentions",
            "src/x.rs::mentions.word"
        )
        .is_some()
    );
}
