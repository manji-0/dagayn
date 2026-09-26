use super::*;

#[test]
fn parses_go_types_methods_calls_and_bridges() {
    let source = br#"package main

import (
  "os"
  "os/exec"
  "plugin"
)

type Repo struct {}

func NewRepo() *Repo {
  return &Repo{}
}

func (r *Repo) Save() {
  os.WriteFile("output.json", []byte("ok"), 0644)
}

func runCommand(path string) {
  exec.Command("git", "status")
  os.ReadFile(path)
  plugin.Open("mylib.so")
}
"#;
    let (nodes, edges) = parse_go("main.go", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Repo"
            && node.language == "go"
            && node.extra["type_role"] == "struct"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "Save"
            && node.parent_name.as_deref() == Some("Repo")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "main.go" && edge.target == "os/exec"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "main.go::Repo"
            && edge.target == "main.go::Repo.Save"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git"
            && edge.extra["evidence_source"] == "exec.Command"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:os.ReadFile@main.go:21>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.so"
            && edge.extra["evidence_source"] == "plugin.Open"
    }));
}
