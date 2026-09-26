"""Regression tests for per-language parse misses."""

from pathlib import Path

import pytest

from dagayn.parser import CodeParser


def _parse(tmp_path: Path, name: str, source: str):
    path = tmp_path / name
    path.write_text(source)
    nodes, edges = CodeParser().parse_file(path)
    prefix = str(path)

    def short(value: str) -> str:
        return value.replace(prefix, "<f>")

    names = {(n.kind, short(n.name), n.parent_name) for n in nodes}
    edge_set = {(e.kind, short(e.source), short(e.target)) for e in edges}
    return names, edge_set, nodes


@pytest.fixture
def parse(tmp_path):
    return lambda name, source: _parse(tmp_path, name, source)


class TestPython:
    SOURCE = """\
def helper(x):
    return x

class Svc:
    handler = lambda self, x: helper(x)
    marker = helper(0)

    def run(self):
        def local():
            helper(2)
        local()

    class Inner:
        def go(self):
            helper(3)
"""

    def test_nested_definitions_are_qualified(self, parse):
        names, edges, _ = parse("m.py", self.SOURCE)
        assert ("Function", "go", "Svc.Inner") in names
        assert ("Function", "local", "Svc.run") in names
        assert ("CONTAINS", "<f>::Svc", "<f>::Svc.Inner") in edges
        assert ("CONTAINS", "<f>::Svc.Inner", "<f>::Svc.Inner.go") in edges
        assert ("CONTAINS", "<f>::Svc.run", "<f>::Svc.run.local") in edges
        assert ("CALLS", "<f>::Svc.run", "<f>::Svc.run.local") in edges

    def test_class_body_calls_use_class_source(self, parse):
        _, edges, _ = parse("m.py", self.SOURCE)
        assert ("CALLS", "<f>::Svc", "<f>::helper") in edges

    def test_lambda_assignment_is_function(self, parse):
        names, edges, nodes = parse("m.py", self.SOURCE)
        assert ("Function", "handler", "Svc") in names
        assert ("CALLS", "<f>::Svc.handler", "<f>::helper") in edges
        handler = next(n for n in nodes if n.name == "handler")
        assert handler.extra.get("python_kind") == "lambda"
