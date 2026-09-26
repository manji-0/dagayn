use super::*;

#[test]
fn parses_swift_types_functions_calls_and_bridges() {
    let source = br#"import Foundation

struct User {
    let name: String
}

class Repo {
    func save(_ user: User) {
        print(user.name)
    }
}

func runProcess() {
    let p = Process.run(URL(fileURLWithPath: "/usr/bin/git"), arguments: ["status"])
    _ = p
}

func loadLib() {
    dlopen("mylib.dylib", RTLD_NOW)
}
"#;
    let (nodes, edges) = parse_swift("App.swift", source);

    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.extra["type_role"] == "struct"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "save")
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "Foundation")
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "print")
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.extra["evidence_source"] == "Process.run"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.dylib"
            && edge.extra["evidence_source"] == "dlopen"
    }));
}
