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

#[test]
fn bash_standard_library_calls_target_their_package() {
    let source = br#"#!/usr/bin/env bash
source ./lib.sh

cd() {
  builtin cd "$@" && echo "now in $PWD"
}

main() {
  echo "hi"
  printf '%s\n' "$1"
  read -r line
  cd /tmp
  grep -q x file
  helper
}
"#;
    let (_, edges) = parse_bash("scripts/app.sh", source);
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
        // A builtin is known by its name alone.
        ("bash", "echo", "MEDIUM"),
        ("bash", "printf", "MEDIUM"),
        ("bash", "read", "MEDIUM"),
        ("bash", "builtin", "MEDIUM"),
        // The file's own `cd` shadows the builtin.
        ("scripts/app.sh::cd", "", ""),
        // Programs on `PATH` are no standard library.
        ("grep", "", ""),
        ("helper", "", ""),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    let import = edges
        .iter()
        .find(|edge| edge.kind == "IMPORTS_FROM")
        .expect("source import");
    assert_eq!(import.extra.get("stdlib"), None);
}
