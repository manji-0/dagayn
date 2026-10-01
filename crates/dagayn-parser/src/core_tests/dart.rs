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
            && edge.target == "dart:core"
            && edge.extra["external_symbol"] == "print"
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

#[test]
fn dart_native_externals_record_their_c_symbol() {
    let source = br#"import 'dart:ffi';
@Native<Double Function(Int32)>(symbol: 'fast_sum')
external double fastSum(int n);
@Native<Int32 Function()>()
external int version();
int plain() => 0;
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("lib/sum.dart", source);
    let symbol = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .pointer("/ffi_import/name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    assert_eq!(symbol("fastSum").as_deref(), Some("fast_sum"));
    assert_eq!(symbol("version").as_deref(), Some("version"));
    assert_eq!(symbol("plain"), None);
}

#[test]
fn dart_standard_library_calls_target_their_package() {
    let source = br#"import 'dart:io';
import 'dart:convert' as convert;
import 'dart:math' show max;
import 'package:http/http.dart' as http;

class Logger {
  void log(String m) {}
}

void jsonEncode(Object o) {}

void main(List<String> args) {
  final f = File('x');
  print(convert.jsonEncode({}));
  var d = DateTime.now();
  int.parse('1');
  max(1, 2);
  sqrt(4);
  http.get(Uri.parse('x'));
  jsonEncode({});
  final logger = Logger();
  logger.log('x');
}
"#;
    let (_nodes, edges) = parse_dart("lib/main.dart", source);
    let find = |kind: &str, symbol: &str| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == kind
                    && (edge.extra["external_symbol"] == symbol
                        || (edge.target == symbol && edge.extra.get("external_symbol").is_none()))
            })
            .unwrap_or_else(|| panic!("no {kind} {symbol} in {edges:#?}"))
    };
    // `dart:` imports: certain; `package:` imports are not the standard library.
    for library in ["dart:io", "dart:convert", "dart:math"] {
        let import = find("IMPORTS_FROM", library);
        assert_eq!(import.target, library);
        assert_eq!(import.extra["stdlib"], true);
        assert_eq!(import.extra["confidence_tier"], "HIGH");
    }
    let http = find("IMPORTS_FROM", "package:http/http.dart");
    assert!(http.extra.get("stdlib").is_none(), "{http:?}");
    // A call through a `dart:` import prefix: certain.
    let encode = find("CALLS", "convert.jsonEncode");
    assert_eq!(encode.target, "dart:convert");
    assert_eq!(encode.extra["confidence_tier"], "HIGH");
    // `dart:core` names and names of unprefixed `dart:` imports: likely.
    for (symbol, library) in [
        ("print", "dart:core"),
        ("DateTime.now", "dart:core"),
        ("int.parse", "dart:core"),
        ("Uri.parse", "dart:core"),
        ("File", "dart:io"),
        ("max", "dart:math"),
    ] {
        let call = find("CALLS", symbol);
        assert_eq!(call.target, library, "{symbol}");
        assert_eq!(call.extra["confidence_tier"], "MEDIUM", "{symbol}");
    }
    // `show max` leaves `sqrt` out of scope; this file's own `jsonEncode`
    // and `Logger.log` are not the library's; a third-party prefix stays.
    for target in [
        "sqrt",
        "lib/main.dart::jsonEncode",
        "get",
        "lib/main.dart::Logger.log",
    ] {
        let call = find("CALLS", target);
        assert!(call.extra.get("stdlib").is_none(), "{call:?}");
    }
    assert!(
        edges
            .iter()
            .all(|edge| edge.extra.get("dart_callee_qualifier").is_none()),
        "{edges:#?}"
    );
}

#[test]
fn dart_receivers_record_the_call_they_came_from() {
    let source = br#"import 'models.dart' as m;
class Store {
  void save() {}
}
class Service {
  final Repo repo;
  Future<Store> open(String p) async => Store();
  void run(Cache cache) async {
    var s = await open("a");
    s.save();
    (await open("b")).save();
    open("c").save();
    final local = Store();
    local.save();
    cache.flush();
    repo.find();
    this.repo.find();
    final z = load();
    z?.go();
    m.Repo().find();
    Repo.shared();
  }
}
"#;
    let (nodes, edges) = parse_dart("lib/service.dart", source);
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:#?}"))
    };
    let open = nodes.iter().find(|node| node.name == "open").expect("open");
    assert_eq!(open.return_type.as_deref(), Some("Future<Store>"));
    // `await` unwraps the `Future`.
    assert_eq!(
        call("save", 10).extra["receiver_from"],
        serde_json::json!({"call": "open", "line": 9, "unwrap": true})
    );
    assert_eq!(call("save", 10).extra["receiver_unknown"], true);
    assert_eq!(
        call("save", 11).extra["receiver_from"],
        serde_json::json!({"call": "open", "line": 11, "unwrap": true})
    );
    assert_eq!(
        call("save", 12).extra["receiver_from"],
        serde_json::json!({"call": "open", "line": 12, "unwrap": false})
    );
    // A type of this library declaring the method: the method itself.
    assert_eq!(
        call("lib/service.dart::Store.save", 14)
            .extra
            .get("receiver_unknown"),
        None
    );
    // Types of other libraries: a parameter, a field, `this.field`, a
    // constructor through an import prefix.
    assert_eq!(call("flush", 15).extra["receiver_type"], "Cache");
    assert_eq!(call("find", 16).extra["receiver_type"], "Repo");
    assert_eq!(call("find", 17).extra["receiver_type"], "Repo");
    assert_eq!(call("find", 20).extra["receiver_type"], "Repo");
    // `?.` unwraps the nullable result.
    assert_eq!(
        call("go", 19).extra["receiver_from"],
        serde_json::json!({"call": "load", "line": 18, "unwrap": true})
    );
    // A member of a type is no unknown receiver.
    assert_eq!(call("shared", 21).extra.get("receiver_unknown"), None);
}
