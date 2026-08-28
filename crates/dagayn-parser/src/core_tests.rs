use super::*;

#[test]
fn detects_extensions_and_shebangs() {
    assert_eq!(detect_language(Path::new("main.py")), Some("python"));
    assert_eq!(detect_language(Path::new("main.R")), Some("r"));
    assert_eq!(detect_language(Path::new("main.unknown")), None);
}

#[test]
fn detects_compound_terraform_extensions() {
    assert_eq!(
        detect_language(Path::new("main.tftest.hcl")),
        Some("terraform")
    );
    assert_eq!(
        detect_language(Path::new("main.TFTEST.HCL")),
        Some("terraform")
    );
    assert_eq!(
        detect_language(Path::new("component.tfcomponent.hcl")),
        Some("terraform")
    );
    assert_eq!(
        detect_language(Path::new("deploy.tfdeploy.hcl")),
        Some("terraform")
    );
    assert_eq!(
        detect_language(Path::new("query.tfquery.hcl")),
        Some("terraform")
    );
    assert_eq!(detect_language(Path::new("plain.hcl")), None);
    assert_eq!(detect_language(Path::new("main.tf")), Some("terraform"));
}

#[test]
fn parses_terraform_json_syntax() {
    let source = br#"{
  "resource": {
    "aws_vpc": { "main": { "cidr_block": "10.0.0.0/16" } }
  },
  "variable": {
    "region": { "default": "us-east-1" }
  },
  "module": {
    "network": { "source": "./modules/network" }
  },
  "output": {
    "vpc_id": { "value": "${aws_vpc.main.id}" }
  },
  "check": {
    "vpc_ready": { "assert": [] }
  }
}"#;
    let (nodes, edges) = parse_terraform("main.tf.json", source);
    let names = nodes
        .iter()
        .map(|node| node.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"resource.aws_vpc.main"));
    assert!(names.contains(&"var.region"));
    assert!(names.contains(&"module.network"));
    assert!(names.contains(&"output.vpc_id"));
    assert!(names.contains(&"check.vpc_ready"));
    let check = nodes
        .iter()
        .find(|node| node.name == "check.vpc_ready")
        .expect("check node exists");
    assert!(
        !check.is_test,
        "production `check` block must not be a test"
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "main.tf.json::module.network"
            && edge.target == "./modules/network"
    }));
}

#[test]
fn parses_terraform_json_vars_file() {
    let source = br#"{ "region": "us-east-1", "tags": { "env": "prod" } }"#;
    let (nodes, edges) = parse_terraform("terraform.tfvars.json", source);
    assert_eq!(nodes.len(), 1, "tfvars.json keeps only the File node");
    assert_eq!(nodes[0].kind, "File");
    assert_eq!(nodes[0].language, "terraform");
    assert!(edges.is_empty());
}

#[test]
fn compound_terraform_files_survive_incremental_filtering() {
    let repo_root = std::env::temp_dir().join(format!(
        "dagayn-parser-tftest-filter-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&repo_root).expect("create temp repo");
    std::fs::write(
        repo_root.join("main.tftest.hcl"),
        b"run \"basic\" {\n  command = apply\n}\n",
    )
    .expect("write tftest file");
    std::fs::write(repo_root.join("main.tf"), b"resource \"a\" \"b\" {}\n").expect("write tf file");

    let candidates = vec!["main.tf".to_string(), "main.tftest.hcl".to_string()];
    let (parseable, removed) = filter_incremental_candidates(&repo_root, &candidates, &[]);
    let mut sorted = parseable.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec!["main.tf".to_string(), "main.tftest.hcl".to_string()]
    );
    assert!(removed.is_empty());

    let collected = collect_parseable_files(&repo_root, Some(false));
    assert!(collected.iter().any(|p| p == "main.tftest.hcl"));

    std::fs::remove_dir_all(&repo_root).expect("clean up temp repo");
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
fn parses_markdown_sections_and_edges() {
    let source = b"# API Reference

<!-- derived-from ./guide.md#Installation -->

See [Getting Started](./guide.md#Getting-Started).

[InstallRef]: ./guide.md#Installation

## Endpoints

Call `build_graph`.
";
    let (nodes, edges) = parse_markdown("api.md", source);
    assert_eq!(nodes.len(), 5);
    assert!(nodes.iter().any(|node| node.name == "api-reference"));
    assert!(nodes.iter().any(|node| node.name == "endpoints"));
    assert!(nodes.iter().any(|node| {
        node.kind == "DocBody"
            && node.name == "api-reference--body-1"
            && node.line_start == 5
            && node.line_end == 5
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "DocBody"
            && node.name == "endpoints--body-1"
            && node.line_start == 11
            && node.line_end == 11
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "DEPENDS_ON"
            && edge.source == "api.md::api-reference"
            && edge.target == "guide.md::installation"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "api.md::api-reference"
            && edge.target == "api.md::api-reference--body-1"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "api.md::api-reference"
            && edge.target == "guide.md::getting-started"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "api.md::api-reference"
            && edge.target == "guide.md::installation"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT" && edge.target == "<unresolved:build_graph>"
    }));
}

#[test]
fn parses_terraform_blocks_calls_and_refs() {
    let source = br#"terraform {
  required_providers {
    aws = {
      source = "hashicorp/aws"
    }
  }
}

variable "tags" {
  type = map(string)
}

locals {
  common_tags = merge(var.tags, {
    ManagedBy = "dagayn"
  })
}

module "network" {
  source = "./modules/network"
}

data "aws_caller_identity" "current" {}

resource "aws_vpc" "main" {
  cidr_block = module.network.cidr_block
  tags = merge(local.common_tags, {
    Account = data.aws_caller_identity.current.account_id
  })
}

check "vpc_ready" {
  assert {
    condition = length(module.network.public_subnet_ids) > 0
  }
}

output "vpc_id" {
  value = aws_vpc.main.id
}
"#;
    let (nodes, edges) = parse_terraform("main.tf", source);
    let names = nodes
        .iter()
        .map(|node| node.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"terraform"));
    assert!(names.contains(&"var.tags"));
    assert!(names.contains(&"local.common_tags"));
    assert!(names.contains(&"module.network"));
    assert!(names.contains(&"data.aws_caller_identity.current"));
    assert!(names.contains(&"resource.aws_vpc.main"));
    assert!(names.contains(&"check.vpc_ready"));
    let check_node = nodes
        .iter()
        .find(|node| node.name == "check.vpc_ready")
        .expect("check node exists");
    assert!(
        !check_node.is_test,
        "production `check` block must not be a test"
    );
    assert_eq!(check_node.kind, "Class");
    assert!(names.contains(&"output.vpc_id"));
    assert!(edges.iter().any(|edge| {
        edge.kind == "DEPENDS_ON"
            && edge.source == "main.tf::terraform"
            && edge.target == "hashicorp/aws"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "main.tf::module.network"
            && edge.target == "./modules/network"
    }));
    assert!(edges
        .iter()
        .any(|edge| edge.kind == "CALLS" && edge.target == "merge"));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "resource.aws_vpc.main"
            && edge.target == "main.tf::data.aws_caller_identity.current"
    }));
}

#[test]
fn extracts_terraform_code_bridges() {
    let source = br#"
resource "null_resource" "bootstrap" {
  provisioner "local-exec" {
    command = "python3 ${path.module}/../scripts/bootstrap.py"
  }
}

resource "aws_lambda_function" "auth" {
  filename = "${path.module}/../app/hello.py"
  handler  = "hello.main"
  runtime  = "python3.12"
  role     = "arn:aws:iam::123456789012:role/lambda"
}

resource "google_cloudfunctions_function" "api" {
  name             = "api"
  runtime          = "python312"
  entry_point      = "serve"
  source_directory = "${path.module}/../app"
}
"#;
    let (_nodes, edges) = parse_terraform("infra/main.tf", source);
    let bridges: Vec<_> = edges
        .iter()
        .filter(|edge| edge.kind == "CROSS_ARTIFACT")
        .collect();
    assert!(bridges.iter().any(|edge| {
        edge.extra["evidence_source"] == "provisioner.local-exec.command"
            && edge.target == "scripts/bootstrap.py"
            && edge.extra["relationship_role"] == "invokes_binary"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(bridges.iter().any(|edge| {
        edge.extra["evidence_source"] == "filename"
            && edge.target == "app/hello.py"
            && edge.extra["relationship_role"] == "maps_entrypoint"
    }));
    assert!(bridges.iter().any(|edge| {
        edge.extra["evidence_source"] == "handler"
            && edge.target == "<unresolved:hello.main>"
            && edge.extra["original_symbol_name"] == "hello.main"
    }));
    assert!(bridges.iter().any(|edge| {
        edge.extra["evidence_source"] == "source_directory" && edge.target == "app"
    }));
    assert!(bridges.iter().any(|edge| {
        edge.extra["evidence_source"] == "entry_point" && edge.target == "<unresolved:serve>"
    }));
}

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
fn parses_ruby_classes_calls_imports_and_bridges() {
    let source = br#"require 'json'

module Auth
  class UserRepository
    def save(user)
      File.write("output.json", "{}")
      puts "Saved #{user}"
    end

    def create_user(name)
      save(name)
    end
  end
end

def run_command(path)
  system("git status")
  File.read(path)
  Fiddle.dlopen("mylib.so")
end
"#;
    let (nodes, edges) = parse_ruby("app.rb", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Auth"
            && node.parent_name.is_none()
            && node.language == "ruby"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "UserRepository"
            && node.parent_name.as_deref() == Some("Auth")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "save"
            && node.parent_name.as_deref() == Some("UserRepository")
            && node.params.is_none()
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "app.rb" && edge.target == "json"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "app.rb::UserRepository.create_user"
            && edge.target == "app.rb::UserRepository.save"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "output.json"
            && edge.extra["evidence_source"] == "File.write"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "system"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:File.read@app.rb:18>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.so"
            && edge.extra["evidence_source"] == "Fiddle.dlopen"
    }));
}

#[test]
fn csharp_methods_keep_declared_names_and_resolve_qualified_calls() {
    let source = br#"internal static class CertificateCriteriaFactory
{
    internal static ClientCertificateInfo CreateAllowedCertificateCriteria(
        CertificateTypes certificateType)
    {
        return new ClientCertificateInfo(certificateType);
    }

    internal static List<Issuer> GetClientCertificateIssuers<T>(T source)
    {
        return null;
    }
}

public abstract class CertificateStoreBroker
{
    public ClientCertificateInfo Resolve(CertificateTypes certificateType)
    {
        return CertificateCriteriaFactory.CreateAllowedCertificateCriteria(certificateType);
    }
}
"#;
    let (nodes, edges) = parse_csharp("Certificate.cs", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "CreateAllowedCertificateCriteria"
            && node.parent_name.as_deref() == Some("CertificateCriteriaFactory")
            && node.return_type.as_deref() == Some("ClientCertificateInfo")
    }));
    // The generic return type must not leak into the method name either.
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "GetClientCertificateIssuers"
            && node.return_type.as_deref() == Some("List<Issuer>")
    }));
    assert!(nodes.iter().all(|node| {
        node.kind != "Function" || !matches!(node.name.as_str(), "ClientCertificateInfo" | "List")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "CertificateStoreBroker"
            && node.extra["type_role"] == "abstract_class"
            && node.extra["is_abstract"] == true
    }));
    // `Factory.Method(...)` resolves to the method, not the receiver type.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "Certificate.cs::CertificateStoreBroker.Resolve"
            && edge.target
                == "Certificate.cs::CertificateCriteriaFactory.CreateAllowedCertificateCriteria"
    }));
    // `new Type(...)` records a call to the constructed type.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source
                == "Certificate.cs::CertificateCriteriaFactory.CreateAllowedCertificateCriteria"
            && edge.target == "ClientCertificateInfo"
    }));
}

#[test]
fn parses_csharp_types_imports_and_bridges() {
    let source = br#"using System.IO;
using System.Diagnostics;
using System.Reflection;

struct User
{
    public string Path;
}

interface IRepository
{
    User FindById(int id);
    void Save(User user);
}

class BridgeSamples : IRepository
{
    public User FindById(int id)
    {
        return null;
    }

    public void Save(User user)
    {
        Process.Start("git", "status");
        File.ReadAllText(user.Path);
        Assembly.LoadFile("mylib.dll");
    }
}
"#;
    let (nodes, edges) = parse_csharp("sample.cs", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "IRepository"
            && node.extra["type_role"] == "interface"
            && node.extra["is_contract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "BridgeSamples" && node.extra["type_role"] == "class"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.extra["type_role"] == "struct"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "FindById"
            && node.parent_name.as_deref() == Some("BridgeSamples")
            && node.params.as_deref() == Some("(int id)")
            && node.return_type.as_deref() == Some("User")
    }));
    assert!(!nodes
        .iter()
        .any(|node| node.kind == "Function" && node.name == "User"));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "Save"
            && node.parent_name.as_deref() == Some("BridgeSamples")
            && node.params.as_deref() == Some("(User user)")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "sample.cs" && edge.target == "System.IO"
    }));
    assert!(edges.iter().all(|edge| edge.kind != "IMPLEMENTS"));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git"
            && edge.extra["evidence_source"] == "Process.Start"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:File.ReadAllText@sample.cs:26>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.dll"
            && edge.extra["evidence_source"] == "Assembly.LoadFile"
    }));
}

#[test]
fn parses_php_types_calls_imports_and_bridges() {
    let source = br#"<?php
use Exception;

interface Repository {
    public function save(User $user): void;
}

class User {
    public function __construct(int $id) {}
    public function toString(): string { return "u"; }
}

class ExtendedRepo implements Repository {
    public function save(User $user): void {
        $user->toString();
        file_put_contents("output.json", "{}");
    }

    public function run($path): void {
        sqlQuery("SELECT 1");
        $this->save(new User(1));
        parent::__construct();
        FFI::cdef("", "mylib.so");
        file_get_contents($path);
    }
}

function sqlQuery(string $query): array { return []; }
"#;
    let (nodes, edges) = parse_php("sample.php", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Repository"
            && node.extra["type_role"] == "interface"
            && node.extra["is_contract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "save"
            && node.parent_name.as_deref() == Some("Repository")
            && node.params.as_deref() == Some("(User $user)")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "sqlQuery"
            && node.parent_name.is_none()
            && node.params.as_deref() == Some("(string $query)")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "sample.php"
            && edge.target == "use Exception;"
    }));
    assert!(edges.iter().all(|edge| edge.kind != "IMPLEMENTS"));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.php::ExtendedRepo.save"
            && edge.target == "sample.php::User.toString"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.php::ExtendedRepo.run"
            && edge.target == "sample.php::sqlQuery"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "output.json"
            && edge.extra["evidence_source"] == "file_put_contents"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:FFI::cdef@sample.php:23>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:file_get_contents@sample.php:24>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
}

#[test]
fn parses_kotlin_types_calls_imports_and_bridges() {
    let source = br#"import java.nio.file.Files

interface UserRepository {
    fun save(user: User)
}

data class User(val id: Int)

class InMemoryRepo : UserRepository {
    fun save(user: User) {
        println(user)
        Files.writeString(java.nio.file.Path.of("output.txt"), "ok")
    }

    fun run(path: String) {
        Runtime.getRuntime().exec("git status")
        Files.readString(java.nio.file.Path.of(path))
        System.loadLibrary("mylib")
    }
}

fun createUser(repo: UserRepository) {
    val user = User(1)
    repo.save(user)
}
"#;
    let (nodes, edges) = parse_kotlin("sample.kt", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "UserRepository" && node.extra["type_role"] == "class"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.extra["type_role"] == "record"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "save"
            && node.parent_name.as_deref() == Some("InMemoryRepo")
            && node.params.is_none()
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "sample.kt"
            && edge.target == "import java.nio.file.Files"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "sample.kt::InMemoryRepo"
            && edge.target == "InMemoryRepo"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.kt::createUser"
            && edge.target == "sample.kt::UserRepository.save"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "Runtime.getRuntime().exec"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:Files.readString@sample.kt:17>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib"
            && edge.extra["evidence_source"] == "System.loadLibrary"
    }));
}

#[test]
fn parses_scala_types_calls_imports_and_bridges() {
    let source = br#"import scala.collection.mutable.{HashMap, ListBuffer}
import java.nio.file.Files

trait Repository[T]:
  def save(entity: T): Unit

final case class User(id: Int)

class InMemoryRepo extends Repository[User] with Serializable:
  private val users = mutable.HashMap[Int, User]()

  override def save(user: User): Unit =
    users.put(user.id, user)
    Files.writeString(Path.of("output.json"), "{}")

object BridgeSamples:
  def runCommand(): Unit =
    Runtime.getRuntime().exec("git status")

  def loadLib(): Unit =
    System.loadLibrary("mylib")
"#;
    let (nodes, edges) = parse_scala("sample.scala", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Repository"
            && node.extra["type_role"] == "trait"
            && node.extra["is_contract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.extra["type_role"] == "record"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "save"
            && node.parent_name.as_deref() == Some("InMemoryRepo")
            && node.params.as_deref() == Some("(user: User)")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.target == "scala.collection.mutable.HashMap"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPLEMENTS"
            && edge.source == "sample.scala::InMemoryRepo"
            && edge.target == "Serializable"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.scala" && edge.target == "HashMap"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "Runtime.getRuntime().exec"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:Files.writeString@sample.scala:14>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib"
            && edge.extra["evidence_source"] == "System.loadLibrary"
    }));
}

#[test]
fn parses_solidity_contracts_state_calls_and_imports() {
    let source = br#"import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

uint256 constant MAX_SUPPLY = 1_000_000 ether;

struct Position {
    address wallet;
}

interface IPool {
    function stake(uint256 amount) external;
}

library RewardMath {
    uint256 internal constant PRECISION = 1e18;
    function mulPrecise(uint256 a, uint256 b) internal pure returns (uint256) {
        require(b > 0, "zero");
        return (a * b) / PRECISION;
    }
}

contract Vault is ERC20, IPool {
    using RewardMath for uint256;

    mapping(address => uint256) public stakes;
    uint256 immutable launchTime;

    event Staked(address indexed user, uint256 amount);

    modifier nonZero(uint256 amount) {
        require(amount > 0, "zero");
        _;
    }

    constructor(string memory name) ERC20(name, "V") {
        launchTime = block.timestamp;
    }

    function stake(uint256 amount) external nonZero(amount) {
        stakes[msg.sender] += amount;
        _mint(msg.sender, amount);
        emit Staked(msg.sender, amount);
    }
}
"#;
    let (nodes, edges) = parse_solidity("sample.sol", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "IPool" && node.extra["type_role"] == "interface"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "stakes"
            && node.parent_name.as_deref() == Some("Vault")
            && node.return_type.as_deref() == Some("mapping(address => uint256)")
            && node.modifiers.as_deref() == Some("public")
            && node.extra["solidity_kind"] == "state_variable"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.target == "@openzeppelin/contracts/token/ERC20/ERC20.sol"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS" && edge.source == "sample.sol::Vault" && edge.target == "ERC20"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "DEPENDS_ON"
            && edge.source == "sample.sol::Vault"
            && edge.target == "RewardMath"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.sol::Vault.constructor"
            && edge.target == "ERC20"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.sol::Vault.stake"
            && edge.target == "sample.sol::Vault.nonZero"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.sol::Vault.stake"
            && edge.target == "sample.sol::Vault.Staked"
    }));
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
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "dart:async" }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS" && edge.source == "sample.dart::Dog" && edge.target == "Animal"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "sample.dart::Dog"
            && edge.target == "SwimmingMixin"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.dart"
            && edge.target == "sample.dart::Dog._run"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.dart" && edge.target == "sample.dart::Dog"
    }));
}

#[test]
fn parses_lua_functions_methods_imports_tests_and_bridges() {
    let source = br#"local json = require("cjson")
local log = require("logging").getLogger("sample")

function greet(name)
    print("Hello, " .. name)
    return name
end

local transform = function(data)
    return json.encode(data)
end

function Animal.new(name)
    return setmetatable({}, Animal)
end

function Animal:speak()
    log:info(self.name)
end

function Dog:fetch(item)
    self:speak()
    os.execute("git status")
    return item
end

local function test_greet()
    local result = greet("World")
    assert(result == "World")
end
"#;
    let (nodes, edges) = parse_lua("sample.lua", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "new"
            && node.parent_name.as_deref() == Some("Animal")
            && node.params.as_deref() == Some("(name)")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "fetch"
            && node.parent_name.as_deref() == Some("Dog")
    }));
    assert!(nodes
        .iter()
        .any(|node| { node.kind == "Test" && node.name == "test_greet" && node.is_test }));
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "cjson" }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.lua::Dog.fetch"
            && edge.target == "sample.lua::speak"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "sample.lua::Dog.fetch"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "os.execute"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "TESTED_BY"
            && edge.source == "sample.lua::greet"
            && edge.target == "sample.lua::test_greet"
    }));
}

#[test]
fn parses_luau_types_functions_methods_imports_and_tests() {
    let source = br#"local utils = require("lib.utils")
local log = require("logging").getLogger("sample")

type Vector3 = {
    x: number,
    y: number,
    z: number,
}

type Callback = (input: string) -> string

function greet(name: string): string
    print("Hello, " .. name)
    return name
end

local transform = function(data: any): string
    return utils.encode(data)
end

function Animal:speak(): string
    log:info(self.name)
    return self.name
end

local function test_greet()
    local result = greet("World")
    assert(result == "World")
end
"#;
    let (nodes, edges) = parse_luau("sample.luau", source);
    assert!(nodes
        .iter()
        .any(|node| { node.kind == "Class" && node.name == "Vector3" && node.language == "luau" }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Callback" && node.language == "luau"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "greet"
            && node.language == "luau"
            && node.params.as_deref() == Some("(name: string)")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "speak"
            && node.parent_name.as_deref() == Some("Animal")
            && node.language == "luau"
    }));
    assert!(nodes
        .iter()
        .any(|node| { node.kind == "Test" && node.name == "test_greet" && node.is_test }));
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "lib.utils" }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.luau::test_greet"
            && edge.target == "sample.luau::greet"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "TESTED_BY"
            && edge.source == "sample.luau::greet"
            && edge.target == "sample.luau::test_greet"
    }));
}

#[test]
fn parses_elixir_modules_functions_imports_and_calls() {
    let source = br#"defmodule Calculator do
  @moduledoc """
  Simple calculator module.
  """

  def add(a, b) do
    a + b
  end

  def subtract(a, b), do: a - b

  defp log(msg) do
    IO.puts(msg)
    :ok
  end

  def compute(a, b) do
    result = add(a, b)
    log("result: #{result}")
    result
  end
end

defmodule MathHelpers do
  alias Calculator
  import Calculator, only: [add: 2]
  require Logger

  def double(x) do
    Calculator.compute(x, x)
  end

  def triple(x) do
    double(x) + x
  end
end
"#;
    let (nodes, edges) = parse_elixir("sample.ex", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Calculator" && node.language == "elixir"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "MathHelpers" && node.language == "elixir"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "add"
            && node.parent_name.as_deref() == Some("Calculator")
            && node.params.as_deref() == Some("(a, b)")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "log"
            && node.parent_name.as_deref() == Some("Calculator")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "triple"
            && node.parent_name.as_deref() == Some("MathHelpers")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "sample.ex" && edge.target == "Logger"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.ex::Calculator.compute"
            && edge.target == "sample.ex::Calculator.add"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.ex::Calculator.compute"
            && edge.target == "sample.ex::Calculator.log"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.ex::MathHelpers.double"
            && edge.target == "sample.ex::Calculator.compute"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.ex::MathHelpers.triple"
            && edge.target == "sample.ex::MathHelpers.double"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.ex" && edge.target == "moduledoc"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.ex::Calculator.log" && edge.target == "puts"
    }));
}

#[test]
fn parses_gdscript_classes_functions_imports_and_calls() {
    let source = br#"extends Node
class_name SampleManager

const MAX_SIZE = 10
const OtherScript = preload("res://scripts/other.gd")

signal item_added(item: Item)

@export var speed: float = 2.5
@onready var timer: Timer = $Timer

var items: Array[Item] = []


class Item:
	var name: String
	var level: int

	func promote() -> void:
		level += 1


func _ready() -> void:
	timer.start()
	_load_items()
	OtherScript.register(self)


func _load_items() -> void:
	for i in range(MAX_SIZE):
		var item := Item.new()
		items.append(item)
		item_added.emit(item)


func get_item(idx: int) -> Item:
	return items[idx]


static func helper() -> int:
	return 42
"#;
    let (nodes, edges) = parse_gdscript("sample.gd", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "SampleManager"
            && node.language == "gdscript"
            && node.extra["type_role"] == "class"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Item" && node.language == "gdscript"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "promote"
            && node.parent_name.as_deref() == Some("Item")
            && node.params.as_deref() == Some("()")
            && node.return_type.as_deref() == Some("void")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "_load_items" && node.parent_name.is_none()
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "get_item"
            && node.params.as_deref() == Some("(idx: int)")
            && node.return_type.as_deref() == Some("Item")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "sample.gd" && edge.target == "Node"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.gd" && edge.target == "preload"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.gd::_ready"
            && edge.target == "sample.gd::_load_items"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.gd::_ready" && edge.target == "start"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.gd::_load_items" && edge.target == "append"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "sample.gd::Item"
            && edge.target == "sample.gd::Item.promote"
    }));
}

#[test]
fn parses_r_functions_classes_imports_calls_and_bridges() {
    let source = br#"library(dplyr)
require(ggplot2)
source("utils.R")

add <- function(x, y) {
  x + y
}

multiply = function(a, b) {
  a * b
}

MyClass <- setRefClass("MyClass",
  fields = list(name = "character", age = "numeric"),
  methods = list(
    greet = function() {
      cat(paste("Hello", name))
    },
    get_age = function() {
      return(age)
    }
  )
)

process_data <- function(data) {
  result <- dplyr::filter(data, x > 5)
  summary <- dplyr::summarize(result, mean_x = mean(x))
  add(1, 2)
  summary
}
"#;
    let (nodes, edges) = parse_r("sample.R", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "add" && node.params.as_deref() == Some("(x, y)")
    }));
    assert!(nodes
        .iter()
        .any(|node| { node.kind == "Class" && node.name == "MyClass" && node.language == "r" }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "greet"
            && node.parent_name.as_deref() == Some("MyClass")
    }));
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "dplyr" }));
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "utils.R" }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.R::process_data"
            && edge.target == "dplyr::filter"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.R::process_data"
            && edge.target == "sample.R::add"
    }));

    let bridge_source = br#"system("./target/release/dagayn-core build .")
system2("./scripts/build.sh", args = c("--strict"))
.Call("dagayn_compute")
.External("dagayn_helper")
dyn.load("./target/release/libdagayn.so")
library.dynam("dagayn", "./target/release")

run_dynamic <- function(cmd) {
  system(cmd)
}
"#;
    let (_nodes, bridge_edges) = parse_r("bridge.R", bridge_source);
    let cross_edges = bridge_edges
        .iter()
        .filter(|edge| edge.kind == "CROSS_ARTIFACT")
        .collect::<Vec<_>>();
    assert_eq!(cross_edges.len(), 7);
    assert!(cross_edges.iter().any(|edge| {
        edge.target == "./target/release/libdagayn.so"
            && edge.extra["evidence_source"] == "dyn.load"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(cross_edges.iter().any(|edge| {
        edge.target == "<dynamic:system@bridge.R:9>"
            && edge.extra["evidence_source"] == "system"
            && edge.extra["confidence_tier"] == "LOW"
    }));
}

#[test]
fn parses_julia_modules_types_functions_macros_and_bridges() {
    let source = br#"module SampleModule

using LinearAlgebra
using Statistics: mean, std
import Base: show, print
import JSON

export greet, Dog, process
public square, add

@enum Color RED BLUE GREEN

abstract type AbstractAnimal end

struct Dog <: AbstractAnimal
    name::String
    age::Int
end

mutable struct MutablePoint
    x::Float64
    y::Float64
end

function greet(name::String)
    println("Hello, $name")
end

function Base.show(io::IO, d::Dog)
    print(io, "Dog($(d.name))")
end

add(a, b) = a + b

square(x) = x^2

macro sayhello(name)
    :(println("Hello, ", $name))
end

function outer()
    function inner()
        return 1
    end
    x = inner()
    result = map(v -> v^2, [1,2,3])
    return x
end

function process(data::Vector{Float64}; verbose=false)
    if verbose
        println("Processing...")
    end
    normed = data ./ maximum(data)
    return sum(normed) / length(normed)
end

include("utils.jl")

@testset "Arithmetic" begin
    @test add(1, 2) == 3
    @test square(4) == 16
end

end # module
"#;
    let (nodes, edges) = parse_julia("sample.jl", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "SampleModule" && node.language == "julia"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Color" && node.extra["julia_kind"] == "enum"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "GREEN"
            && node.parent_name.as_deref() == Some("Color")
            && node.extra["julia_kind"] == "enum_variant"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "AbstractAnimal"
            && node.extra["type_role"] == "abstract_type"
            && node.extra["is_abstract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "show"
            && node.parent_name.as_deref() == Some("SampleModule")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "inner"
            && node.parent_name.as_deref() == Some("SampleModule.outer")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Test"
            && node.name.starts_with("testset:Arithmetic@L")
            && node.parent_name.as_deref() == Some("SampleModule")
    }));
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "Statistics.mean" }));
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "utils.jl" }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "sample.jl::SampleModule.Dog"
            && edge.target == "AbstractAnimal"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "sample.jl::SampleModule.show"
            && edge.target == "Base"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.jl::SampleModule.outer"
            && edge.target == "sample.jl::SampleModule.outer.inner"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge
                .source
                .starts_with("sample.jl::SampleModule.testset:Arithmetic@L")
            && edge.target == "sample.jl::SampleModule.add"
    }));

    let bridge_source = br#"function run_command()
    run(`git status`)
end

function read_config()
    open("config.yaml", "r")
end

function write_output()
    write("output.json", "{}")
end

function load_lib()
    Libdl.dlopen("mylib.so")
end
"#;
    let (_nodes, bridge_edges) = parse_julia("bridge.jl", bridge_source);
    assert!(bridge_edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.extra["evidence_source"] == "run"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(bridge_edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.so"
            && edge.extra["evidence_source"] == "Libdl.dlopen"
    }));
}

#[test]
fn parses_perl_packages_subroutines_imports_calls_and_bridges() {
    let source = br#"use strict;
use warnings;
use File::Basename;

package Animal;

sub new {
    my ($class, %args) = @_;
    return bless \%args, $class;
}

sub speak {
    my ($self) = @_;
    return "...";
}

package Dog;

sub new {
    my ($class, %args) = @_;
    my $self = Animal::new($class, %args);
    return $self;
}

sub fetch {
    my ($self, $item) = @_;
    return "Fetched $item";
}

sub bark {
    my ($self) = @_;
    print $self->speak() . "\n";
}
"#;
    let (nodes, edges) = parse_perl("sample.pl", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Animal"
            && node.language == "perl"
            && node.extra["type_role"] == "class"
    }));
    assert!(nodes
        .iter()
        .any(|node| { node.kind == "Class" && node.name == "Dog" }));
    assert!(nodes
        .iter()
        .any(|node| { node.kind == "Function" && node.name == "bark" }));
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "use strict;" }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.pl::new" && edge.target == "bless"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.pl::bark"
            && edge.target == "sample.pl::speak"
    }));

    let bridge_source = br#"sub run_command {
    system("git status");
}

sub read_config {
    open(my $fh, '<', "config.yaml") or die;
    return $fh;
}

sub run_dynamic {
    my ($cmd) = @_;
    system($cmd);
}
"#;
    let (_nodes, bridge_edges) = parse_perl("bridge.pl", bridge_source);
    assert!(bridge_edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "system"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(bridge_edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:open@bridge.pl:6>"
            && edge.extra["evidence_source"] == "open"
            && edge.extra["confidence_tier"] == "LOW"
    }));
}

#[test]
fn parses_vue_script_blocks_with_typescript_offsets() {
    let source = br#"<template>
  <div class="app">
    <UserList :users="users" @select="onSelectUser" />
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import UserList from './UserList.vue'

interface User {
  id: number
  name: string
}

const count = ref(0)

function increment() {
  count.value++
}

function onSelectUser(user: User) {
  console.log(user.name)
}

const doubled = computed(() => count.value * 2)
</script>
"#;
    let (nodes, edges) = parse_vue("sample.vue", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "File" && node.name == "sample.vue" && node.language == "vue"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.language == "vue"
            && node.extra["type_role"] == "interface"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "increment"
            && node.language == "vue"
            && node.line_start == 18
    }));
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "vue" && edge.line == 8 }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.vue" && edge.target == "ref"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.vue::onSelectUser"
            && edge.target == "log"
            && edge.line == 23
    }));
}

#[test]
fn parses_svelte_script_blocks_with_typescript_offsets() {
    let source = br#"<script lang="ts">
import { writable } from 'svelte/store'

interface User {
  name: string
}

const count = writable(0)

function increment() {
  console.log('increment')
}

function selectUser(user: User) {
  return user.name
}
</script>

<button on:click={increment}>{$count}</button>
"#;
    let (nodes, edges) = parse_svelte("sample.svelte", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "File" && node.name == "sample.svelte" && node.language == "svelte"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.language == "svelte"
            && node.extra["type_role"] == "interface"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "increment"
            && node.language == "svelte"
            && node.line_start == 10
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.target == "svelte/store" && edge.line == 2
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.svelte" && edge.target == "writable"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.svelte::increment"
            && edge.target == "log"
            && edge.line == 11
    }));
}

#[test]
fn parses_zig_file_without_extra_nodes_for_python_parity() {
    let source = br#"const std = @import("std");

pub fn main() void {
    std.debug.print("hello\n", .{});
}
"#;
    let (nodes, edges) = parse_zig("src/main.zig", source);

    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].kind, "File");
    assert_eq!(nodes[0].name, "src/main.zig");
    assert_eq!(nodes[0].language, "zig");
    assert_eq!(nodes[0].line_start, 1);
    assert_eq!(nodes[0].line_end, 6);
    assert!(edges.is_empty());
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
fn parses_perl_xs_as_c_for_python_parity() {
    let source = br#"#include "EXTERN.h"
#include "perl.h"
#include "XSUB.h"
#include <string.h>

typedef struct {
    int x;
    int y;
} Point;

static int
_add(int a, int b) {
    return a + b;
}

static double
compute_distance(int x1, int y1, int x2, int y2) {
    return _add(x1, x2);
}

MODULE = MyModule  PACKAGE = MyModule

int
add(a, b)
    int a
    int b
  CODE:
    RETVAL = _add(a, b);
  OUTPUT:
    RETVAL
"#;
    let (nodes, edges) = parse_rust_owned_file("MyModule.xs", source);

    assert!(nodes
        .iter()
        .any(|node| node.kind == "Class" && node.name == "Point"));
    assert!(nodes
        .iter()
        .any(|node| node.kind == "Function" && node.name == "_add"));
    assert!(nodes
        .iter()
        .any(|node| node.kind == "Function" && node.name == "compute_distance"));
    assert!(edges
        .iter()
        .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "XSUB.h"));
    assert!(edges
        .iter()
        .any(|edge| edge.kind == "CALLS" && edge.target.ends_with("::_add")));
}

#[test]
fn parses_c_header_as_c_for_python_parity() {
    let source = br#"#ifndef USER_H
#define USER_H
#include <stdint.h>

typedef struct {
    int id;
} User;

static inline int user_id(User *user) {
    return user->id;
}

#endif
"#;
    let (nodes, edges) = parse_rust_owned_file("include/user.h", source);

    assert!(nodes
        .iter()
        .any(|node| node.kind == "File" && node.language == "c"));
    assert!(nodes
        .iter()
        .any(|node| node.kind == "Class" && node.name == "User"));
    assert!(nodes
        .iter()
        .any(|node| node.kind == "Function" && node.name == "user_id"));
    assert!(edges
        .iter()
        .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "stdint.h"));
}

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
    assert!(nodes
        .iter()
        .any(|node| node.kind == "Function" && node.name == "save"));
    assert!(edges
        .iter()
        .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "import Foundation"));
    assert!(edges
        .iter()
        .any(|edge| edge.kind == "CALLS" && edge.target == "print"));
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

#[test]
fn parses_c_structs_functions_imports_calls_and_bridges() {
    let source = br#"#include <stdio.h>
#include <dlfcn.h>

typedef struct {
    int id;
} User;

User* create_user(void) {
    return malloc(sizeof(User));
}

void print_user(User* user) {
    printf("%d", user->id);
}

void run_command(const char *cmd) {
    system("git status");
    fopen("config.yaml", "r");
    dlopen("mylib.so", RTLD_NOW);
    system(cmd);
}

int main() {
    User* u = create_user();
    print_user(u);
    return 0;
}
"#;
    let (nodes, edges) = parse_c("sample.c", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.language == "c"
            && node.extra["type_role"] == "class"
    }));
    assert!(nodes
        .iter()
        .any(|node| { node.kind == "Function" && node.name == "create_user" }));
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "stdio.h" }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.c::main"
            && edge.target == "sample.c::create_user"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "sample.c::run_command"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "system"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "config.yaml"
            && edge.extra["relationship_role"] == "opens_file"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.so"
            && edge.extra["bridge_kind"] == "ffi"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:system@sample.c:20>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
}

#[test]
fn parses_cpp_classes_inheritance_functions_imports_calls_and_bridges() {
    let source = br#"#include <iostream>
#include <cstdlib>

class Animal {
public:
    Animal() {}
};

class Dog : public Animal {
public:
    Dog() : Animal() {}
    void speak() {}
};

void greet(const Animal& animal) {}

int main() {
    Dog d;
    d.speak();
    greet(d);
    return 0;
}

void run_command() {
    std::system("git status");
}
"#;
    let (nodes, edges) = parse_cpp("sample.cpp", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Animal"
            && node.language == "cpp"
            && node.extra["type_role"] == "class"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "Dog" && node.parent_name.as_deref() == Some("Dog")
    }));
    assert!(edges
        .iter()
        .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "iostream" }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS" && edge.source == "sample.cpp::Dog" && edge.target == "Animal"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.cpp::main" && edge.target == "speak"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.cpp::main"
            && edge.target == "sample.cpp::greet"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "sample.cpp::run_command"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "std::system"
            && edge.extra["source_language"] == "cpp"
    }));
}

#[test]
fn parses_objc_classes_methods_imports_messages_and_c_functions() {
    let source = br#"#import <Foundation/Foundation.h>
#import "Logger.h"

@interface Calculator : NSObject
- (NSInteger)add:(NSInteger)a to:(NSInteger)b;
@end

@implementation Calculator

- (NSInteger)add:(NSInteger)a to:(NSInteger)b {
    NSInteger sum = a + b;
    [self logResult:sum];
    return sum;
}

- (void)logResult:(NSInteger)value {
    NSLog(@"Result: %ld", (long)value);
}

+ (Calculator *)sharedCalculator {
    return [[Calculator alloc] init];
}

@end

int main(int argc, const char * argv[]) {
    Calculator *calc = [Calculator sharedCalculator];
    NSInteger r = [calc add:3 to:4];
    NSLog(@"Final: %ld", (long)r);
    return 0;
}
"#;
    let (nodes, edges) = parse_objc("sample.m", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Calculator" && node.language == "objc"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "add"
            && node.parent_name.as_deref() == Some("Calculator")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "main" && node.parent_name.is_none()
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.target == "#import <Foundation/Foundation.h>"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.m::Calculator.add"
            && edge.target == "sample.m::Calculator.logResult"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.m::main"
            && edge.target == "sample.m::Calculator.sharedCalculator"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.m::main"
            && edge.target == "sample.m::Calculator.add"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.m::main" && edge.target == "NSLog"
    }));
}

#[test]
fn parses_rust_items_imports_and_calls() {
    let source = br#"pub use dagayn_graph::{GraphStore};
use std::fs;

#[derive(Serialize, Deserialize)]
pub struct Foo {
    value: i32,
}

pub enum Mode {
    Fast,
}

impl Foo {
    pub fn new() -> Self {
        Self { value: 1 }
    }

    fn load(&self) {
        fs::read("path");
        consume(helper);
        helper();
    }
}

fn consume(_f: fn()) {}
fn helper() {}
"#;
    let (nodes, edges) = parse_rust("src/lib.rs", source);
    let node_names = nodes
        .iter()
        .map(|node| {
            (
                node.kind.as_str(),
                node.name.as_str(),
                node.parent_name.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert!(node_names.contains(&("File", "src/lib.rs", None)));
    assert!(node_names.contains(&("Class", "Foo", None)));
    assert!(node_names.contains(&("Class", "Mode", None)));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Foo"
            && node.extra["type_role"] == "struct"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
            && node.modifiers.as_deref() == Some("pub")
            && node.extra["derive_traits"] == serde_json::json!(["Serialize", "Deserialize"])
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Mode"
            && node.extra["type_role"] == "enum"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(node_names.contains(&("Function", "new", Some("Foo"))));
    assert!(node_names.contains(&("Function", "load", Some("Foo"))));
    assert!(node_names.contains(&("Function", "helper", None)));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "src/lib.rs" && edge.target == "std::fs"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "src/lib.rs"
            && edge.target == "pub dagayn_graph::{GraphStore}"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/lib.rs::Foo.load"
            && edge.target == "src/lib.rs::helper"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "src/lib.rs::Foo.load"
            && edge.target == "src/lib.rs::helper"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "src/lib.rs::Foo.load"
            && edge.target == "path"
    }));
}

#[test]
fn parses_python_items_imports_and_calls() {
    let source = br#"from models import User
import os

class Service(Base):
    def run(self, name: str) -> User:
        helper(name)
        os.getenv("ENV")

def helper(value: str) -> None:
    print(value)
"#;
    let (nodes, edges) = parse_python("app.py", source);
    let node_names = nodes
        .iter()
        .map(|node| {
            (
                node.kind.as_str(),
                node.name.as_str(),
                node.parent_name.as_deref(),
                node.params.as_deref(),
                node.return_type.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert!(node_names.contains(&("File", "app.py", None, None, None)));
    assert!(node_names.contains(&("Class", "Service", None, None, None)));
    assert!(node_names.contains(&(
        "Function",
        "run",
        Some("Service"),
        Some("(self, name: str)"),
        Some("User")
    )));
    assert!(node_names.contains(&(
        "Function",
        "helper",
        None,
        Some("(value: str)"),
        Some("None")
    )));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "app.py" && edge.target == "models"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "app.py" && edge.target == "os"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS" && edge.source == "app.py::Service" && edge.target == "Base"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "app.py::Service.run"
            && edge.target == "app.py::helper"
    }));
}

#[test]
fn parses_typescript_items_calls_tests_and_references() {
    let source = br#"import { Thing } from './thing';

interface Shape {
  id: string;
}

type UserPayload = {
  id: string;
  name: string;
};

enum UserStatus {
  active,
  disabled,
}

@Entity()
class UserModel {
  id!: string;
  name!: string;
}

class CardProps {
  title!: string;
  count?: number;
}

class UpdateUserDto {
  id!: string;
}

class Service extends Base {
  run(input: string): void {
    helper(input);
  }
}

function helper(value: string): void {
  console.log(value);
}

const indirect = { helper };
const callbacks = [helper];

describe('Service', () => {
  it('runs', () => {
    helper('x');
  });
});
"#;
    let (nodes, edges) = parse_javascript_like("service.test.ts", source, "typescript");
    let node_names = nodes
        .iter()
        .map(|node| {
            (
                node.kind.as_str(),
                node.name.as_str(),
                node.parent_name.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert!(node_names.contains(&("Class", "Shape", None)));
    assert!(node_names.contains(&("Class", "UserPayload", None)));
    assert!(node_names.contains(&("Class", "UserStatus", None)));
    assert!(node_names.contains(&("Class", "UserModel", None)));
    assert!(node_names.contains(&("Class", "CardProps", None)));
    assert!(node_names.contains(&("Class", "UpdateUserDto", None)));
    assert!(node_names.contains(&("Class", "Service", None)));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Shape"
            && node.extra["type_role"] == "interface"
            && node.extra["is_contract"] == true
    }));
    for name in [
        "UserPayload",
        "UserStatus",
        "UserModel",
        "CardProps",
        "UpdateUserDto",
    ] {
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == name
                && node.extra["container_role"] == "data_container"
                && node.extra["value_semantics"] == true
        }));
    }
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "UserPayload"
            && node.extra["type_role"] == "type_alias"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "UserStatus" && node.extra["type_role"] == "enum"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Service"
            && node.extra["type_role"] == "class"
            && node.extra.get("container_role").is_none()
    }));
    assert!(node_names
        .iter()
        .any(|(_, name, parent)| *name == "run" && *parent == Some("Service")));
    assert!(node_names
        .iter()
        .any(|(_, name, parent)| *name == "helper" && parent.is_none()));
    assert!(node_names
        .iter()
        .any(|(kind, name, _)| *kind == "Test" && name.starts_with("it:runs@L")));
    assert!(edges
        .iter()
        .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "./thing"));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "service.test.ts::Service"
            && edge.target == "Base"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "service.test.ts::Service.run"
            && edge.target == "service.test.ts::helper"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "service.test.ts"
            && edge.target == "service.test.ts::helper"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "TESTED_BY"
            && edge.source == "service.test.ts::helper"
            && edge.target.contains("it:runs")
    }));
}

#[test]
fn resolves_typescript_imported_call_targets() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-import-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src")).unwrap();
    std::fs::write(
        repo_root.join("src/helper.ts"),
        b"export function helper() { return 1; }\n",
    )
    .unwrap();

    let source = br#"import { helper } from './helper';

export function run() {
  helper();
  const refs = [helper];
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/consumer.ts", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/consumer.ts::run"
            && edge.target == "src/helper.ts::helper"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "src/consumer.ts::run"
            && edge.target == "src/helper.ts::helper"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_typescript_tsconfig_alias_imports() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-alias-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/lib")).unwrap();
    std::fs::write(
        repo_root.join("tsconfig.json"),
        br#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@/*": ["src/*"],
    },
  },
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo_root.join("src/lib/utils.ts"),
        b"export function cn(...args: string[]): string { return args.join(' '); }\n",
    )
    .unwrap();

    let source = br#"import { cn } from '@/lib/utils';

export function formatUser(name: string): string {
  return cn('user', name);
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "alias_importer.ts", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "alias_importer.ts"
            && edge.target == "src/lib/utils.ts"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "alias_importer.ts::formatUser"
            && edge.target == "src/lib/utils.ts::cn"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_typescript_barrel_reexports_to_origin() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-barrel-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/components")).unwrap();
    std::fs::write(
        repo_root.join("src/components/MarkdownMsg.ts"),
        b"export function MarkdownMsg() { return 'ok'; }\n",
    )
    .unwrap();
    std::fs::write(
        repo_root.join("src/components/index.ts"),
        b"export { MarkdownMsg as Msg } from './MarkdownMsg';\n",
    )
    .unwrap();

    let source = br#"import { Msg } from './components';

export function render() {
  return Msg();
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.ts", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/app.ts::render"
            && edge.target == "src/components/MarkdownMsg.ts::MarkdownMsg"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_typescript_star_barrel_reexports_to_origin() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-star-barrel-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/components")).unwrap();
    std::fs::write(
        repo_root.join("src/components/MarkdownMsg.ts"),
        b"export function MarkdownMsg() { return 'ok'; }\n",
    )
    .unwrap();
    std::fs::write(
        repo_root.join("src/components/index.ts"),
        b"export * from './MarkdownMsg';\n",
    )
    .unwrap();

    let source = br#"import { MarkdownMsg } from './components';

export function render() {
  return MarkdownMsg();
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.ts", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/app.ts::render"
            && edge.target == "src/components/MarkdownMsg.ts::MarkdownMsg"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_tsx_jsx_component_calls() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-tsx-jsx-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(&repo_root).unwrap();
    std::fs::write(
        repo_root.join("MarkdownMsg.tsx"),
        b"export function MarkdownMsg() { return <div />; }\n",
    )
    .unwrap();

    let source = br#"import MarkdownMsg from './MarkdownMsg';

export function BookWorkspace() {
  return <section><MarkdownMsg text={value} /></section>;
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "BookWorkspace.tsx", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "BookWorkspace.tsx::BookWorkspace"
            && edge.target == "MarkdownMsg.tsx::MarkdownMsg"
    }));
    assert!(!edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && (edge.target == "section" || edge.target == "div" || edge.target == "span")
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_tsx_namespace_component_calls() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-tsx-namespace-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(&repo_root).unwrap();
    std::fs::write(
        repo_root.join("MarkdownMsg.tsx"),
        b"export function MarkdownMsg() { return <div />; }\n",
    )
    .unwrap();

    let source = br#"import * as UI from './MarkdownMsg';

export function BookWorkspace() {
  return <UI.Messages.MarkdownMsg text={value} />;
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "BookWorkspace.tsx", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "BookWorkspace.tsx::BookWorkspace"
            && edge.target == "MarkdownMsg.tsx::MarkdownMsg"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_jsx_component_calls() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-jsx-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(&repo_root).unwrap();
    std::fs::write(
        repo_root.join("MarkdownMsg.jsx"),
        b"export function MarkdownMsg() { return <div />; }\n",
    )
    .unwrap();

    let source = br#"import { MarkdownMsg } from './MarkdownMsg';

export function BookWorkspace() {
  return <MarkdownMsg text={value} />;
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "BookWorkspace.jsx", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "BookWorkspace.jsx::BookWorkspace"
            && edge.target == "MarkdownMsg.jsx::MarkdownMsg"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_javascript_cross_artifact_edges() {
    let source = br#"child_process.spawn("./bin/tool", ["--flag"]);

function runDynamic(cmd) {
  child_process.exec(cmd);
}
"#;
    let (nodes, edges) = parse_javascript_like("bridge.js", source, "javascript");
    assert!(nodes
        .iter()
        .any(|node| node.kind == "Function" && node.name == "runDynamic"));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "bridge.js"
            && edge.target == "./bin/tool"
            && edge.extra["evidence_source"] == "child_process.spawn"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "bridge.js::runDynamic"
            && edge.target == "<dynamic:child_process.exec@bridge.js:4>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
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
    assert!(results
        .iter()
        .any(|item| item["file_path"] == "docs/README.md"));

    let _ = std::fs::remove_dir_all(&repo_root);
}
