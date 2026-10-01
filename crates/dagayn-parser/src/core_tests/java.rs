use super::*;

#[test]
fn java_qualified_calls_resolve_to_the_invoked_method() {
    let source = br#"class Broker {
    static CertificateInfo build(CertTypes t) { return null; }
}

class Factory {
    CertificateInfo createAllowed(CertTypes t) {
        return Broker.build(t);
    }

    List<Issuer> issuers() {
        return Registry.<Issuer>lookup();
    }
}
"#;
    let (nodes, edges) = parse_java("F.java", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "createAllowed"
            && node.parent_name.as_deref() == Some("Factory")
    }));
    // `Broker.build(t)` targets the method, not the receiver class.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "F.java::Factory.createAllowed"
            && edge.target == "F.java::Broker.build"
    }));
    assert!(edges.iter().all(|edge| {
        edge.kind != "CALLS"
            || edge.source != "F.java::Factory.createAllowed"
            || edge.target != "F.java::Broker"
    }));
    // An explicit type argument sits between receiver and name.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "F.java::Factory.issuers" && edge.target == "lookup"
    }));
}

#[test]
fn parses_java_types_imports_calls_and_bridges() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-java-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/main/java/com/example/util")).unwrap();
    std::fs::create_dir_all(repo_root.join("src/main/java/com/example/app")).unwrap();
    std::fs::write(
        repo_root.join("src/main/java/com/example/util/Helper.java"),
        b"package com.example.util;\npublic class Helper {}\n",
    )
    .unwrap();

    let source = br#"package com.example.app;

import static com.example.util.Helper.MAX;
import java.util.Map;

public record UserRecord(String id) {}

public interface Repository {
  void save(UserRecord user);
}

abstract class BaseRepo implements Repository {
  public void save(UserRecord user) {
    Runtime.getRuntime().exec("./bin/dagayn");
    Runtime.getRuntime().exec(command());
    System.loadLibrary("dagayn");
  }
}

class CachedRepo extends BaseRepo {
  public void save(UserRecord user) {
    super.save(user);
  }
}
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file_in_repo(
        Some(&repo_root),
        "src/main/java/com/example/app/App.java",
        source,
    );
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Repository"
            && node.extra["type_role"] == "interface"
            && node.extra["is_contract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "UserRecord"
            && node.extra["type_role"] == "record"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "BaseRepo"
            && node.extra["type_role"] == "abstract_class"
            && node.extra["is_abstract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "save"
            && node.parent_name.as_deref() == Some("BaseRepo")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.target == "src/main/java/com/example/util/Helper.java"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPLEMENTS"
            && edge.source == "src/main/java/com/example/app/App.java::BaseRepo"
            && edge.target == "Repository"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "src/main/java/com/example/app/App.java::CachedRepo"
            && edge.target == "BaseRepo"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "./bin/dagayn"
            && edge.extra["evidence_source"] == "Runtime.getRuntime().exec"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target
                == "<dynamic:Runtime.getRuntime().exec@src/main/java/com/example/app/App.java:15>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "dagayn"
            && edge.extra["evidence_source"] == "System.loadLibrary"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn java_native_methods_record_their_jni_symbols() {
    let source = br#"package com.example;
public class Sum {
    static { System.loadLibrary("fastsum"); }
    public static native double fastSum(double[] xs);
    public double total(double[] xs) { return fastSum(xs); }
    static class Inner { native void do_it(); }
}
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("src/com/example/Sum.java", source);
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
        symbol("do_it").as_deref(),
        Some("Java_com_example_Sum_00024Inner_do_1it")
    );
    assert_eq!(symbol("total"), None);
}

#[test]
fn java_standard_library_calls_target_their_package() {
    let source = br#"package com.example;

import static java.lang.Math.max;
import java.util.List;
import java.io.*;
import com.acme.util.Strings;

class Report {
    static String valueOf(Object o) { return ""; }

    void run(List<String> names, int n) {
        System.out.println(n);
        Math.min(1, 2);
        max(1, 2);
        Integer.parseInt("1");
        java.util.Arrays.asList(1);
        new StringBuilder().append("x");
        names.add("a");
        var copy = new ArrayList<String>();
        copy.add("b");
        new File("x").exists();
        valueOf(n);
        Strings.join(names);
        helper.add("c");
        "x".trim();
    }
}
"#;
    let (_, edges) = parse_java("src/com/example/Report.java", source);
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
        ("java.lang", "System.out.println", "HIGH"),
        ("java.lang", "Math.min", "HIGH"),
        ("java.lang", "Math.max", "HIGH"),
        ("java.lang", "Integer.parseInt", "HIGH"),
        ("java.util", "java.util.Arrays.asList", "HIGH"),
        ("java.lang", "StringBuilder.append", "HIGH"),
        ("java.util", "List.add", "MEDIUM"),
        ("java.io", "File.exists", "HIGH"),
        ("java.lang", "String.trim", "HIGH"),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    // `ArrayList` is not imported (only `java.util.List` is).
    assert!(calls.contains(&("add", "", "")), "{calls:?}");
    // The file's own `valueOf`, a repository class, and an untyped receiver.
    assert!(calls.contains(&("src/com/example/Report.java::Report.valueOf", "", "")));
    assert!(calls.contains(&("join", "", "")), "{calls:?}");
    assert_eq!(
        edges
            .iter()
            .filter(|edge| edge.kind == "CALLS" && edge.extra["stdlib"] == true)
            .count(),
        9
    );

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
    let list = import("java.util.List");
    assert_eq!(list.target, "java.util");
    assert_eq!(list.extra["stdlib"], true);
    assert_eq!(list.extra["confidence_tier"], "HIGH");
    assert_eq!(import("java.lang.Math.max").target, "java.lang");
    assert_eq!(import("java.io.*").target, "java.io");
    let third_party = import("com.acme.util.Strings");
    assert_eq!(third_party.extra.get("stdlib"), None);
}

#[test]
fn java_classes_of_its_own_are_not_the_class_library() {
    let source = br#"import com.acme.*;

class Math { static int max(int a, int b) { return a; } }

class Use {
    void run() {
        Math.max(1, 2);
        System.exit(0);
    }
}
"#;
    let (_, edges) = parse_java("Use.java", source);
    let call = |name: &str| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == "CALLS"
                    && (edge.target.ends_with(name) || edge.extra["external_symbol"] == name)
            })
            .unwrap_or_else(|| panic!("no call {name}"))
    };
    assert_eq!(call("max").target, "Use.java::Math.max");
    // `com.acme.*` may bring in a `System` of its own.
    let exit = call("System.exit");
    assert_eq!(exit.target, "java.lang");
    assert_eq!(exit.extra["confidence_tier"], "MEDIUM");
}

#[test]
fn java_receivers_record_the_call_they_came_from() {
    let source = br#"package app;

import com.acme.Repo;

class Service {
    private Repo repo;
    private final Cache cache = new Cache();

    List<User> users(Store store, T item) {
        store.open().fetch();
        var conn = factory.connect();
        conn.execute();
        repo.save();
        this.repo.flush();
        new Repo().load();
        cache.get();
        Repo.create();
        plugin.run();
        b.with(1).with(2);
        return null;
    }
}

class Cache {
    Object get() { return null; }
}
"#;
    let (nodes, edges) = parse_java("src/app/Service.java", source);
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
    assert_eq!(users.return_type.as_deref(), Some("List<User>"));
    // Declared as a class of another file: parameter, field, `this.` field,
    // constructor.
    assert_eq!(call("open").extra["receiver_type"], "Store");
    assert_eq!(call("save").extra["receiver_type"], "Repo");
    assert_eq!(call("flush").extra["receiver_type"], "Repo");
    assert_eq!(call("load").extra["receiver_type"], "Repo");
    // A class of this file keeps the same-file binding.
    assert_eq!(
        call("src/app/Service.java::Cache.get")
            .extra
            .get("receiver_unknown"),
        None
    );
    // A static call and an untyped receiver.
    assert_eq!(call("create").extra.get("receiver_unknown"), None);
    assert_eq!(call("run").extra["receiver_unknown"], true);
    assert_eq!(call("run").extra.get("receiver_from"), None);
    assert_eq!(call("connect").extra["receiver_unknown"], true);
    // Receivers that are call results, directly or through a variable.
    assert_eq!(
        call("fetch").extra["receiver_from"],
        serde_json::json!({"call": "open", "line": 10, "unwrap": false})
    );
    assert_eq!(
        call("execute").extra["receiver_from"],
        serde_json::json!({"call": "connect", "line": 11, "unwrap": false})
    );
    // A repeated method is skipped back to the receiver before it.
    assert_eq!(call("with").extra.get("receiver_from"), None);
    assert_eq!(call("with").extra["receiver_unknown"], true);
}
