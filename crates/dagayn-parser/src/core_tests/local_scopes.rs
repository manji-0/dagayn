//! Declarations inside a function body are local to it: two functions'
//! `helper`s are `a.helper` and `b.helper`, and each function's call binds
//! to its own (JavaScript does not make locals nodes at all).

use super::*;

fn parse(file_path: &str, source: &str) -> (Vec<String>, Vec<(String, String)>) {
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file(file_path, source.as_bytes());
    let prefix = format!("{file_path}::");
    let names = nodes
        .iter()
        .filter(|node| node.kind != NodeKind::File)
        .map(|node| match &node.parent_name {
            Some(parent) => format!("{parent}.{}", node.name),
            None => node.name.clone(),
        })
        .collect();
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| {
            let strip = |value: &str| value.strip_prefix(&prefix).unwrap_or(value).to_string();
            (strip(&edge.source), strip(&edge.target))
        })
        .collect();
    (names, calls)
}

// Unused in a build whose only languages here are Java, Go, Zig, Scala, C#.
#[allow(dead_code)]
fn assert_local_helpers(file_path: &str, source: &str) {
    let (names, calls) = parse(file_path, source);
    for expected in ["a", "a.helper", "b", "b.helper"] {
        assert!(
            names.contains(&expected.to_string()),
            "{file_path}: {expected} not in {names:?}"
        );
    }
    for caller in ["a", "b"] {
        let target = format!("{caller}.helper");
        assert!(
            calls.contains(&(caller.to_string(), target.clone())),
            "{file_path}: no {caller} -> {target} in {calls:?}"
        );
    }
}

#[cfg(feature = "lang-python")]
#[test]
fn python_nested_functions() {
    assert_local_helpers(
        "n.py",
        "def a():\n    def helper():\n        return 1\n    return helper()\n\ndef b():\n    def helper():\n        return 2\n    return helper()\n",
    );
}

#[cfg(feature = "lang-rust")]
#[test]
fn rust_items_in_function_bodies() {
    assert_local_helpers(
        "n.rs",
        "fn a() -> u8 {\n    fn helper() -> u8 { 1 }\n    helper()\n}\nfn b() -> u8 {\n    fn helper() -> u8 { 2 }\n    helper()\n}\n",
    );
}

#[cfg(feature = "lang-lua")]
#[test]
fn lua_local_functions() {
    assert_local_helpers(
        "n.lua",
        "local function a()\n  local function helper() return 1 end\n  return helper()\nend\nlocal function b()\n  local helper = function() return 2 end\n  return helper()\nend\n",
    );
}

#[cfg(feature = "lang-kotlin")]
#[test]
fn kotlin_local_functions() {
    assert_local_helpers(
        "n.kt",
        "fun a() {\n  fun helper() = 1\n  helper()\n}\nfun b() {\n  fun helper() = 2\n  helper()\n}\n",
    );
}

#[cfg(feature = "lang-swift")]
#[test]
fn swift_nested_functions() {
    assert_local_helpers(
        "n.swift",
        "func a() {\n  func helper() {}\n  helper()\n}\nfunc b() {\n  func helper() {}\n  helper()\n}\n",
    );
}

#[cfg(feature = "lang-dart")]
#[test]
fn dart_local_functions() {
    assert_local_helpers(
        "n.dart",
        "void a() { int helper() => 1; helper(); }\nvoid b() { int helper() => 2; helper(); }\n",
    );
}

#[cfg(all(feature = "lang-java", feature = "lang-go", feature = "lang-zig"))]
#[test]
fn local_types_in_java_go_and_zig() {
    let (java, _) = parse(
        "N.java",
        "class A {\n  void f() { class Local {} }\n  void g() { class Local {} }\n}\n",
    );
    assert!(java.contains(&"A.f.Local".to_string()) && java.contains(&"A.g.Local".to_string()));
    let (go, _) = parse(
        "n.go",
        "package p\nfunc a() {\n\ttype T struct{}\n\t_ = T{}\n}\nfunc b() {\n\ttype T struct{}\n\t_ = T{}\n}\n",
    );
    assert!(go.contains(&"a.T".to_string()) && go.contains(&"b.T".to_string()));
    let (zig, _) = parse(
        "n.zig",
        "fn a() void {\n    const S = struct { x: u8 };\n    _ = S;\n}\nfn b() void {\n    const S = struct { y: u8 };\n    _ = S;\n}\n",
    );
    assert!(zig.contains(&"a.S".to_string()) && zig.contains(&"b.S".to_string()));
}

#[cfg(feature = "lang-scala")]
#[test]
fn scala_nested_defs() {
    let (names, calls) = parse(
        "n.scala",
        "object O {\n  def a() = { def helper() = 1; helper() }\n  def b() = { def helper() = 2; helper() }\n}\n",
    );
    assert!(names.contains(&"O.a.helper".to_string()) && names.contains(&"O.b.helper".to_string()));
    assert!(
        calls.contains(&("O.b".to_string(), "O.b.helper".to_string())),
        "{calls:?}"
    );
}

#[cfg(feature = "lang-julia")]
#[test]
fn julia_nested_functions() {
    assert_local_helpers(
        "n.jl",
        "function a()\n    function helper()\n        1\n    end\n    helper()\nend\nfunction b()\n    helper() = 2\n    helper()\nend\n",
    );
}

#[cfg(feature = "lang-r")]
#[test]
fn r_local_functions() {
    assert_local_helpers(
        "n.r",
        "a <- function() {\n  helper <- function() 1\n  helper()\n}\nb <- function() {\n  helper <- function() 2\n  helper()\n}\n",
    );
}

#[cfg(feature = "lang-csharp")]
#[test]
fn csharp_local_functions() {
    let (names, calls) = parse(
        "n.cs",
        "class C {\n  void A() { int helper() => 1; helper(); }\n  void B() { int helper() => 2; helper(); }\n}\n",
    );
    assert!(names.contains(&"C.A.helper".to_string()) && names.contains(&"C.B.helper".to_string()));
    assert!(
        calls.contains(&("C.B".to_string(), "C.B.helper".to_string())),
        "{calls:?}"
    );
}
