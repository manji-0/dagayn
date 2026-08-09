"""Layer-2 manifest-backed CROSS_ARTIFACT bridge extraction.

Parses common build and codegen manifests and emits explainable
``CROSS_ARTIFACT`` edges with confidence/evidence in ``extra``.

Prefer exact manifest fields (``tool.maturin.manifest-path``,
``openapitools.json`` generator ``inputSpec``/``output``, package.json
dependency on a generated package name) over naming-only heuristics.
"""

from __future__ import annotations

import json
import logging
import re
import tomllib
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import Any, Iterable, Iterator

from ..incremental_files import _load_ignore_patterns, _should_ignore
from ._base.types import EdgeInfo, NodeInfo

logger = logging.getLogger(__name__)

EXTRACTOR_ID = "manifest_bridges"

# Confidence contract for Layer-2 manifest bridges (see docs/CROSS-ARTIFACT-EDGES-WIP.md).
CONFIDENCE_EXACT = 1.0
CONFIDENCE_HIGH = 0.8

_OPENAPI_GENERATOR_SCRIPT_RE = re.compile(
    r"openapi-generator(?:-cli)?\s+generate\b(?P<args>.*)$",
    re.IGNORECASE,
)
_CLI_INPUT_RE = re.compile(
    r"(?:--input-spec|-i)\s+(?P<path>(?:\"[^\"]+\"|'[^']+'|[^\s]+))",
    re.IGNORECASE,
)
_CLI_OUTPUT_RE = re.compile(
    r"(?:--output|-o)\s+(?P<path>(?:\"[^\"]+\"|'[^']+'|[^\s]+))",
    re.IGNORECASE,
)


@dataclass
class ManifestBridgeResult:
    """Nodes and edges discovered from manifests under a repository root."""

    nodes: list[NodeInfo] = field(default_factory=list)
    edges: list[EdgeInfo] = field(default_factory=list)

    @property
    def edge_count(self) -> int:
        return len(self.edges)


def discover_manifest_bridges(repo_root: Path) -> ManifestBridgeResult:
    """Scan *repo_root* for supported manifests and build bridge edges."""
    repo_root = repo_root.resolve()
    result = ManifestBridgeResult()
    ignore_patterns = _load_ignore_patterns(repo_root)

    pyprojects = list(_iter_named_files(repo_root, "pyproject.toml", ignore_patterns))
    openapitools = list(_iter_named_files(repo_root, "openapitools.json", ignore_patterns))
    package_jsons = list(_iter_named_files(repo_root, "package.json", ignore_patterns))

    # Generator output roots (repo-relative), later mapped to npm package names.
    generated_roots: set[str] = set()

    for rel_path in pyprojects:
        _extract_maturin_bridges(repo_root, rel_path, result)

    for rel_path in openapitools:
        _extract_openapitools_bridges(repo_root, rel_path, result, generated_roots)

    for rel_path in package_jsons:
        _extract_package_json_generator_scripts(repo_root, rel_path, result, generated_roots)

    generated_by_name = _index_generated_package_names(repo_root, generated_roots)
    for rel_path in package_jsons:
        _extract_generated_client_consumers(repo_root, rel_path, generated_by_name, result)

    return result


def _iter_named_files(
    repo_root: Path,
    file_name: str,
    ignore_patterns: list[str],
) -> Iterator[str]:
    for path in repo_root.rglob(file_name):
        if not path.is_file() or path.is_symlink():
            continue
        try:
            rel = path.relative_to(repo_root).as_posix()
        except ValueError:
            continue
        if _should_ignore(rel, ignore_patterns):
            continue
        yield rel


def _extract_maturin_bridges(
    repo_root: Path,
    pyproject_rel: str,
    result: ManifestBridgeResult,
) -> None:
    data = _load_toml(repo_root / pyproject_rel)
    if data is None:
        return

    tool = data.get("tool")
    if not isinstance(tool, dict):
        return
    maturin = tool.get("maturin")
    if not isinstance(maturin, dict):
        return

    pyproject_dir = PurePosixPath(pyproject_rel).parent
    manifest_path = maturin.get("manifest-path")
    module_name = maturin.get("module-name")
    evidence_source = "tool.maturin.manifest-path"
    confidence = CONFIDENCE_EXACT
    confidence_tier = "EXACT"

    if isinstance(manifest_path, str) and manifest_path.strip():
        cargo_rel = _resolve_rel(pyproject_dir, manifest_path.strip())
    else:
        # Default maturin layout: Cargo.toml beside pyproject.toml.
        cargo_rel = _resolve_rel(pyproject_dir, "Cargo.toml")
        evidence_source = "tool.maturin"
        confidence = CONFIDENCE_HIGH
        confidence_tier = "HIGH"

    cargo_abs = repo_root / cargo_rel
    if not cargo_abs.is_file():
        logger.debug(
            "Skipping maturin bridge from %s: missing Cargo.toml at %s",
            pyproject_rel,
            cargo_rel,
        )
        return

    _ensure_file_node(result, pyproject_rel, language="toml")
    _ensure_file_node(result, cargo_rel, language="toml")

    extra = _bridge_extra(
        relationship_role="builds_artifact",
        bridge_kind="extension_module",
        evidence_kind="manifest",
        evidence_source=evidence_source,
        source_language="python",
        target_language="rust",
        confidence=confidence,
        confidence_tier=confidence_tier,
    )
    if isinstance(module_name, str) and module_name.strip():
        extra["module_name"] = module_name.strip()
    extra["manifest_kind"] = "maturin"

    result.edges.append(
        EdgeInfo(
            kind="CROSS_ARTIFACT",
            source=pyproject_rel,
            target=cargo_rel,
            file_path=pyproject_rel,
            line=0,
            extra=extra,
        )
    )


def _extract_openapitools_bridges(
    repo_root: Path,
    config_rel: str,
    result: ManifestBridgeResult,
    generated_roots: set[str],
) -> None:
    data = _load_json(repo_root / config_rel)
    if data is None:
        return

    generators = (data.get("generator-cli") or {}).get("generators")
    if not isinstance(generators, dict):
        return

    config_dir = PurePosixPath(config_rel).parent
    for gen_name, gen_cfg in generators.items():
        if not isinstance(gen_cfg, dict):
            continue
        input_spec = gen_cfg.get("inputSpec") or gen_cfg.get("input")
        output = gen_cfg.get("output")
        if not isinstance(input_spec, str) or not input_spec.strip():
            continue
        if not isinstance(output, str) or not output.strip():
            continue

        schema_rel = _resolve_rel(config_dir, input_spec.strip())
        output_rel = _resolve_rel(config_dir, output.strip()).rstrip("/")
        schema_abs = repo_root / schema_rel
        output_abs = repo_root / output_rel
        if not schema_abs.is_file():
            logger.debug(
                "Skipping openapitools generator %s: missing schema %s",
                gen_name,
                schema_rel,
            )
            continue
        if not output_abs.exists():
            logger.debug(
                "Skipping openapitools generator %s: missing output %s",
                gen_name,
                output_rel,
            )
            continue

        package_rel = _package_root_for_output(repo_root, output_rel)
        _ensure_file_node(result, config_rel, language="json")
        _ensure_file_node(
            result,
            schema_rel,
            language=_schema_language(schema_rel),
        )
        _ensure_file_node(result, package_rel, language=_package_language(package_rel))

        extra = _bridge_extra(
            relationship_role="generates_code",
            bridge_kind="generated_code",
            evidence_kind="manifest",
            evidence_source="openapitools.generator-cli.generators",
            source_language=_schema_language(schema_rel),
            target_language=_package_language(package_rel),
            confidence=CONFIDENCE_EXACT,
            confidence_tier="EXACT",
        )
        extra["manifest_kind"] = "openapitools"
        extra["generator_name"] = str(gen_cfg.get("generatorName") or gen_name)
        extra["generator_key"] = str(gen_name)
        extra["output_path"] = output_rel

        result.edges.append(
            EdgeInfo(
                kind="CROSS_ARTIFACT",
                source=schema_rel,
                target=package_rel,
                file_path=config_rel,
                line=0,
                extra=extra,
            )
        )
        root = (
            package_rel[: -len("/package.json")]
            if package_rel.endswith("/package.json")
            else package_rel
        )
        generated_roots.add(root)


def _extract_package_json_generator_scripts(
    repo_root: Path,
    package_rel: str,
    result: ManifestBridgeResult,
    generated_roots: set[str],
) -> None:
    data = _load_json(repo_root / package_rel)
    if data is None:
        return
    scripts = data.get("scripts")
    if not isinstance(scripts, dict):
        return

    package_dir = PurePosixPath(package_rel).parent
    for script_name, script in scripts.items():
        if not isinstance(script, str):
            continue
        match = _OPENAPI_GENERATOR_SCRIPT_RE.search(script)
        if not match:
            continue
        args = match.group("args")
        input_match = _CLI_INPUT_RE.search(args)
        output_match = _CLI_OUTPUT_RE.search(args)
        if not input_match or not output_match:
            continue

        schema_rel = _resolve_rel(package_dir, _strip_quotes(input_match.group("path")))
        output_rel = _resolve_rel(package_dir, _strip_quotes(output_match.group("path"))).rstrip(
            "/"
        )
        if not (repo_root / schema_rel).is_file() or not (repo_root / output_rel).exists():
            continue

        package_out = _package_root_for_output(repo_root, output_rel)
        # Avoid duplicating an equivalent openapitools edge.
        if any(
            e.source == schema_rel and e.target == package_out and e.kind == "CROSS_ARTIFACT"
            for e in result.edges
        ):
            continue

        _ensure_file_node(result, package_rel, language="json")
        _ensure_file_node(result, schema_rel, language=_schema_language(schema_rel))
        _ensure_file_node(result, package_out, language=_package_language(package_out))

        extra = _bridge_extra(
            relationship_role="generates_code",
            bridge_kind="generated_code",
            evidence_kind="manifest",
            evidence_source=f"package.json.scripts.{script_name}",
            source_language=_schema_language(schema_rel),
            target_language=_package_language(package_out),
            confidence=CONFIDENCE_EXACT,
            confidence_tier="EXACT",
        )
        extra["manifest_kind"] = "package_json_script"
        extra["output_path"] = output_rel

        result.edges.append(
            EdgeInfo(
                kind="CROSS_ARTIFACT",
                source=schema_rel,
                target=package_out,
                file_path=package_rel,
                line=0,
                extra=extra,
            )
        )
        root = (
            package_out[: -len("/package.json")]
            if package_out.endswith("/package.json")
            else package_out
        )
        generated_roots.add(root)


def _index_generated_package_names(
    repo_root: Path,
    generated_roots: set[str],
) -> dict[str, str]:
    """Return npm package name → generated package root mappings."""
    by_name: dict[str, str] = {}
    for package_root in generated_roots:
        root = package_root.rstrip("/")
        pkg_json = repo_root / root / "package.json"
        if not pkg_json.is_file():
            continue
        data = _load_json(pkg_json)
        if not data:
            continue
        name = data.get("name")
        if isinstance(name, str) and name.strip():
            by_name[name.strip()] = root
    return by_name


def _extract_generated_client_consumers(
    repo_root: Path,
    package_rel: str,
    generated_by_name: dict[str, str],
    result: ManifestBridgeResult,
) -> None:
    data = _load_json(repo_root / package_rel)
    if data is None:
        return

    consumer_name = data.get("name") if isinstance(data.get("name"), str) else None
    deps: dict[str, Any] = {}
    for key in ("dependencies", "devDependencies", "optionalDependencies", "peerDependencies"):
        section = data.get(key)
        if isinstance(section, dict):
            deps.update(section)

    consumer_root = str(PurePosixPath(package_rel).parent)
    if consumer_root == ".":
        consumer_root = ""

    for dep_name, generated_root in generated_by_name.items():
        if dep_name not in deps:
            continue

        gen_root = generated_root.rstrip("/")
        if consumer_root.rstrip("/") == gen_root:
            continue
        if package_rel.startswith(f"{gen_root}/"):
            continue

        consumer_target = package_rel
        generated_target = (
            f"{gen_root}/package.json"
            if (repo_root / gen_root / "package.json").is_file()
            else gen_root
        )

        _ensure_file_node(result, consumer_target, language="json")
        _ensure_file_node(result, generated_target, language="json")

        extra = _bridge_extra(
            relationship_role="binds_generated_client",
            bridge_kind="generated_code",
            evidence_kind="manifest",
            evidence_source="package.json.dependencies",
            source_language="javascript",
            target_language="javascript",
            confidence=CONFIDENCE_EXACT,
            confidence_tier="EXACT",
        )
        extra["manifest_kind"] = "generated_client_dependency"
        extra["dependency_name"] = dep_name
        if consumer_name:
            extra["consumer_package_name"] = consumer_name

        result.edges.append(
            EdgeInfo(
                kind="CROSS_ARTIFACT",
                source=consumer_target,
                target=generated_target,
                file_path=package_rel,
                line=0,
                extra=extra,
            )
        )


def _package_root_for_output(repo_root: Path, output_rel: str) -> str:
    """Prefer a package.json under the generator output when present."""
    output_rel = output_rel.rstrip("/")
    direct = repo_root / output_rel / "package.json"
    if direct.is_file():
        return f"{output_rel}/package.json"
    return output_rel


def _ensure_file_node(result: ManifestBridgeResult, rel_path: str, *, language: str) -> None:
    if any(n.kind == "File" and n.file_path == rel_path for n in result.nodes):
        return
    result.nodes.append(
        NodeInfo(
            kind="File",
            name=rel_path,
            file_path=rel_path,
            line_start=1,
            line_end=1,
            language=language,
            extra={
                "extractor": EXTRACTOR_ID,
                "node_role": "Artifact",
                "origin_file": rel_path,
            },
        )
    )


def _bridge_extra(
    *,
    relationship_role: str,
    bridge_kind: str,
    evidence_kind: str,
    evidence_source: str,
    source_language: str,
    target_language: str,
    confidence: float,
    confidence_tier: str,
) -> dict[str, Any]:
    return {
        "relationship_role": relationship_role,
        "bridge_kind": bridge_kind,
        "evidence_kind": evidence_kind,
        "evidence_source": evidence_source,
        "source_language": source_language,
        "target_language": target_language,
        "confidence": confidence,
        "confidence_tier": confidence_tier,
        "extractor": EXTRACTOR_ID,
    }


def _resolve_rel(base_dir: PurePosixPath, declared: str) -> str:
    declared_path = PurePosixPath(declared)
    if declared_path.is_absolute() or declared.startswith("/"):
        # Treat absolute-looking paths as repo-root-relative by stripping the root.
        return declared.lstrip("/")
    if str(base_dir) in ("", "."):
        return declared_path.as_posix()
    return (base_dir / declared_path).as_posix()


def _schema_language(path: str) -> str:
    lower = path.lower()
    if lower.endswith((".yaml", ".yml")):
        return "yaml"
    if lower.endswith(".json"):
        return "json"
    if lower.endswith(".proto"):
        return "protobuf"
    return "schema"


def _package_language(path: str) -> str:
    lower = path.lower()
    if lower.endswith("package.json") or lower.endswith(".ts") or lower.endswith(".js"):
        return "javascript"
    if lower.endswith(".py") or "python" in lower:
        return "python"
    return "javascript"


def _strip_quotes(value: str) -> str:
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value


def _load_toml(path: Path) -> dict[str, Any] | None:
    try:
        with path.open("rb") as fh:
            data = tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        logger.debug("Failed to parse TOML %s: %s", path, exc)
        return None
    return data if isinstance(data, dict) else None


def _load_json(path: Path) -> dict[str, Any] | None:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        logger.debug("Failed to parse JSON %s: %s", path, exc)
        return None
    return data if isinstance(data, dict) else None


def refine_node_line_ends(repo_root: Path, nodes: Iterable[NodeInfo]) -> None:
    """Fill ``line_end`` for File nodes from on-disk content when available."""
    for node in nodes:
        if node.kind != "File":
            continue
        abs_path = repo_root / node.file_path
        try:
            text = abs_path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        node.line_end = max(1, text.count("\n") + (0 if text.endswith("\n") else 1 if text else 1))
