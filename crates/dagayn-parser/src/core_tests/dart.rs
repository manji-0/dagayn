use super::*;

#[test]
fn resolves_dart_and_julia_imports_to_repo_relative_files() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-uri-import-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("app/lib/util")).unwrap();
    std::fs::create_dir_all(repo_root.join("jl/src")).unwrap();
    std::fs::write(repo_root.join("app/pubspec.yaml"), b"name: myapp\n").unwrap();
    std::fs::write(
        repo_root.join("app/lib/util/text.dart"),
        b"String t() => '';\n",
    )
    .unwrap();
    std::fs::write(repo_root.join("app/lib/models.dart"), b"class M {}\n").unwrap();
    std::fs::write(repo_root.join("jl/src/helpers.jl"), b"f() = 1\n").unwrap();

    let mut parser = RustOwnedParser::new();
    let dart = br#"import 'dart:async';
import 'package:flutter/material.dart';
import 'package:myapp/util/text.dart';
import '../models.dart';

class A {}
"#;
    let (_, edges) = parser.parse_file_in_repo(Some(&repo_root), "app/lib/util/view.dart", dart);
    let imports: Vec<&str> = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| edge.target.as_str())
        .collect();
    assert_eq!(
        imports,
        vec![
            // SDK and third-party URIs have no file here.
            "dart:async",
            "package:flutter/material.dart",
            // This repository's own package maps through its pubspec.
            "app/lib/util/text.dart",
            // A relative URI resolves against the importing file.
            "app/lib/models.dart",
        ]
    );

    let julia = br#"module Runner

using LinearAlgebra

include("helpers.jl")

end
"#;
    let (nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "jl/src/runner.jl", julia);
    let imports: Vec<&str> = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| edge.target.as_str())
        .collect();
    assert_eq!(imports, vec!["LinearAlgebra", "jl/src/helpers.jl"]);
    // A Julia `module` is a namespace, so another file's `using` can find it.
    assert_eq!(
        nodes[0].extra["namespaces"],
        serde_json::json!(["Runner"]),
        "julia module should be recorded as a namespace"
    );

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_dart_types_imports_and_calls() {
    let source = br#"import 'dart:async';

abstract class Animal {
  void speak();
}

mixin SwimmingMixin {
  void swim() => print('swimming');
}

enum PetType { dog, cat }

class Dog extends Animal with SwimmingMixin {
  void speak() {
    print('woof');
  }

  Future<void> fetch(String item) async {
    await _run();
    print(item);
  }

  void _run() {
    print('running');
  }

  static Dog create(String name) {
    return Dog(name);
  }
}

Dog createDog(String name) {
  return Dog(name);
}
"#;
    let (nodes, edges) = parse_dart("sample.dart", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Animal"
            && node.extra["type_role"] == "abstract_class"
            && node.extra["is_abstract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "PetType"
            && node.extra["type_role"] == "enum"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "SwimmingMixin" && node.extra["type_role"] == "mixin"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "fetch"
            && node.parent_name.as_deref() == Some("Dog")
            && node.params.as_deref() == Some("(String item)")
    }));
    assert!(
        edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "dart:async" })
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS" && edge.source == "sample.dart::Dog" && edge.target == "Animal"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "sample.dart::Dog"
            && edge.target == "SwimmingMixin"
    }));
    // Calls are attributed to the enclosing method, not to the file.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.dart::Dog.fetch"
            && edge.target == "sample.dart::Dog._run"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.dart::createDog"
            && edge.target == "sample.dart::Dog"
    }));
    // An `=>` body belongs to its signature too.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.dart::SwimmingMixin.swim"
            && edge.target == "print"
    }));
    assert!(
        edges
            .iter()
            .all(|edge| edge.kind != "CALLS" || edge.source != "sample.dart")
    );
    // The function node spans its body, not just the signature line.
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "fetch" && node.line_end > node.line_start
    }));
}
