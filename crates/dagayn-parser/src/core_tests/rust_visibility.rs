//! Rust visibility in `modifiers` and the attributes that make a function a
//! test.

use super::*;

/// `modifiers` of the node `name` (owned by `parent`) of kind `kind`.
fn modifiers_of(
    nodes: &[ParsedNode],
    kind: &str,
    name: &str,
    parent: Option<&str>,
) -> Option<String> {
    nodes
        .iter()
        .find(|node| {
            node.kind == kind && node.name == name && node.parent_name.as_deref() == parent
        })
        .unwrap_or_else(|| panic!("no {kind} {name} under {parent:?}"))
        .modifiers
        .clone()
}

#[test]
fn records_declared_visibility_on_functions_types_and_modules() {
    let source = br#"pub fn open() {}
pub(crate) fn shared() {}
pub(super) fn parent_only() {}
pub(self) fn own() {}
pub(in crate::outer) fn scoped() {}
fn private() {}
pub(crate) async unsafe fn spawn() {}

pub struct Public;
pub(crate) struct Internal;
pub(super) enum Up { A }
pub(in crate::outer) type Alias = u8;
struct Hidden;
pub(crate) trait Store {}
pub mod api {}
mod inner {}

extern "C" {
    pub fn ffi_call();
}

impl Public {
    pub fn make() -> Self { Public }
    pub(crate) fn reset(&mut self) {}
    fn step(&self) {}
}

fn outer() {
    fn nested() {}
    let call = |x: u8| x;
}
"#;
    let (nodes, _) = parse_rust("src/lib.rs", source);
    let function = |name: &str| modifiers_of(&nodes, "Function", name, None);
    assert_eq!(function("open").as_deref(), Some("pub"));
    assert_eq!(function("shared").as_deref(), Some("pub(crate)"));
    assert_eq!(function("parent_only").as_deref(), Some("pub(super)"));
    assert_eq!(function("own").as_deref(), Some("pub(self)"));
    assert_eq!(function("scoped").as_deref(), Some("pub(in crate::outer)"));
    assert_eq!(function("private"), None);
    assert_eq!(function("spawn").as_deref(), Some("pub(crate)"));
    assert_eq!(function("ffi_call").as_deref(), Some("pub"));

    let class = |name: &str| modifiers_of(&nodes, "Class", name, None);
    assert_eq!(class("Public").as_deref(), Some("pub"));
    assert_eq!(class("Internal").as_deref(), Some("pub(crate)"));
    assert_eq!(class("Up").as_deref(), Some("pub(super)"));
    assert_eq!(class("Hidden"), None);
    assert_eq!(class("Store").as_deref(), Some("pub(crate)"));
    assert_eq!(class("api").as_deref(), Some("pub"));
    assert_eq!(class("inner"), None);
    assert_eq!(
        modifiers_of(&nodes, "Type", "Alias", None).as_deref(),
        Some("pub(in crate::outer)")
    );

    let method = |name: &str| modifiers_of(&nodes, "Function", name, Some("Public"));
    assert_eq!(method("make").as_deref(), Some("pub"));
    assert_eq!(method("reset").as_deref(), Some("pub(crate)"));
    assert_eq!(method("step"), None);

    assert_eq!(
        modifiers_of(&nodes, "Function", "nested", Some("outer")),
        None
    );
    assert_eq!(
        modifiers_of(&nodes, "Function", "call", Some("outer")),
        None
    );
}

#[test]
fn trait_items_and_trait_impl_methods_take_the_trait_visibility() {
    let source = br#"pub trait Render {
    fn render(&self) -> String;
    fn name(&self) -> &str { "x" }
}

pub(crate) trait Store {
    fn save(&self);
}

trait Local {
    fn local(&self);
}

pub struct Page;

impl Render for Page {
    fn render(&self) -> String { String::new() }
}

impl Store for Page {
    fn save(&self) {}
}

impl Local for Page {
    fn local(&self) {}
}

impl std::fmt::Display for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { Ok(()) }
}
"#;
    let (nodes, _) = parse_rust("src/lib.rs", source);
    let member = |name: &str, parent: &str| modifiers_of(&nodes, "Function", name, Some(parent));
    assert_eq!(member("render", "Render").as_deref(), Some("pub"));
    assert_eq!(member("name", "Render").as_deref(), Some("pub"));
    assert_eq!(member("save", "Store").as_deref(), Some("pub(crate)"));
    assert_eq!(member("local", "Local"), None);

    assert_eq!(member("render", "Page").as_deref(), Some("pub"));
    assert_eq!(member("save", "Page").as_deref(), Some("pub(crate)"));
    assert_eq!(member("local", "Page"), None);
    // A trait declared in another file or crate: its visibility is unknown here.
    assert_eq!(member("fmt", "Page"), None);
}

#[test]
fn only_test_attributes_make_a_rust_function_a_test() {
    let source = br#"#[test]
fn plain() {}

#[tokio::test(flavor = "multi_thread")]
async fn on_tokio() {}

#[rstest]
fn with_rstest() {}

#[test_case(1)]
fn with_cases(_n: u8) {}

#[wasm_bindgen_test]
fn in_wasm() {}

#[cfg(test)]
fn test_only_helper() {}

#[cfg(all(test, feature = "x"))]
fn gated_helper() {}

#[cfg(not(test))]
fn production_only() {}

#[cfg(any(test, feature = "x"))]
fn either() {}

#[cfg_attr(test, derive(Debug))]
fn attr_mentions_test() {}

#[doc = "see the tests"]
fn documented() {}

#[contest]
fn contest() {}

#[pyo3(signature = (include_tests = false))]
fn with_signature(include_tests: bool) {}

mod gated {
    #![cfg(test)]
    fn inner_gated() {}
}

mod ungated {
    #![cfg(not(test))]
    fn inner_ungated() {}
}
"#;
    let (nodes, _) = parse_rust("src/lib.rs", source);
    let kind = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .kind
            .as_str()
    };
    for name in [
        "plain",
        "on_tokio",
        "with_rstest",
        "with_cases",
        "in_wasm",
        "test_only_helper",
        "gated_helper",
        "inner_gated",
    ] {
        assert_eq!(kind(name), "Test", "{name}");
    }
    for name in [
        "production_only",
        "either",
        "attr_mentions_test",
        "documented",
        "contest",
        "with_signature",
        "inner_ungated",
    ] {
        assert_eq!(kind(name), "Function", "{name}");
    }
}
