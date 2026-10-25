#!/usr/bin/env python3
"""Component principles on real repositories, read from their graphs.

Step 4 of docs/plans/COMPONENT-PRINCIPLES-TARGET.md#order-of-work. For each
repository this reads ``.dagayn/graph.db`` read-only (no tool change, no
build) and computes, per declared unit:

- **SAP**: instability ``I``, abstractness ``A`` (every top-level type, as
  ``sap_metrics`` counts it since #211), and distance ``D``. Units come
  from the manifests as ``crates/dagayn-tools/src/units.rs`` discovers them
  (``unit_scopes``: code files, tests included), and the edges are the
  ``strict_static`` set of ``architecture.rs``.
- **CRP** ``surface_used``: for each unit edge A -> B (the edge set of the
  overview's ``unit_edges``, production code only), the public symbols of B
  that A reaches through ``CALLS``, ``REFERENCES``, ``INHERITS``, or
  ``IMPLEMENTS``, over all public symbols of B. Public is Python names
  without a leading underscore at module or class top level, Rust
  functions, methods, and types whose ``modifiers`` is ``pub`` (declared
  visibility, rust extractor 25; ``pub(crate)`` and the other restricted
  forms are not public, and modules are not symbols), and
  TypeScript/JavaScript nodes flagged ``exported`` plus the non-private
  members of exported classes.
- **refused_member** (the LSP half): methods of a type that inherits or
  implements something, outside tests, whose body only refuses the call
  (``unimplemented!()``, ``todo!()``, a ``panic!`` saying "not
  implemented", ``raise NotImplementedError`` in a subclass override,
  ``throw new Error("not implemented")``).

When ``component_principles_judgments.yaml`` exists, every judged id must be
one of the extremes (or candidates) this run reports, and the hand verdicts
(actionable / false / noise) are tallied per measure.

    python eval/measure_component_principles.py --repo dagayn=PATH \\
        --repo cfwdon=PATH [--out DIR] [--markdown]
"""

from __future__ import annotations

import argparse
import json
import re
import sqlite3
import sys
import tomllib
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

HERE = Path(__file__).resolve().parent
JUDGMENTS = HERE / "component_principles_judgments.yaml"
VERDICTS = ("actionable", "false", "noise")
MEASURES = ("sap", "crp", "refused_member")
EXTREMES = 5
# A public surface smaller than this is too small for a low share to mean anything.
MIN_SURFACE = 10
# Zone cutoffs on A and I.
ZONE_LOW, ZONE_HIGH = 0.3, 0.7
MAX_REFUSED = 30
EXPECTED_SCHEMA = "20"

# crates/dagayn-tools/src/findings.rs
CODE_LANGUAGES = frozenset(
    "python rust typescript tsx javascript go java kotlin scala c cpp csharp objc swift "
    "ruby php perl lua dart elixir julia r zig gdscript bash vue".split()
)
TEST_PARTS = frozenset({"tests", "test", "__tests__", "spec", "testdata"})
NON_PRODUCTION_PARTS = frozenset(
    {"examples", "example", "benches", "bench", "fixtures", "generated", "vendor"}
)
# crates/dagayn-tools/src/units.rs
NON_UNIT_DIRS = frozenset(
    {
        "tests",
        "test",
        "__tests__",
        "testdata",
        "fixtures",
        "examples",
        "example",
        "benches",
        "bench",
        "vendor",
        "node_modules",
        "third_party",
    }
)
DOC_SUFFIXES = (".md", ".markdown", ".mdown", ".mkdn")
# crates/dagayn-tools/src/architecture.rs `sap_metrics`
NT_ROLES = frozenset(
    "class abstract_class interface protocol trait abstract_type mixin struct enum record "
    "typed_dict union error_set".split()
)
NA_ROLES = frozenset("abstract_class interface protocol trait abstract_type".split())
DEPENDENCY_KINDS = frozenset({"IMPORTS_FROM", "DEPENDS_ON", "INHERITS", "IMPLEMENTS"})
UNIT_EDGE_KINDS = frozenset({"CALLS", "IMPORTS_FROM", "REFERENCES", "INHERITS", "IMPLEMENTS"})
SURFACE_KINDS = frozenset({"CALLS", "REFERENCES", "INHERITS", "IMPLEMENTS"})
SYMBOL_KINDS = frozenset({"Function", "Class", "Type"})
TS_LANGUAGES = frozenset({"typescript", "tsx", "javascript"})


# -- graph -------------------------------------------------------------------


@dataclass
class Graph:
    root: Path
    meta: dict[str, str]
    nodes: list[dict[str, Any]]
    edges: list[dict[str, Any]]
    by_qn: dict[str, dict[str, Any]] = field(default_factory=dict)

    def __post_init__(self) -> None:
        self.by_qn = {node["qualified_name"]: node for node in self.nodes}

    def owner(self, node: dict[str, Any]) -> dict[str, Any] | None:
        if not node["parent_name"]:
            return None
        return self.by_qn.get(f"{node['file_path']}::{node['parent_name']}")


def load(root: Path) -> Graph:
    db = sqlite3.connect(f"file:{root}/.dagayn/graph.db?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    try:
        meta = {
            row["key"]: str(row["value"]) for row in db.execute("SELECT key, value FROM metadata")
        }
        nodes = []
        for row in db.execute(
            "SELECT kind, name, qualified_name, file_path, line_start, line_end, language, "
            "parent_name, modifiers, is_test, extra FROM nodes"
        ):
            node = dict(row)
            node["extra"] = json.loads(node["extra"] or "{}")
            nodes.append(node)
        edges = [
            dict(row)
            for row in db.execute(
                "SELECT kind, source_qualified, target_qualified, file_path, line, "
                "COALESCE(json_extract(extra, '$.external'), 0) AS external FROM edges"
            )
        ]
    finally:
        db.close()
    return Graph(root, meta, nodes, edges)


def check_meta(name: str, graph: Graph) -> list[str]:
    problems = []
    if graph.meta.get("last_build_type") != "full":
        problems.append(f"{name}: last_build_type={graph.meta.get('last_build_type')}")
    if graph.meta.get("schema_version") != EXPECTED_SCHEMA:
        problems.append(f"{name}: schema_version={graph.meta.get('schema_version')}")
    return problems


def is_test_path(rel: str) -> bool:
    name = rel.rsplit("/", 1)[-1]
    return (
        any(part in TEST_PARTS for part in rel.split("/"))
        or name.startswith("test_")
        or "_test." in name
        or ".test." in name
        or ".spec." in name
        or name == "conftest.py"
    )


def is_test_node(node: dict[str, Any]) -> bool:
    return bool(node["is_test"]) or node["kind"] == "Test" or is_test_path(node["file_path"])


def is_production(node: dict[str, Any]) -> bool:
    """`findings.rs::is_production_code`."""
    rel = node["file_path"]
    if is_test_node(node) or node["language"] not in CODE_LANGUAGES:
        return False
    name = rel.rsplit("/", 1)[-1]
    if name in ("build.rs", "setup.py", "conftest.py") or "_pb2" in name or ".generated." in name:
        return False
    return not any(part in NON_PRODUCTION_PARTS for part in rel.split("/"))


def is_production_scope(scope: str) -> bool:
    """`stability.rs::is_production_scope`."""
    return not is_test_path(f"{scope}/probe.rs") and "fixtures" not in scope.split("/")


# -- units (port of units.rs `UnitIndex::discover`) ----------------------------


@dataclass
class Unit:
    name: str
    kind: str
    path: str
    depends_on: list[str]


def parent(path: str) -> str:
    return path.rsplit("/", 1)[0] if "/" in path else ""


def _manifest(path: Path, parse: Any) -> dict[str, Any]:
    try:
        value = parse(path.read_text())
    except (OSError, ValueError):
        return {}
    return value if isinstance(value, dict) else {}


def declared_unit(root: Path, directory: str) -> Unit | None:
    if any(part in NON_UNIT_DIRS for part in directory.split("/")):
        return None
    base = root / directory if directory else root
    cargo = _manifest(base / "Cargo.toml", tomllib.loads)
    name = cargo.get("package", {}).get("name")
    if isinstance(name, str):
        deps: list[str] = []
        for section in ("dependencies", "build-dependencies"):
            for key, spec in (cargo.get(section) or {}).items():
                real = spec.get("package") if isinstance(spec, dict) else None
                deps.append(real if isinstance(real, str) else key)
        return Unit(name, "cargo_crate", directory, deps)
    package = _manifest(base / "package.json", json.loads)
    name = package.get("name")
    if isinstance(name, str):
        deps = [
            dep
            for section in ("dependencies", "peerDependencies")
            for dep in package.get(section) or {}
        ]
        return Unit(name, "npm_package", directory, deps)
    gomod = base / "go.mod"
    if gomod.is_file():
        for line in gomod.read_text().splitlines():
            if line.strip().startswith("module "):
                return Unit(
                    line.strip().removeprefix("module ").strip(), "go_module", directory, []
                )
    if (
        directory
        and (base / "__init__.py").is_file()
        and not (root / parent(directory) / "__init__.py").is_file()
    ):
        return Unit(directory.rsplit("/", 1)[-1], "python_package", directory, [])
    if (
        directory
        and base.is_dir()
        and any(p.suffix == ".tf" and p.is_file() for p in base.iterdir())
    ):
        return Unit(directory, "terraform_module", directory, [])
    return None


@dataclass
class UnitIndex:
    units: list[Unit]
    unit_of_file: dict[str, int]

    def labels(self) -> list[str]:
        counts = Counter(unit.name for unit in self.units)
        return [f"{u.name} ({u.path})" if counts[u.name] > 1 else u.name for u in self.units]


def discover(root: Path, files: list[str]) -> UnitIndex:
    units: list[Unit] = []
    by_dir: dict[str, int | None] = {}
    fallback: dict[str, int] = {}
    unit_of_file: dict[str, int] = {}
    for file in files:
        directory = parent(file)
        found = None
        while True:
            if directory not in by_dir:
                unit = declared_unit(root, directory)
                if unit is not None:
                    units.append(unit)
                by_dir[directory] = len(units) - 1 if unit is not None else None
            found = by_dir[directory]
            if found is not None or not directory:
                break
            directory = parent(directory)
        if found is None:
            top = file.split("/", 1)[0] if "/" in file else ""
            if top not in fallback:
                units.append(Unit(top or "<root>", "directory", top, []))
                fallback[top] = len(units) - 1
            found = fallback[top]
        unit_of_file[file] = found
    return UnitIndex(units, unit_of_file)


def file_to_package(path: str) -> str:
    return parent(path) or "<root>"


# crates/dagayn-graph/src/postprocess/bare_names.rs `language_family`
FAMILY_BY_EXTENSION = {
    **dict.fromkeys(("py", "pyi", "ipynb"), "python"),
    "rs": "rust",
    **dict.fromkeys(("c", "h", "cc", "cpp", "cxx", "hh", "hpp", "hxx", "m", "mm"), "c"),
    **dict.fromkeys(
        ("js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "vue", "svelte"), "javascript"
    ),
    **dict.fromkeys(("java", "kt", "kts", "scala", "sc"), "jvm"),
    "cs": "csharp",
    "go": "go",
    "rb": "ruby",
    "php": "php",
    "swift": "swift",
    "dart": "dart",
    "lua": "lua",
    **dict.fromkeys(("pl", "pm"), "perl"),
    "r": "r",
    "jl": "julia",
    **dict.fromkeys(("ex", "exs"), "elixir"),
    "zig": "zig",
    "gd": "gdscript",
    **dict.fromkeys(("sh", "bash", "zsh"), "bash"),
}


def language_family(path: str) -> str | None:
    if "." not in path:
        return None
    return FAMILY_BY_EXTENSION.get(path.rsplit(".", 1)[1].lower())


def scope_by_name(
    names: dict[str, set[tuple[str | None, str]]], name: str, source_file: str
) -> str | None:
    """`architecture.rs` `ScopesByName::get`: the one scope holding every
    node called *name*, when one of them is in the source file's language
    family."""
    entries = names.get(name, set())
    scopes = {scope for _, scope in entries}
    if len(scopes) != 1:
        return None
    family = language_family(source_file)
    if not any(family is None or other is None or other == family for other, _ in entries):
        return None
    return next(iter(scopes))


# -- SAP ---------------------------------------------------------------------


def sap(graph: Graph) -> list[dict[str, Any]]:
    """`sap_metrics` over the unit view of `stability.rs::unit_view`."""
    code_files = sorted(
        {
            n["file_path"]
            for n in graph.nodes
            if n["kind"] == "File" and n["language"] in CODE_LANGUAGES
        }
    )
    index = discover(graph.root, code_files)
    labels = index.labels()
    unit_scope = {file: labels[i] for file, i in index.unit_of_file.items()}

    qualified: dict[str, str] = {}
    names: dict[str, set[tuple[str | None, str]]] = defaultdict(set)
    for node in graph.nodes:
        path = node["file_path"]
        if not path or node["language"] == "markdown" or path.lower().endswith(DOC_SUFFIXES):
            continue
        scope = unit_scope.get(path) or file_to_package(path)
        qualified[node["qualified_name"]] = scope
        names[node["name"]].add((language_family(path), scope))

    nt: Counter[str] = Counter()
    na: Counter[str] = Counter()
    nt_test: Counter[str] = Counter()
    untyped: dict[str, Counter[str]] = defaultdict(Counter)
    abstract_types: dict[str, list[str]] = defaultdict(list)
    types: dict[str, list[str]] = defaultdict(list)
    for node in graph.nodes:
        scope = qualified.get(node["qualified_name"])
        if scope is None or node["kind"] != "Class" or node["parent_name"]:
            continue
        extra = node["extra"]
        role = extra.get("type_role") or "class"
        if role not in NT_ROLES:
            continue
        nt[scope] += 1
        types[scope].append(f"{node['file_path']}:{node['line_start']} {node['name']} ({role})")
        if is_test_node(node):
            nt_test[scope] += 1
        if "type_role" not in extra:
            untyped[scope][node["language"]] += 1
        if extra.get("is_abstract") or extra.get("is_contract") or role in NA_ROLES:
            na[scope] += 1
            abstract_types[scope].append(
                f"{node['file_path']}:{node['line_start']} {node['name']} ({role})"
            )

    outgoing: dict[str, Counter[str]] = defaultdict(Counter)
    sites: dict[tuple[str, str], list[str]] = defaultdict(list)
    for edge in graph.edges:
        if edge["kind"] not in DEPENDENCY_KINDS:
            continue
        source = qualified.get(edge["source_qualified"])
        target = qualified.get(edge["target_qualified"])
        if target is None and edge["kind"] != "IMPORTS_FROM" and not edge["external"]:
            target = scope_by_name(names, edge["target_qualified"], edge["file_path"])
        if source is None or target is None or target == source:
            continue
        outgoing[source][target] += 1
        if len(sites[(source, target)]) < 3:
            sites[(source, target)].append(f"{edge['file_path']}:{edge['line']} {edge['kind']}")
    incoming: dict[str, set[str]] = defaultdict(set)
    for source, targets in outgoing.items():
        for target in targets:
            incoming[target].add(source)

    rows = []
    for scope in sorted(set(labels)):
        ce, ca = len(outgoing[scope]), len(incoming[scope])
        a = na[scope] / nt[scope] if nt[scope] else 0.0
        i = ce / (ca + ce) if ca + ce else 0.0
        reason = (
            "no-eligible-types" if not nt[scope] else "isolated" if not ca + ce else "applicable"
        )
        zone = None
        if reason == "applicable" and a < ZONE_LOW and i < ZONE_LOW:
            zone = "pain"
        elif reason == "applicable" and a > ZONE_HIGH and i > ZONE_HIGH:
            zone = "uselessness"
        rows.append(
            {
                "unit": scope,
                "production": is_production_scope(scope),
                "na": na[scope],
                "nt": nt[scope],
                "nt_from_tests": nt_test[scope],
                "nt_without_type_role": dict(untyped[scope]),
                "ca": ca,
                "ce": ce,
                "abstractness": round(a, 3),
                "instability": round(i, 3),
                "distance": round(abs(a + i - 1), 3),
                "applicability": reason,
                "zone": zone,
                "abstract_types": abstract_types[scope][:8],
                "types": sorted(types[scope])[:12],
                "depends_on": {t: sites[(scope, t)] for t in sorted(outgoing[scope])},
                "depended_on_by": {s: sites[(s, scope)] for s in sorted(incoming[scope])},
            }
        )
    return rows


def sap_extremes(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    applicable = [r for r in rows if r["production"] and r["applicability"] == "applicable"]
    return sorted(applicable, key=lambda r: (-r["distance"], r["unit"]))[:EXTREMES]


# -- CRP ---------------------------------------------------------------------


def public_rule(node: dict[str, Any], graph: Graph) -> str | None:
    """The rule that makes ``node`` public surface, or None."""
    if node["kind"] not in SYMBOL_KINDS:
        return None
    language = node["language"]
    if language == "python":
        if node["name"].startswith("_"):
            return None
        if not node["parent_name"]:
            return "python-name"
        owner = graph.owner(node)
        if (
            owner
            and owner["kind"] == "Class"
            and not owner["parent_name"]
            and not owner["name"].startswith("_")
        ):
            return "python-member"
        return None
    if language == "rust":
        if node["extra"].get("type_role") == "module":
            return None
        if "pub" not in (node["modifiers"] or "").split():
            return None
        return "rust-pub-function" if node["kind"] == "Function" else "rust-pub-type"
    if language in TS_LANGUAGES:
        if node["extra"].get("exported"):
            return "ts-exported"
        owner = graph.owner(node)
        modifiers = (node["modifiers"] or "").split()
        if (
            owner
            and owner["kind"] == "Class"
            and owner["extra"].get("exported")
            and not node["name"].startswith("#")
            and "private" not in modifiers
            and "protected" not in modifiers
        ):
            return "ts-member"
    return None


def crp(graph: Graph) -> list[dict[str, Any]]:
    """`surface_used` per unit edge of `units.rs::unit_map`."""
    production = {n["qualified_name"]: n for n in graph.nodes if is_production(n)}
    index = discover(graph.root, sorted({n["file_path"] for n in production.values()}))
    labels = index.labels()

    def unit_of(qn: str) -> int | None:
        node = production.get(qn)
        return index.unit_of_file.get(node["file_path"] if node else qn)

    public: dict[int, set[str]] = defaultdict(set)
    rules: dict[int, Counter[str]] = defaultdict(Counter)
    for qn, node in production.items():
        unit = index.unit_of_file.get(node["file_path"])
        if unit is None:
            continue
        rule = public_rule(node, graph)
        if rule:
            public[unit].add(qn)
            rules[unit][rule] += 1

    weights: dict[tuple[int, int], Counter[str]] = defaultdict(Counter)
    used: dict[tuple[int, int], set[str]] = defaultdict(set)
    reached: dict[tuple[int, int], set[str]] = defaultdict(set)
    for edge in graph.edges:
        if edge["kind"] not in UNIT_EDGE_KINDS:
            continue
        source, target = unit_of(edge["source_qualified"]), unit_of(edge["target_qualified"])
        if source is None or target is None or source == target:
            continue
        weights[(source, target)][edge["kind"]] += 1
        if edge["kind"] in SURFACE_KINDS and edge["target_qualified"] in production:
            reached[(source, target)].add(edge["target_qualified"])
            if edge["target_qualified"] in public[target]:
                used[(source, target)].add(edge["target_qualified"])
    by_package = {(u.kind, u.name): i for i, u in enumerate(index.units)}
    declared = set()
    for source, unit in enumerate(index.units):
        for dep in unit.depends_on:
            target = by_package.get((unit.kind, dep))
            if target is not None and target != source:
                declared.add((source, target))
                weights.setdefault((source, target), Counter())

    rows = []
    for (source, target), kinds in sorted(weights.items()):
        total, hit = len(public[target]), used[(source, target)]
        rows.append(
            {
                "edge": f"{labels[source]} -> {labels[target]}",
                "to_kind": index.units[target].kind,
                "weights": dict(kinds),
                "declared": (source, target) in declared,
                "used": len(hit),
                "total": total,
                "share": round(len(hit) / total, 3) if total else None,
                "reached_not_public": len(reached[(source, target)] - hit),
                "surface_by_rule": dict(rules[target]),
                "used_symbols": sorted(hit)[:12],
            }
        )
    return rows


def crp_extremes(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    eligible = [r for r in rows if r["total"] >= MIN_SURFACE]
    return sorted(eligible, key=lambda r: (r["share"], -r["total"], r["edge"]))[:EXTREMES]


# -- refused_member ----------------------------------------------------------

REFUSAL_WORDS = r"(?:not\s+(?:yet\s+)?implemented|unimplemented|not\s+supported|unsupported)"
RUST_REFUSAL = re.compile(
    rf'^(?:(?:unimplemented|todo)!\s*\(.*\)|panic!\s*\(\s*".*{REFUSAL_WORDS}.*\))\s*;?$',
    re.IGNORECASE | re.DOTALL,
)
TS_REFUSAL = re.compile(
    rf"^throw\s+new\s+\w*Error\s*\(\s*[`'\"].*{REFUSAL_WORDS}.*\)\s*;?$",
    re.IGNORECASE | re.DOTALL,
)
PY_REFUSAL = re.compile(r"^raise\s+NotImplementedError\b.*$", re.DOTALL)
PY_DOCSTRING = re.compile(r'^\s*(?:"""[\s\S]*?"""|\'\'\'[\s\S]*?\'\'\')')


def _drop_comment_lines(text: str, marker: str) -> list[str]:
    return [
        line.strip()
        for line in text.splitlines()
        if line.strip() and not line.strip().startswith(marker)
    ]


def body_refuses(node: dict[str, Any], lines: list[str]) -> str | None:
    """The refusing statement when the body holds nothing else."""
    start, end = node["line_start"], node["line_end"]
    if not start or not end:
        return None
    text = "\n".join(lines[start - 1 : end])
    if node["language"] == "python":
        header, sep, body = text.partition(":\n")
        if not sep:
            return None
        statements = _drop_comment_lines(PY_DOCSTRING.sub("", body), "#")
        joined = " ".join(statements)
        return joined if statements and PY_REFUSAL.match(joined) else None
    if "{" not in text or "}" not in text:
        return None
    body = text[text.index("{") + 1 : text.rindex("}")]
    body = " ".join(_drop_comment_lines(re.sub(r"/\*[\s\S]*?\*/", "", body), "//"))
    pattern = RUST_REFUSAL if node["language"] == "rust" else TS_REFUSAL
    return body if pattern.match(body) else None


def refused_members(graph: Graph) -> dict[str, Any]:
    bases: dict[str, set[str]] = defaultdict(set)
    for edge in graph.edges:
        if edge["kind"] in ("INHERITS", "IMPLEMENTS"):
            bases[edge["source_qualified"]].add(edge["target_qualified"])
    sources: dict[str, list[str]] = {}

    def lines_of(path: str) -> list[str]:
        if path not in sources:
            try:
                sources[path] = (graph.root / path).read_text(errors="replace").splitlines()
            except OSError:
                sources[path] = []
        return sources[path]

    candidates = []
    anywhere = []
    checked = 0
    for node in graph.nodes:
        if node["kind"] != "Function" or not is_production(node):
            continue
        if node["language"] not in TS_LANGUAGES | {"python", "rust"}:
            continue
        owner = graph.owner(node)
        overrides = (
            owner is not None
            and owner["kind"] == "Class"
            and bool(bases.get(owner["qualified_name"]))
            # An abstraction's own default is not an override.
            and owner["extra"].get("type_role") not in NA_ROLES
            and not owner["extra"].get("is_abstract")
        )
        checked += overrides
        refusal = body_refuses(node, lines_of(node["file_path"]))
        if refusal is None:
            continue
        anywhere.append(f"{node['file_path']}:{node['line_start']}")
        if not overrides or owner is None:
            continue
        candidates.append(
            {
                "qualified_name": node["qualified_name"],
                "place": f"{node['file_path']}:{node['line_start']}",
                "owner": owner["name"],
                "bases": sorted(bases[owner["qualified_name"]]),
                "refusal": refusal[:160],
            }
        )
    candidates.sort(key=lambda c: c["qualified_name"])
    return {
        "override_methods_read": checked,
        "refusing_bodies_anywhere": sorted(anywhere),
        "candidates": candidates[:MAX_REFUSED],
    }


# -- judgments ---------------------------------------------------------------


def score(results: dict[str, dict[str, Any]]) -> dict[str, Any]:
    rows = yaml.safe_load(JUDGMENTS.read_text())["judgments"] or []
    reported = {
        name: {
            "sap": {r["unit"] for r in result["sap_extremes"]},
            "crp": {r["edge"] for r in result["crp_extremes"]},
            "refused_member": {c["qualified_name"] for c in result["refused_member"]["candidates"]},
        }
        for name, result in results.items()
    }
    tally: dict[str, Counter[str]] = defaultdict(Counter)
    repos: dict[str, set[str]] = defaultdict(set)
    for row in rows:
        if row["verdict"] not in VERDICTS or row["measure"] not in MEASURES:
            sys.exit(f"bad row {row}")
        if row["repo"] not in reported:
            continue
        if row["id"] not in reported[row["repo"]][row["measure"]]:
            sys.exit(f"judged {row['measure']} {row['id']!r} is not reported for {row['repo']}")
        tally[row["measure"]][row["verdict"]] += 1
        repos[row["measure"]].add(row["repo"])
    out: dict[str, Any] = {}
    for measure in MEASURES:
        counts = tally[measure]
        n = sum(counts.values())
        out[measure] = {
            **{verdict: counts[verdict] for verdict in VERDICTS},
            "n": n,
            "repos": sorted(repos[measure]),
            "share_actionable": round(counts["actionable"] / n, 3) if n else None,
        }
    out["refused_member"]["observable"] = sum(
        len(result["refused_member"]["candidates"]) for result in results.values()
    )
    return out


# -- output ------------------------------------------------------------------


def markdown(results: dict[str, dict[str, Any]]) -> str:
    out = []
    for name, result in results.items():
        out += [
            f"## {name}",
            "",
            "| Unit | Nt | Na | Ca | Ce | I | A | D | zone |",
            "|---|---:|---:|---:|---:|---:|---:|---:|---|",
        ]
        for r in result["sap"]:
            if not r["production"]:
                continue
            d = r["distance"] if r["applicability"] == "applicable" else r["applicability"]
            out.append(
                f"| {r['unit']} | {r['nt']} | {r['na']} | {r['ca']} | {r['ce']} | "
                f"{r['instability']} | {r['abstractness']} | {d} | {r['zone'] or ''} |"
            )
        out += ["", "| Unit edge | used / public | share |", "|---|---:|---:|"]
        out += [
            f"| {r['edge']} | {r['used']} / {r['total']} | {r['share']} |"
            for r in result["crp_extremes"]
        ]
        refused = result["refused_member"]
        out += [
            "",
            f"refused_member: {len(refused['candidates'])} candidates, "
            f"{len(refused['refusing_bodies_anywhere'])} refusing bodies anywhere",
            "",
        ]
    return "\n".join(out)


def main() -> int:
    parser = argparse.ArgumentParser(description=(__doc__ or "").split("\n\n")[0])
    parser.add_argument("--repo", action="append", required=True, metavar="NAME=PATH")
    parser.add_argument("--out", type=Path, help="write results.json and summary.json here")
    parser.add_argument(
        "--markdown", action="store_true", help="print tables instead of the JSON summary"
    )
    args = parser.parse_args()

    problems: list[str] = []
    results: dict[str, dict[str, Any]] = {}
    for spec in args.repo:
        name, _, path = spec.partition("=")
        graph = load(Path(path).resolve())
        problems += check_meta(name, graph)
        sap_rows, crp_rows = sap(graph), crp(graph)
        results[name] = {
            "graph": {
                key: graph.meta.get(key)
                for key in (
                    "schema_version",
                    "last_build_type",
                    "git_head_sha",
                    "extractor_versions",
                )
            },
            "sap": sap_rows,
            "sap_extremes": sap_extremes(sap_rows),
            "crp": crp_rows,
            "crp_extremes": crp_extremes(crp_rows),
            "refused_member": refused_members(graph),
        }
    summary: dict[str, Any] = {"repos": {}, "problems": problems}
    for name, result in results.items():
        production = [r for r in result["sap"] if r["production"]]
        summary["repos"][name] = {
            "units": len(production),
            "sap_applicable": sum(r["applicability"] == "applicable" for r in production),
            "sap_extremes": [[r["unit"], r["distance"], r["zone"]] for r in result["sap_extremes"]],
            "unit_edges": len(result["crp"]),
            "crp_eligible": sum(r["total"] >= MIN_SURFACE for r in result["crp"]),
            "crp_extremes": [[r["edge"], r["used"], r["total"]] for r in result["crp_extremes"]],
            "override_methods_read": result["refused_member"]["override_methods_read"],
            "refused_member": len(result["refused_member"]["candidates"]),
            "refusing_bodies_anywhere": len(result["refused_member"]["refusing_bodies_anywhere"]),
        }
    if JUDGMENTS.exists():
        summary["judged"] = score(results)
    if args.out:
        args.out.mkdir(parents=True, exist_ok=True)
        (args.out / "results.json").write_text(json.dumps(results, indent=2))
        (args.out / "summary.json").write_text(json.dumps(summary, indent=2))
    print(markdown(results) if args.markdown else json.dumps(summary, indent=2))
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
