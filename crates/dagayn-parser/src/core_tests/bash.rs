use super::*;

#[test]
fn parses_bash_functions_calls_and_sources() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-bash-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("scripts")).unwrap();
    std::fs::write(repo_root.join("scripts/lib.sh"), b"helper() { echo ok; }\n").unwrap();

    let source = br#"#!/usr/bin/env bash
source ./lib.sh

greet() {
  echo "hi"
}

main() {
  greet
}

main "$@"
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "scripts/app.sh", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "greet"
            && node.language == "bash"
            && node.file_path == "scripts/app.sh"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "main"
            && node.language == "bash"
            && node.file_path == "scripts/app.sh"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "scripts/app.sh"
            && edge.target == "scripts/lib.sh"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "scripts/app.sh::main"
            && edge.target == "scripts/app.sh::greet"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "scripts/app.sh"
            && edge.target == "scripts/app.sh::main"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}
