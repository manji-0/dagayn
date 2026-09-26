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
