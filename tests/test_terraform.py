from pathlib import Path

import pytest

from dagayn.parser import CodeParser

pytest.importorskip("tree_sitter")

FIXTURES = Path(__file__).parent / "fixtures"


class TestTerraformParsing:
    def setup_method(self):
        self.parser = CodeParser()
        self.nodes, self.edges = self.parser.parse_file(FIXTURES / "sample_terraform.tf")

    def test_detects_language(self):
        assert self.parser.detect_language(Path("main.tf")) == "terraform"
        assert self.parser.detect_language(Path("terraform.tfvars")) == "terraform"

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
