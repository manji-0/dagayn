use super::*;

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
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "merge")
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "main.tf::resource.aws_vpc.main"
            && edge.target == "main.tf::data.aws_caller_identity.current"
    }));
}

#[test]
fn terraform_reference_and_call_sources_use_node_qualified_names() {
    let source = br#"locals {
  bucket_name = lower("logs")
}

resource "aws_s3_bucket" "logs" {
  bucket = local.bucket_name
  tags = {
    Self = aws_s3_bucket.logs.id
  }
}

output "bucket_arn" {
  value = aws_s3_bucket.logs.arn
}
"#;
    let (nodes, edges) = parse_terraform("infra/main.tf", source);
    let qualified = nodes
        .iter()
        .map(|node| {
            if node.kind == "File" {
                node.file_path.to_string()
            } else {
                format!("{}::{}", node.file_path, node.name)
            }
        })
        .collect::<HashSet<_>>();
    let flow_edges = edges
        .iter()
        .filter(|edge| matches!(edge.kind, EdgeKind::References | EdgeKind::Calls))
        .collect::<Vec<_>>();
    assert!(!flow_edges.is_empty());
    for edge in &flow_edges {
        assert!(
            qualified.contains(&edge.source),
            "{:?} source {:?} is not a node qualified name",
            edge.kind,
            edge.source
        );
    }
    assert!(flow_edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "infra/main.tf::local.bucket_name"
            && edge.target == "lower"
    }));
    assert!(flow_edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "infra/main.tf::resource.aws_s3_bucket.logs"
            && edge.target == "infra/main.tf::local.bucket_name"
    }));
    assert!(flow_edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "infra/main.tf::output.bucket_arn"
            && edge.target == "infra/main.tf::resource.aws_s3_bucket.logs"
    }));
    assert!(
        !flow_edges
            .iter()
            .any(|edge| edge.kind == "REFERENCES" && edge.source == edge.target),
        "a block referencing itself must not produce a self edge"
    );
}

#[test]
fn terraform_string_literals_are_not_references() {
    let source = br#"resource "aws_s3_bucket" "logs" {}

locals {
  greeting = "hello"
}

resource "aws_instance" "web" {
  instance_type = "t3.micro"
  filename      = "handler.zip"
  bucket_id     = "${aws_s3_bucket.logs.id}"
  user_data     = "bucket=${aws_s3_bucket.logs.arn} file=handler.zip ${var.prefix}.example"
  script        = <<-EOT
    echo config.json app.main
    echo ${local.greeting}
  EOT
}
"#;
    let (_nodes, edges) = parse_terraform("main.tf", source);
    let targets = edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::References && edge.source == "main.tf::resource.aws_instance.web"
        })
        .map(|edge| edge.target.as_str())
        .collect::<HashSet<_>>();
    for literal in [
        "resource.t3.micro",
        "resource.handler.zip",
        "resource.prefix.example",
        "resource.config.json",
        "resource.app.main",
    ] {
        assert!(
            !targets.contains(literal),
            "string literal text leaked as {literal}: {targets:?}"
        );
    }
    assert!(targets.contains("main.tf::resource.aws_s3_bucket.logs"));
    assert!(targets.contains("var.prefix"));
    assert!(targets.contains("main.tf::local.greeting"));
    assert_eq!(targets.len(), 3, "unexpected targets: {targets:?}");
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
