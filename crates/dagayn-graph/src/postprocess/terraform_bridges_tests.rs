use crate::*;

use std::path::PathBuf;

/// A repository on disk, parsed with the real parsers into a fresh graph.
struct Repo {
    root: PathBuf,
    db: PathBuf,
    store: GraphStore,
}

impl Repo {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("dagayn-tf-bridges-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let db = root.with_extension("db");
        let _ = std::fs::remove_file(&db);
        let mut store = GraphStore::open(&db).expect("open");
        store
            .set_metadata("repo_root", root.to_string_lossy().as_ref())
            .unwrap();
        let mut repo = Self { root, db, store };
        for (path, text) in files {
            repo.write(path, text);
        }
        repo
    }

    /// Write `path` and store what the parser makes of it.
    fn write(&mut self, path: &str, text: &str) {
        let file = self.root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        let (nodes, edges) = dagayn_parser::parse_rust_owned_file(path, text.as_bytes());
        let nodes: Vec<NodeInput> = nodes
            .into_iter()
            .map(|node| NodeInput {
                kind: node.kind.as_str().to_string(),
                name: node.name,
                file_path: node.file_path.as_str().to_string(),
                line_start: node.line_start,
                line_end: node.line_end,
                language: node.language,
                parent_name: node.parent_name,
                params: node.params,
                return_type: node.return_type,
                modifiers: node.modifiers,
                is_test: node.is_test,
                extra: node.extra,
            })
            .collect();
        let edges: Vec<EdgeInput> = edges
            .into_iter()
            .map(|edge| EdgeInput {
                kind: edge.kind.as_str().to_string(),
                source: edge.source,
                target: edge.target,
                file_path: edge.file_path.as_str().to_string(),
                line: edge.line,
                extra: edge.extra,
            })
            .collect();
        self.store
            .store_file_nodes_edges(path, &nodes, &edges, "hash", 0)
            .expect("store");
    }

    /// `(source, target, evidence_source, confidence_tier)` of every
    /// Terraform bridge after the step ran.
    fn bridges(&mut self) -> Vec<(String, String, String, String)> {
        self.store
            .resolve_terraform_artifact_refs()
            .expect("resolve");
        let mut stmt = self
            .store
            .conn
            .prepare(
                "SELECT source_qualified, target_qualified, \
                 json_extract(extra, '$.evidence_source'), confidence_tier FROM edges \
                 WHERE kind = 'CROSS_ARTIFACT' \
                 AND json_extract(extra, '$.source_language') = 'terraform' \
                 ORDER BY source_qualified, target_qualified",
            )
            .unwrap();
        stmt.query_map([], |row| <(String, String, String, String)>::try_from(row))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    }

    fn bridge_extra(&self, source: &str, target: &str) -> Value {
        let raw: String = self
            .store
            .conn
            .query_row(
                "SELECT extra FROM edges WHERE kind = 'CROSS_ARTIFACT' \
                 AND source_qualified = ? AND target_qualified = ?",
                [source, target],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str(&raw).unwrap()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_file(&self.db);
    }
}

fn row(source: &str, target: &str, evidence: &str, tier: &str) -> (String, String, String, String) {
    (
        source.to_string(),
        target.to_string(),
        evidence.to_string(),
        tier.to_string(),
    )
}

#[test]
fn resource_paths_and_a_unique_handler_become_bridges() {
    let mut repo = Repo::new(
        "resource",
        &[
            ("app/hello.py", "def main(event, context):\n    return 1\n"),
            (
                "infra/main.tf",
                "resource \"aws_lambda_function\" \"auth\" {\n  filename = \"${path.module}/../app/hello.py\"\n  handler  = \"hello.main\"\n}\n",
            ),
        ],
    );
    let lambda = "infra/main.tf::resource.aws_lambda_function.auth";
    assert_eq!(
        repo.bridges(),
        vec![
            row(lambda, "app/hello.py", "filename", "HIGH"),
            row(lambda, "app/hello.py::main", "handler", "HIGH"),
        ]
    );
    let extra = repo.bridge_extra(lambda, "app/hello.py");
    assert_eq!(extra["attr_lines"], json!([2, 2]));
    assert_eq!(extra["target_language"], "python");
    // Idempotent: a second run replaces the set with the same edges.
    assert_eq!(repo.bridges().len(), 2);
}

/// A trimmed terraform-aws-modules/lambda: `source_path` reaches the
/// packaging program through `data "external"`, `handler` reaches the
/// function through a conditional, `local_existing_package` through a local.
const LAMBDA_MODULE_MAIN: &str = r#"
locals {
  filename = var.local_existing_package != null ? var.local_existing_package : data.external.archive_prepare.result.filename
}

data "external" "archive_prepare" {
  program = ["python3", "${path.module}/package.py", "prepare"]
  query = {
    runtime     = var.runtime
    source_path = try(tostring(var.source_path), jsonencode(var.source_path))
  }
}

resource "aws_lambda_function" "this" {
  handler  = var.package_type != "Zip" ? null : var.handler
  runtime  = var.runtime
  filename = local.filename
}
"#;

const LAMBDA_MODULE_VARIABLES: &str = r#"
variable "source_path" {}
variable "handler" {}
variable "runtime" {}
variable "package_type" {}
variable "local_existing_package" {}
"#;

fn lambda_repo(name: &str, caller: &str) -> Repo {
    Repo::new(
        name,
        &[
            ("main.tf", LAMBDA_MODULE_MAIN),
            ("variables.tf", LAMBDA_MODULE_VARIABLES),
            ("package.py", "def prepare():\n    return 1\n"),
            (
                "examples/fixtures/app/index.py",
                "def lambda_handler(event, context):\n    return 1\n",
            ),
            // Another `index.lambda_handler`: only the call's own code can say
            // which one it runs.
            (
                "examples/fixtures/other/index.py",
                "def lambda_handler(event, context):\n    return 2\n",
            ),
            ("examples/simple/main.tf", caller),
        ],
    )
}

#[test]
fn module_inputs_bridge_through_the_module_to_the_code_they_name() {
    let mut repo = lambda_repo(
        "module-inputs",
        r#"
module "lambda" {
  source = "../../"

  handler      = "index.lambda_handler"
  runtime      = "python3.12"
  package_type = "Zip"
  source_path  = [
    "${path.module}/../fixtures/app",
    { path = "${path.module}/../fixtures/other/index.py", pip_requirements = false },
  ]
}
"#,
    );
    let call = "examples/simple/main.tf::module.lambda";
    let bridges = repo.bridges();
    let from_call: Vec<_> = bridges.iter().filter(|b| b.0 == call).cloned().collect();
    assert_eq!(
        from_call,
        vec![
            // Ambiguous across the two code paths this call names.
            row(call, "<unresolved:index.lambda_handler>", "handler", "HIGH"),
            row(call, "examples/fixtures/app", "source_path", "HIGH"),
            row(
                call,
                "examples/fixtures/other/index.py",
                "source_path",
                "HIGH"
            ),
        ],
    );
    let extra = repo.bridge_extra(call, "examples/fixtures/app");
    assert_eq!(extra["binding"], "module_input");
    assert_eq!(
        extra["consumers"],
        json!(["main.tf::data.external.archive_prepare.query"])
    );
    assert_eq!(extra["attr_lines"], json!([8, 11]));
    // The module's own program is a bridge of the module, not of the call.
    assert!(bridges.contains(&row(
        "main.tf::data.external.archive_prepare",
        "package.py",
        "program",
        "HIGH"
    )));
}

#[test]
fn a_module_call_handler_resolves_in_the_code_that_call_names() {
    let mut repo = lambda_repo(
        "module-handler",
        r#"
module "lambda" {
  source = "../../"

  handler     = "index.lambda_handler"
  source_path = "${path.module}/../fixtures/app"
}
"#,
    );
    let call = "examples/simple/main.tf::module.lambda";
    let bridges = repo.bridges();
    assert!(bridges.contains(&row(
        call,
        "examples/fixtures/app/index.py::lambda_handler",
        "handler",
        "HIGH"
    )));
    let modules: Vec<String> = {
        let mut stmt = repo
            .store
            .conn
            .prepare(
                "SELECT target_qualified FROM edges WHERE kind = 'IMPORTS_FROM' \
                 AND source_qualified = ? AND json_extract(extra, '$.extractor') = \
                 'terraform_modules' ORDER BY target_qualified",
            )
            .unwrap();
        stmt.query_map([call], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    };
    assert_eq!(modules, vec!["main.tf", "variables.tf"]);
}

#[test]
fn a_handler_whose_code_is_only_in_a_package_stays_unresolved() {
    let mut repo = lambda_repo(
        "module-package",
        r#"
module "lambda" {
  source = "../../"

  handler                = "index.lambda_handler"
  local_existing_package = "${path.module}/../fixtures/existing_package.zip"
}
"#,
    );
    let call = "examples/simple/main.tf::module.lambda";
    let from_call: Vec<_> = repo
        .bridges()
        .into_iter()
        .filter(|b| b.0 == call)
        .map(|b| (b.1, b.2))
        .collect();
    assert_eq!(
        from_call,
        vec![
            (
                "<unresolved:index.lambda_handler>".to_string(),
                "handler".to_string()
            ),
            (
                "examples/fixtures/existing_package.zip".to_string(),
                "local_existing_package".to_string()
            ),
        ]
    );
}

#[test]
fn values_pass_through_locals_and_nested_module_calls() {
    let mut repo = Repo::new(
        "nested",
        &[
            ("app/serve.py", "def serve(request):\n    return 1\n"),
            (
                "modules/core/main.tf",
                "locals {\n  entry = var.entry_point\n}\n\nresource \"google_cloudfunctions2_function\" \"fn\" {\n  build_config {\n    entry_point = local.entry\n  }\n}\n",
            ),
            (
                "modules/wrapper/main.tf",
                "module \"core\" {\n  source      = \"../core\"\n  entry_point = var.entrypoint\n}\n",
            ),
            (
                "examples/main.tf",
                "module \"wrapper\" {\n  source     = \"../modules/wrapper\"\n  entrypoint = \"serve\"\n  region     = \"serve\"\n}\n\nmodule \"remote\" {\n  source     = \"../../outside\"\n  entrypoint = \"serve\"\n}\n",
            ),
        ],
    );
    assert_eq!(
        repo.bridges(),
        vec![row(
            "examples/main.tf::module.wrapper",
            "app/serve.py::serve",
            "entrypoint",
            "HIGH"
        )]
    );
}
