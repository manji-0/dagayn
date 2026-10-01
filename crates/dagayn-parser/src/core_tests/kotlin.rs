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
            && edge.target == "java.nio.file"
            && edge.extra["external_symbol"] == "java.nio.file.Files"
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

#[test]
fn kotlin_standard_library_calls_target_their_package() {
    let source = br#"import java.io.File
import kotlin.math.max
import com.acme.util.Strings

class Report(private val out: File) {
    fun run(names: List<String>, check: () -> Unit) {
        println(names)
        val xs = listOf(1, 2)
        File("x").readText()
        val f = File("y")
        f.readLines()
        max(1, 2)
        kotlin.math.min(1, 2)
        System.getenv("HOME")
        names.size()
        check()
        require(true)
        Strings.join(names)
        helper.add("c")
    }

    fun require(value: Boolean) {}
}
"#;
    let (_, edges) = parse_kotlin("src/Report.kt", source);
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| {
            (
                edge.target.as_str(),
                edge.extra["external_symbol"].as_str().unwrap_or_default(),
                edge.extra["confidence_tier"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        ("kotlin", "println", "MEDIUM"),
        ("kotlin", "listOf", "MEDIUM"),
        ("java.io", "File", "HIGH"),
        ("java.io", "File.readText", "HIGH"),
        ("java.io", "File.readLines", "MEDIUM"),
        ("kotlin.math", "max", "HIGH"),
        ("kotlin.math", "kotlin.math.min", "HIGH"),
        ("java.lang", "System.getenv", "HIGH"),
        ("kotlin", "List.size", "MEDIUM"),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    // A parameter, the file's own `require`, a repository class, and an
    // untyped receiver.
    assert!(calls.contains(&("check", "", "")), "{calls:?}");
    assert!(
        calls.contains(&("src/Report.kt::Report.require", "", "")),
        "{calls:?}"
    );
    assert!(calls.contains(&("join", "", "")), "{calls:?}");
    assert!(calls.contains(&("add", "", "")), "{calls:?}");

    let import = |symbol: &str| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == "IMPORTS_FROM"
                    && edge
                        .extra
                        .get("external_symbol")
                        .map_or(edge.target == symbol, |written| written == symbol)
            })
            .unwrap_or_else(|| panic!("no import {symbol}"))
    };
    let file = import("java.io.File");
    assert_eq!(file.target, "java.io");
    assert_eq!(file.extra["confidence_tier"], "HIGH");
    assert_eq!(import("kotlin.math.max").target, "kotlin.math");
    assert_eq!(import("com.acme.util.Strings").extra.get("stdlib"), None);
}

#[test]
fn kotlin_receivers_record_the_call_they_came_from() {
    let source = br#"package app

import com.acme.Repo

class Service(private val repo: Repo) {
    fun users(store: Store): User? {
        store.open().fetch()
        val conn = factory.connect()
        conn.execute()
        repo.save()
        this.repo.flush()
        Repo().load()
        cache.get()
        Repo.create()
        items.forEach { it.run() }
        return null
    }

    private val cache = Cache()
}

class Cache {
    fun get(): Any? = null
}
"#;
    let (nodes, edges) = parse_kotlin("src/app/Service.kt", source);
    let call = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target)
            .unwrap_or_else(|| panic!("no {target} in {edges:#?}"))
    };
    let users = nodes
        .iter()
        .find(|node| node.name == "users")
        .expect("users");
    assert_eq!(users.return_type.as_deref(), Some("User?"));
    // Declared as a class of another file: parameter, constructor property,
    // `this.` property, constructor call.
    assert_eq!(call("open").extra["receiver_type"], "Store");
    assert_eq!(call("save").extra["receiver_type"], "Repo");
    assert_eq!(call("flush").extra["receiver_type"], "Repo");
    assert_eq!(call("load").extra["receiver_type"], "Repo");
    // A class of this file keeps the same-file binding, wherever the
    // property is declared.
    assert_eq!(
        call("src/app/Service.kt::Cache.get")
            .extra
            .get("receiver_unknown"),
        None
    );
    // A call on an object or class, and untyped receivers.
    assert_eq!(call("create").extra.get("receiver_unknown"), None);
    assert_eq!(call("run").extra["receiver_unknown"], true);
    assert_eq!(call("connect").extra["receiver_unknown"], true);
    // Receivers that are call results, directly or through a variable.
    assert_eq!(
        call("fetch").extra["receiver_from"],
        serde_json::json!({"call": "open", "line": 7, "unwrap": false})
    );
    assert_eq!(
        call("execute").extra["receiver_from"],
        serde_json::json!({"call": "connect", "line": 8, "unwrap": false})
    );
}
