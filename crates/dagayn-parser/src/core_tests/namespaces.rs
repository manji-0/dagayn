use super::*;

#[test]
fn records_declared_namespaces_on_the_file_node() {
    fn namespaces(nodes: &[ParsedNode]) -> Vec<String> {
        nodes
            .iter()
            .find(|node| node.kind == "File")
            .and_then(|node| node.extra["namespaces"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect()
    }

    let (nodes, _) = parse_csharp(
        "a.cs",
        br#"namespace Repro.Infra.Cert
{
    internal class A {}
}
"#,
    );
    assert_eq!(namespaces(&nodes), vec!["Repro.Infra.Cert".to_string()]);

    // A file-scoped namespace declares the same thing without a block.
    let (nodes, _) = parse_csharp(
        "b.cs",
        br#"namespace Repro.Infra.Cert;

internal class B {}
"#,
    );
    assert_eq!(namespaces(&nodes), vec!["Repro.Infra.Cert".to_string()]);

    let (nodes, _) = parse_java("A.java", b"package com.example.app;\nclass A {}\n");
    assert_eq!(namespaces(&nodes), vec!["com.example.app".to_string()]);

    let (nodes, _) = parse_kotlin("A.kt", b"package com.example.kt\n\nclass A\n");
    assert_eq!(namespaces(&nodes), vec!["com.example.kt".to_string()]);

    let (nodes, _) = parse_scala("A.scala", b"package com.example.sc\n\nclass A\n");
    assert_eq!(namespaces(&nodes), vec!["com.example.sc".to_string()]);

    let (nodes, edges) = parse_php(
        "a.php",
        br#"<?php
namespace App\Infra;

use App\Util\Helper;
use App\Util\{One, Two};
use App\Other\Third as T;

class A {}
"#,
    );
    assert_eq!(namespaces(&nodes), vec!["App\\Infra".to_string()]);
    // Group form expands per clause and an `as` alias is dropped.
    let imports: Vec<&str> = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| edge.target.as_str())
        .collect();
    assert_eq!(
        imports,
        vec![
            "App\\Util\\Helper",
            "App\\Util\\One",
            "App\\Util\\Two",
            "App\\Other\\Third",
        ]
    );

    // A file with no namespace declaration records nothing.
    let (nodes, _) = parse_csharp("c.cs", b"internal class C {}\n");
    assert!(nodes[0].extra.get("namespaces").is_none());
}
