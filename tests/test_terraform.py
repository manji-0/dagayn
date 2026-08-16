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


class TestTerraformCodeBridges:
    """CROSS_ARTIFACT bridges from Terraform resources to application code."""

    def setup_method(self):
        self.parser = CodeParser()

    def _cross_artifact(self, edges):
        return [e for e in edges if e.kind == "CROSS_ARTIFACT"]

    def test_aws_lambda_filename_and_handler_bridges(self):
        _, edges = self.parser.parse_bytes(
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
        ca = self._cross_artifact(edges)
        filename = next(
            e
            for e in ca
            if e.extra.get("evidence_source") == "filename"
            and e.extra.get("relationship_role") == "maps_entrypoint"
        )
        assert filename.source == "infra/main.tf::resource.aws_lambda_function.auth"
        assert filename.target == "app/hello.py"
        assert filename.extra["confidence_tier"] == "HIGH"
        assert filename.extra["bridge_kind"] == "manifest_link"
        assert filename.extra["source_language"] == "terraform"
        assert filename.extra["target_language"] == "python"

        handler = next(e for e in ca if e.extra.get("evidence_source") == "handler")
        assert handler.target == "<unresolved:hello.main>"
        assert handler.extra["original_symbol_name"] == "hello.main"
        assert handler.extra["confidence_tier"] == "HIGH"
        assert handler.extra["relationship_role"] == "maps_entrypoint"

    def test_gcp_source_directory_and_entry_point_bridges(self):
        _, edges = self.parser.parse_bytes(
            Path("infra/main.tf"),
            b"""
resource "google_cloudfunctions_function" "api" {
  name                = "api"
  runtime             = "python312"
  entry_point         = "serve"
  source_directory    = "${path.module}/../app"
  available_memory_mb = 128
  trigger_http        = true
}
""",
        )
        ca = self._cross_artifact(edges)
        source_dir = next(e for e in ca if e.extra.get("evidence_source") == "source_directory")
        assert source_dir.target == "app"
        assert source_dir.extra["relationship_role"] == "maps_entrypoint"
        assert source_dir.extra["confidence_tier"] == "HIGH"

        entry = next(e for e in ca if e.extra.get("evidence_source") == "entry_point")
        assert entry.target == "<unresolved:serve>"
        assert entry.extra["original_symbol_name"] == "serve"

    def test_local_exec_and_archive_file_path_bridges(self):
        _, edges = self.parser.parse_file(
            FIXTURES / "terraform_cross_artifact" / "infra" / "main.tf"
        )
        ca = self._cross_artifact(edges)

        local_exec = next(
            e for e in ca if e.extra.get("evidence_source") == "provisioner.local-exec.command"
        )
        assert local_exec.extra["relationship_role"] == "invokes_binary"
        assert local_exec.extra["bridge_kind"] == "subprocess"
        assert local_exec.target.endswith("scripts/bootstrap.py")
        assert local_exec.extra["confidence_tier"] == "HIGH"

        archive = next(e for e in ca if e.extra.get("evidence_source") == "source_dir")
        assert archive.source.endswith("::data.archive_file.hello_zip")
        assert archive.target.endswith("app")

    def test_rejects_remote_uri_filename(self):
        _, edges = self.parser.parse_bytes(
            Path("infra/main.tf"),
            b"""
resource "aws_lambda_function" "remote" {
  filename = "s3://bucket/key.zip"
  handler  = "index.handler"
  runtime  = "nodejs18.x"
  role     = "arn:aws:iam::123456789012:role/lambda"
}
""",
        )
        ca = self._cross_artifact(edges)
        assert not any(e.extra.get("evidence_source") == "filename" for e in ca)
        assert any(e.extra.get("evidence_source") == "handler" for e in ca)
