use super::*;

#[test]
fn parses_kotlin_types_calls_imports_and_bridges() {
    let source = br#"import java.nio.file.Files

interface UserRepository {
    fun save(user: User)
}

data class User(val id: Int)

class InMemoryRepo : UserRepository {
    fun save(user: User) {
        println(user)
        Files.writeString(java.nio.file.Path.of("output.txt"), "ok")
    }

    fun run(path: String) {
        Runtime.getRuntime().exec("git status")
        Files.readString(java.nio.file.Path.of(path))
        System.loadLibrary("mylib")
    }
}

fun createUser(repo: UserRepository) {
    val user = User(1)
    repo.save(user)
}
"#;
    let (nodes, edges) = parse_kotlin("sample.kt", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "UserRepository"
            && node.extra["type_role"] == "interface"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.extra["type_role"] == "record"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "save"
            && node.parent_name.as_deref() == Some("InMemoryRepo")
            && node.params.is_none()
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "sample.kt"
            && edge.target == "java.nio.file.Files"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPLEMENTS"
            && edge.source == "sample.kt::InMemoryRepo"
            && edge.target == "UserRepository"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.kt::createUser"
            && edge.target == "sample.kt::UserRepository.save"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "Runtime.getRuntime().exec"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:Files.readString@sample.kt:17>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib"
            && edge.extra["evidence_source"] == "System.loadLibrary"
    }));
}

#[test]
fn kotlin_external_functions_record_their_jni_symbols() {
    let source = br#"package com.example

class Sum {
    external fun fastSum(xs: DoubleArray): Double
    fun total(xs: DoubleArray) = fastSum(xs)
    companion object {
        external fun viaCompanion(): Int
        @JvmStatic external fun staticOne(): Int
    }
}

external fun topLevel(): Int
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("src/com/example/sum.kt", source);
    let symbol = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .pointer("/ffi_import/symbol")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    assert_eq!(
        symbol("fastSum").as_deref(),
        Some("Java_com_example_Sum_fastSum")
    );
    assert_eq!(
        symbol("viaCompanion").as_deref(),
        Some("Java_com_example_Sum_00024Companion_viaCompanion")
    );
    assert_eq!(
        symbol("staticOne").as_deref(),
        Some("Java_com_example_Sum_staticOne")
    );
    assert_eq!(
        symbol("topLevel").as_deref(),
        Some("Java_com_example_SumKt_topLevel")
    );
    assert_eq!(symbol("total"), None);

    let (nodes, _) = parser.parse_file(
        "src/util.kt",
        b"@file:JvmName(\"NativeUtil\")\npackage com.example\n\nexternal fun ping(): Int\n",
    );
    let ping = nodes.iter().find(|node| node.name == "ping").unwrap();
    assert_eq!(
        ping.extra
            .pointer("/ffi_import/symbol")
            .and_then(serde_json::Value::as_str),
        Some("Java_com_example_NativeUtil_ping")
    );
}
