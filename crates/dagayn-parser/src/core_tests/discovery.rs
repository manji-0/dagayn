use super::*;

#[test]
fn detects_extensions_and_shebangs() {
    assert_eq!(detect_language(Path::new("main.py")), Some("python"));
    assert_eq!(detect_language(Path::new("main.R")), Some("r"));
    assert_eq!(detect_language(Path::new("main.unknown")), None);
}

#[test]
fn nested_dir_ignore_matches_python_behavior() {
    let patterns = vec!["node_modules/**".to_string()];
    assert!(should_ignore(
        "pkg/app/node_modules/react/index.js",
        &patterns,
        None
    ));
    assert!(should_ignore(
        "node_modules/react/index.js",
        &patterns,
        None
    ));
    assert!(!should_ignore("pkg/app/src/index.js", &patterns, None));
}

#[test]
fn walk_files_prunes_ignored_directories() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-walk-ignore-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src")).unwrap();
    std::fs::create_dir_all(repo_root.join("pkg/node_modules/lib")).unwrap();
    std::fs::write(repo_root.join("src/main.py"), b"def main():\n    pass\n").unwrap();
    std::fs::write(
        repo_root.join("pkg/node_modules/lib/index.js"),
        b"export const slow = 1;\n",
    )
    .unwrap();

    let patterns = load_ignore_patterns(&repo_root);
    let globset = build_globset(&patterns);
    let files = walk_files(&repo_root, &patterns, globset.as_ref());

    assert!(files.contains(&"src/main.py".to_string()));
    assert!(!files.iter().any(|file| file.contains("node_modules")));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_extensionless_shebang_script_as_rust_owned() {
    let source = br#"#!/usr/bin/env bash
deploy() {
  echo "deploy"
}

deploy "$@"
"#;
    assert!(!rust_parser_owns_path("bin/deploy"));
    assert!(rust_parser_owns_source("bin/deploy", source));

    let (nodes, edges) = parse_rust_owned_file("bin/deploy", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "deploy" && node.language == "bash"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "bin/deploy" && edge.target == "bin/deploy::deploy"
    }));
}

#[test]
fn parses_powershell_file_without_extra_nodes_for_python_parity() {
    let source = br#"function Invoke-Hello {
    param($Name)
    Write-Host "Hello $Name"
}

Invoke-Hello -Name World
"#;
    let (nodes, edges) = parse_powershell("scripts/hello.ps1", source);

    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].kind, "File");
    assert_eq!(nodes[0].name, "scripts/hello.ps1");
    assert_eq!(nodes[0].language, "powershell");
    assert_eq!(nodes[0].line_start, 1);
    assert_eq!(nodes[0].line_end, 7);
    assert!(edges.is_empty());
}

#[test]
fn parses_rust_owned_files_as_one_compact_batch() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-batch-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("docs")).unwrap();
    std::fs::write(
        repo_root.join("docs/README.md"),
        b"# Guide\n\nSee `build_graph`.\n",
    )
    .unwrap();
    std::fs::write(
        repo_root.join("main.tf"),
        br#"variable "region" {
  default = "us-east-1"
}
"#,
    )
    .unwrap();

    let payload = parse_rust_owned_files_compact_json(
        &repo_root,
        &["docs/README.md".to_string(), "main.tf".to_string()],
    );
    let parsed: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(parsed["errors"].as_array().unwrap().len(), 0);
    let batch = parsed["batch"].as_array().unwrap();
    assert_eq!(batch.len(), 2);
    assert!(batch.iter().any(|item| item[0] == "docs/README.md"));
    assert!(batch.iter().any(|item| item[0] == "main.tf"));
    let results = parsed["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|item| item["status"] == "ok"));
    assert!(
        results
            .iter()
            .any(|item| item["file_path"] == "docs/README.md")
    );

    let _ = std::fs::remove_dir_all(&repo_root);
}
