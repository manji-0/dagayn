"""The package's module-level imports form a DAG.

Only imports executed at import time count: function-local imports and
``if TYPE_CHECKING:`` blocks are the sanctioned way to defer a dependency.
"""

from __future__ import annotations

import ast
from pathlib import Path

import networkx as nx

PACKAGE_ROOT = Path(__file__).resolve().parent.parent / "dagayn"


def _module_name(path: Path) -> str:
    parts = list(path.relative_to(PACKAGE_ROOT.parent).with_suffix("").parts)
    if parts[-1] == "__init__":
        parts.pop()
    return ".".join(parts)


def _is_type_checking(test: ast.expr) -> bool:
    return (isinstance(test, ast.Name) and test.id == "TYPE_CHECKING") or (
        isinstance(test, ast.Attribute) and test.attr == "TYPE_CHECKING"
    )


def _eager_imports(tree: ast.Module) -> list[ast.Import | ast.ImportFrom]:
    found: list[ast.Import | ast.ImportFrom] = []
    pending: list[ast.stmt] = list(tree.body)
    while pending:
        node = pending.pop()
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            found.append(node)
        elif isinstance(node, ast.If):
            if not _is_type_checking(node.test):
                pending.extend(node.body)
            pending.extend(node.orelse)
        elif isinstance(node, ast.Try):
            pending.extend(node.body + node.orelse + node.finalbody)
            for handler in node.handlers:
                pending.extend(handler.body)
    return found


def _import_graph() -> nx.DiGraph:
    modules = {_module_name(path): path for path in PACKAGE_ROOT.rglob("*.py")}
    graph = nx.DiGraph()
    for name, path in modules.items():
        is_package = path.name == "__init__.py"
        package = name if is_package else name.rpartition(".")[0]
        for node in _eager_imports(ast.parse(path.read_text(encoding="utf-8"))):
            if isinstance(node, ast.Import):
                targets = [alias.name for alias in node.names]
            else:
                if node.level:
                    base = package.rsplit(".", node.level - 1)[0] if node.level > 1 else package
                    base = f"{base}.{node.module}" if node.module else base
                else:
                    base = node.module or ""
                # `from pkg import sub` loads pkg.sub, not pkg's __init__ body.
                submodules = [f"{base}.{alias.name}" for alias in node.names]
                targets = [t for t in submodules if t in modules] or [base]
            for target in targets:
                if target in modules and target != name:
                    graph.add_edge(name, target)
    return graph


def test_module_level_imports_are_acyclic() -> None:
    cycles = list(nx.simple_cycles(_import_graph()))
    assert not cycles, "import cycles:\n" + "\n".join(" -> ".join(map(str, c)) for c in cycles[:20])
