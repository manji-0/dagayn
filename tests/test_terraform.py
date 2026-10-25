from pathlib import Path

from dagayn.parser import CodeParser

FIXTURES = Path(__file__).parent / "fixtures"


class TestTerraformParsing:
    def setup_method(self):
        self.parser = CodeParser()
        self.nodes, self.edges = self.parser.parse_file(FIXTURES / "sample_terraform.tf")

    def test_detects_language(self):
        assert self.parser.detect_language(Path("main.tf")) == "terraform"
        assert self.parser.detect_language(Path("terraform.tfvars")) == "terraform"

    def test_detects_compound_terraform_extensions(self):
        for name in (
            "main.tftest.hcl",
            "component.tfcomponent.hcl",
            "deploy.tfdeploy.hcl",
            "query.tfquery.hcl",
            "main.tf.json",
            "terraform.tfvars.json",
        ):
            assert self.parser.detect_language(Path(name)) == "terraform"
        assert self.parser.detect_language(Path("plain.hcl")) is None

    def test_parses_terraform_json_syntax(self):
        nodes, edges = self.parser.parse_bytes(
            Path("main.tf.json"),
            b"""{
  "resource": { "aws_vpc": { "main": { "cidr_block": "10.0.0.0/16" } } },
  "variable": { "region": { "default": "us-east-1" } },
  "module": { "network": { "source": "./modules/network" } },
  "output": { "vpc_id": { "value": "${aws_vpc.main.id}" } }
}""",
        )
        names = {node.name for node in nodes}
        assert "resource.aws_vpc.main" in names
        assert "var.region" in names
        assert "module.network" in names
        assert "output.vpc_id" in names
        assert any(e.kind == "IMPORTS_FROM" and e.target == "./modules/network" for e in edges)

    def test_tfvars_json_keeps_file_node(self):
        nodes, edges = self.parser.parse_bytes(
            Path("terraform.tfvars.json"),
            b"""{ "region": "us-east-1" }""",
        )
        assert len(nodes) == 1
        assert nodes[0].kind == "File"
        assert nodes[0].language == "terraform"
        assert edges == []

    def test_finds_terraform_nodes(self):
        names = {node.name for node in self.nodes}
        assert "terraform" in names
        assert "provider.aws" in names
        assert "var.tags" in names
        assert "local.common_tags" in names
        assert "module.network" in names
        assert "data.aws_caller_identity.current" in names
        assert "resource.aws_vpc.main" in names
        assert "output.vpc_id" in names
        assert "check.vpc_ready" in names

    def test_production_check_block_is_not_a_test(self):
        """Terraform 1.5+ `check` blocks run during plan/apply (#136)."""
        check = next(node for node in self.nodes if node.name == "check.vpc_ready")
        assert check.kind == "Class"
        assert check.is_test is False

    def test_extracts_dependency_and_module_edges(self):
        depends_on = [edge for edge in self.edges if edge.kind == "DEPENDS_ON"]
        imports = [edge for edge in self.edges if edge.kind == "IMPORTS_FROM"]

        assert any(edge.target == "hashicorp/aws" for edge in depends_on)
        assert any(edge.target == "./modules/network" for edge in imports)

    def test_extracts_function_calls(self):
        calls = [edge for edge in self.edges if edge.kind == "CALLS"]
        targets = {edge.target for edge in calls}
        assert "merge" in targets
        assert "length" in targets

    def test_extracts_cross_block_references(self):
        refs = [edge for edge in self.edges if edge.kind == "REFERENCES"]
        targets = {edge.target for edge in refs}

        assert any(target.endswith("::var.tags") for target in targets)
        assert any(target.endswith("::local.common_tags") for target in targets)
        assert any(target.endswith("::module.network") for target in targets)
        assert any(target.endswith("::data.aws_caller_identity.current") for target in targets)
        assert any(target.endswith("::resource.aws_vpc.main") for target in targets)

    def test_reference_and_call_sources_are_node_qualified_names(self):
        """REFERENCES/CALLS sources must match node qualified names.

        Bare sources such as ``resource.aws_vpc.main`` never match a node, so
        post-processing demoted every Terraform edge to LOW confidence.
        """
        qualified = {
            node.file_path if node.kind == "File" else f"{node.file_path}::{node.name}"
            for node in self.nodes
        }
        flow_edges = [edge for edge in self.edges if edge.kind in {"REFERENCES", "CALLS"}]
        assert flow_edges
        for edge in flow_edges:
            assert edge.source in qualified, (edge.kind, edge.source)

    def test_string_literals_are_not_references(self):
        """Only ``${ ... }`` interpolations inside strings and heredocs are references."""
        _, edges = self.parser.parse_bytes(
            Path("main.tf"),
            b"""resource "aws_s3_bucket" "logs" {}

resource "aws_instance" "web" {
  instance_type = "t3.micro"
  filename      = "handler.zip"
  user_data     = "bucket=${aws_s3_bucket.logs.id} file=handler.zip"
  script        = <<-EOT
    echo config.json ${var.prefix}
  EOT
}
""",
        )
        targets = {
            edge.target
            for edge in edges
            if edge.kind == "REFERENCES" and edge.source == "main.tf::resource.aws_instance.web"
        }
        assert targets == {"main.tf::resource.aws_s3_bucket.logs", "var.prefix"}


class TestTerraformCodeValues:
    """What the parser records for post-processing to bridge Terraform to code.

    The edges themselves are written by post-processing
    (tests/test_postprocessing.py and the Rust graph tests), which sees every
    file.
    """

    def setup_method(self):
        self.parser = CodeParser()

    def _values(self, nodes, name):
        node = next(n for n in nodes if n.name == name)
        return {value["attr"]: value for value in node.extra.get("terraform_values", [])}

    def test_aws_lambda_filename_and_handler(self):
        nodes, edges = self.parser.parse_bytes(
            Path("infra/main.tf"),
            b"""
resource "aws_lambda_function" "auth" {
  filename = "${path.module}/../app/hello.py"
  handler  = "hello.main"
  runtime  = "python3.12"
  role     = "arn:aws:iam::123456789012:role/lambda"
}
""",
        )
        assert not [e for e in edges if e.kind == "CROSS_ARTIFACT"]
        values = self._values(nodes, "resource.aws_lambda_function.auth")
        assert set(values) == {"filename", "handler"}
        assert values["filename"]["role"] == "code_path"
        assert values["filename"]["paths"] == ["app/hello.py"]
        assert values["filename"]["lines"] == [3, 3]
        assert values["handler"]["role"] == "entrypoint"
        assert values["handler"]["symbol"] == "hello.main"

    def test_gcp_source_directory_and_entry_point(self):
        nodes, _ = self.parser.parse_bytes(
            Path("infra/main.tf"),
            b"""
resource "google_cloudfunctions_function" "api" {
  name                = "api"
  entry_point         = "serve"
  source_directory    = "${path.module}/../app"
}
""",
        )
        values = self._values(nodes, "resource.google_cloudfunctions_function.api")
        assert values["source_directory"]["paths"] == ["app"]
        assert values["entry_point"]["symbol"] == "serve"

    def test_local_exec_and_archive_file_paths(self):
        nodes, _ = self.parser.parse_file(
            FIXTURES / "terraform_cross_artifact" / "infra" / "main.tf"
        )
        local_exec = next(
            value
            for node in nodes
            for value in node.extra.get("terraform_values", [])
            if value["attr"] == "provisioner.local-exec.command"
        )
        assert local_exec["role"] == "command"
        assert any(path.endswith("scripts/bootstrap.py") for path in local_exec["paths"])
        archive = self._values(nodes, "data.archive_file.hello_zip")
        assert archive["source_dir"]["paths"][0].endswith("app")

    def test_one_segment_module_directory_is_a_path(self):
        nodes, _ = self.parser.parse_bytes(
            Path("examples/server/main.tf"),
            b"""
data "archive_file" "source" {
  type        = "zip"
  source_dir  = "${path.module}/function"
  output_path = "function.zip"
}
""",
        )
        values = self._values(nodes, "data.archive_file.source")
        assert values["source_dir"]["paths"] == ["examples/server/function"]

    def test_rejects_remote_uri_filename(self):
        nodes, _ = self.parser.parse_bytes(
            Path("infra/main.tf"),
            b"""
resource "aws_lambda_function" "remote" {
  filename = "s3://bucket/key.zip"
  handler  = "index.handler"
}
""",
        )
        values = self._values(nodes, "resource.aws_lambda_function.remote")
        assert values["filename"]["paths"] == []
        assert values["handler"]["symbol"] == "index.handler"
