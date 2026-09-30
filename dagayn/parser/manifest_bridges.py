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
import os
import re
import shlex
import tomllib
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import TYPE_CHECKING, Any, Iterable, Iterator

from ._base.types import EdgeInfo, NodeInfo
from .ignore import _load_ignore_patterns, _should_ignore

if TYPE_CHECKING:
    from .native_library_manifests import NativeLibrary

logger = logging.getLogger(__name__)

type ManifestValue = Any
type ManifestData = dict[str, ManifestValue]

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
_WASM_PACK_BUILD_RE = re.compile(r"wasm-pack\s+build\b(?P<args>[^&|;]*)", re.IGNORECASE)
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

    found = _collect_named_files(repo_root, _MANIFEST_NAMES, ignore_patterns)
    pyprojects = found["pyproject.toml"]
    cargo_manifests = found["Cargo.toml"]
    openapitools = found["openapitools.json"]
    package_jsons = found["package.json"]
    build_scripts = sorted(
        path for name in ("Makefile", "makefile", "GNUmakefile", "justfile") for path in found[name]
    )

    # Generator output roots (repo-relative), later mapped to npm package names.
    generated_roots: set[str] = set()

    # Cargo.toml -> Python module name, for crates maturin builds.
    maturin_modules: dict[str, str | None] = {}
    for rel_path in pyprojects:
        built = _extract_maturin_bridges(repo_root, rel_path, result)
        if built is not None:
            cargo_rel, module_name = built
            maturin_modules[cargo_rel] = module_name
    # setuptools-rust names the module the same way maturin does.
    for rel_path in sorted([*pyprojects, *found["setup.py"]]):
        for cargo_rel, module_name in _extract_setuptools_rust(repo_root, rel_path, result):
            maturin_modules.setdefault(cargo_rel, module_name)

    wasm_hints = _collect_wasm_package_hints(repo_root, package_jsons)
    for rel_path in cargo_manifests:
        _extract_cargo_crate_root(repo_root, rel_path, maturin_modules, wasm_hints, result)

    _extract_wasm_producers(repo_root, package_jsons, build_scripts, found["asconfig.json"], result)

    makefiles = sorted(
        path for name in ("Makefile", "makefile", "GNUmakefile") for path in found[name]
    )
    _extract_native_libraries(
        repo_root,
        cmake_lists=found["CMakeLists.txt"],
        meson_builds=found["meson.build"],
        makefiles=makefiles,
        command_sources=_build_commands(repo_root, package_jsons, found["justfile"]),
        binding_gyps=found["binding.gyp"],
        cargo_build_scripts=found["build.rs"],
        zig_builds=found["build.zig"],
        result=result,
    )

    for rel_path in openapitools:
        _extract_openapitools_bridges(repo_root, rel_path, result, generated_roots)

    for rel_path in package_jsons:
        _extract_package_json_generator_scripts(repo_root, rel_path, result, generated_roots)

    generated_by_name = _index_generated_package_names(repo_root, generated_roots)
    for rel_path in package_jsons:
        _extract_generated_client_consumers(repo_root, rel_path, generated_by_name, result)

    return result


_MANIFEST_NAMES = (
    "pyproject.toml",
    "Cargo.toml",
    "openapitools.json",
    "package.json",
    "asconfig.json",
    "Makefile",
    "makefile",
    "GNUmakefile",
    "justfile",
    "CMakeLists.txt",
    "meson.build",
    "binding.gyp",
    "build.rs",
    "setup.py",
    "build.zig",
)


def _collect_named_files(
    repo_root: Path,
    names: Iterable[str],
    ignore_patterns: list[str],
) -> dict[str, list[str]]:
    """Repo-relative paths of files named *names*, in one directory walk.

    Ignored directories (``node_modules``, build outputs, ...) are pruned
    instead of walked and filtered afterwards.
    """
    wanted = set(names)
    found: dict[str, list[str]] = {name: [] for name in wanted}
    for dirpath, dirnames, filenames in os.walk(repo_root):
        rel_dir = Path(dirpath).relative_to(repo_root).as_posix()
        prefix = "" if rel_dir == "." else f"{rel_dir}/"
        dirnames[:] = sorted(
            name
            for name in dirnames
            if name != ".git"
            and not os.path.islink(os.path.join(dirpath, name))
            and not _should_ignore(f"{prefix}{name}/_", ignore_patterns)
        )
        for name in filenames:
            if name not in wanted:
                continue
            rel = f"{prefix}{name}"
            if os.path.islink(os.path.join(dirpath, name)) or _should_ignore(rel, ignore_patterns):
                continue
            found[name].append(rel)
    for paths in found.values():
        paths.sort()
    return found


def _extract_maturin_bridges(
    repo_root: Path,
    pyproject_rel: str,
    result: ManifestBridgeResult,
) -> tuple[str, str | None] | None:
    """Emit pyproject.toml -> Cargo.toml; return (Cargo.toml, module-name)."""
    data = _load_toml(repo_root / pyproject_rel)
    if data is None:
        return None

    tool = data.get("tool")
    if not isinstance(tool, dict):
        return None
    maturin = tool.get("maturin")
    if not isinstance(maturin, dict):
        return None

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

    if cargo_rel is None:
        logger.debug(
            "Skipping maturin bridge from %s: manifest-path escapes repository root",
            pyproject_rel,
        )
        return None

    cargo_abs = _contained_path(repo_root, cargo_rel)
    if cargo_abs is None or not cargo_abs.is_file():
        logger.debug(
            "Skipping maturin bridge from %s: missing Cargo.toml at %s",
            pyproject_rel,
            cargo_rel,
        )
        return None

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
    module = module_name.strip() if isinstance(module_name, str) and module_name.strip() else None
    return cargo_rel, module


_RUST_EXTENSION_RE = re.compile(r"\bRustExtension\s*\(")
_PY_STRING_ARG_RE = re.compile(r"""^\s*(?:(?P<key>\w+)\s*=\s*)?(["'])(?P<value>[^"']*)\2\s*$""")


def _python_call_string_args(text: str, start: int) -> tuple[list[str], dict[str, str]]:
    """String-literal arguments of the call whose ``(`` ends at *start*:
    positional values in order, and ``key="value"`` keywords. Other
    arguments are skipped."""
    args: list[str] = []
    depth, current, index = 1, "", start
    while index < len(text) and depth:
        char = text[index]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
        if depth == 0 or (char == "," and depth == 1):
            args.append(current)
            current = ""
        else:
            current += char
        index += 1
    positional: list[str] = []
    keywords: dict[str, str] = {}
    for arg in args:
        match = _PY_STRING_ARG_RE.match(arg)
        if match is None:
            continue
        if match.group("key"):
            keywords[match.group("key")] = match.group("value")
        else:
            positional.append(match.group("value"))
    return positional, keywords


def _extract_setuptools_rust(
    repo_root: Path, rel_path: str, result: ManifestBridgeResult
) -> list[tuple[str, str]]:
    """setuptools-rust extensions: ``RustExtension("pkg._core", "Cargo.toml")``
    in ``setup.py``, or ``[[tool.setuptools-rust.ext-modules]]`` with
    ``target`` / ``path`` in ``pyproject.toml``. Emits config -> Cargo.toml
    and returns ``(Cargo.toml, module)`` pairs."""
    base = PurePosixPath(rel_path).parent
    declared: list[tuple[str, str]] = []  # (module, Cargo.toml as written)
    if rel_path.endswith(".toml"):
        data = _load_toml(repo_root / rel_path) or {}
        tool = data.get("tool")
        section = tool.get("setuptools-rust") if isinstance(tool, dict) else None
        modules = section.get("ext-modules") if isinstance(section, dict) else None
        for module in modules if isinstance(modules, list) else []:
            if isinstance(module, dict) and isinstance(module.get("target"), str):
                path = module.get("path")
                declared.append((module["target"], path if isinstance(path, str) else "Cargo.toml"))
    else:
        try:
            text = (repo_root / rel_path).read_text(encoding="utf-8", errors="replace")
        except OSError:
            return []
        for match in _RUST_EXTENSION_RE.finditer(text):
            positional, keywords = _python_call_string_args(text, match.end())
            target = keywords.get("target") or (positional[0] if positional else None)
            if target is None:
                continue
            rest = positional if "target" in keywords else positional[1:]
            declared.append((target, keywords.get("path") or (rest[0] if rest else "Cargo.toml")))
    found: list[tuple[str, str]] = []
    for module, path in declared:
        cargo_rel = _resolve_rel(base, path)
        if cargo_rel is None or not (repo_root / cargo_rel).is_file():
            continue
        language = "toml" if rel_path.endswith(".toml") else "python"
        _ensure_file_node(result, rel_path, language=language)
        _ensure_file_node(result, cargo_rel, language="toml")
        extra = _bridge_extra(
            relationship_role="builds_artifact",
            bridge_kind="extension_module",
            evidence_kind="manifest",
            evidence_source="setuptools-rust",
            source_language="python",
            target_language="rust",
            confidence=CONFIDENCE_EXACT,
            confidence_tier="EXACT",
        )
        extra["module_name"] = module
        extra["manifest_kind"] = "setuptools-rust"
        result.edges.append(
            EdgeInfo(
                kind="CROSS_ARTIFACT",
                source=rel_path,
                target=cargo_rel,
                file_path=rel_path,
                line=0,
                extra=extra,
            )
        )
        found.append((cargo_rel, module))
    return found


@dataclass
class _WasmPackageHints:
    """What package.json files say about wasm-pack output, repo-relative."""

    # (crate dir, out dir, out name, scope) from `wasm-pack build` scripts.
    builds: list[tuple[str, str | None, str | None, str | None]] = field(default_factory=list)
    # (dependency name, path) for `"name": "file:../crate/pkg"` dependencies.
    file_dependencies: list[tuple[str, str]] = field(default_factory=list)


def _collect_wasm_package_hints(repo_root: Path, package_jsons: list[str]) -> _WasmPackageHints:
    hints = _WasmPackageHints()
    for package_rel in package_jsons:
        data = _load_json(repo_root / package_rel)
        if data is None:
            continue
        package_dir = PurePosixPath(package_rel).parent
        for section in ("dependencies", "devDependencies", "optionalDependencies"):
            deps = data.get(section)
            if not isinstance(deps, dict):
                continue
            for name, spec in deps.items():
                if not isinstance(name, str) or not isinstance(spec, str):
                    continue
                for prefix in ("file:", "link:", "portal:"):
                    if spec.startswith(prefix):
                        target = _resolve_rel(package_dir, spec[len(prefix) :])
                        if target is not None:
                            hints.file_dependencies.append((name, target))
        scripts = data.get("scripts")
        if not isinstance(scripts, dict):
            continue
        for script in scripts.values():
            if not isinstance(script, str):
                continue
            for match in _WASM_PACK_BUILD_RE.finditer(script):
                build = _parse_wasm_pack_args(package_dir, match.group("args"))
                if build is not None:
                    hints.builds.append(build)
    return hints


def _parse_wasm_pack_args(
    package_dir: PurePosixPath, args: str
) -> tuple[str, str | None, str | None, str | None] | None:
    """`wasm-pack build [path] --out-dir D --out-name N --scope S` -> parts.

    The crate path is relative to where the script runs (the package.json
    directory); `--out-dir` is relative to the crate.
    """
    tokens = [_strip_quotes(token) for token in args.split()]
    crate_path = "."
    options: dict[str, str] = {}
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token.startswith("--") and "=" in token:
            key, _, value = token.partition("=")
            options[key] = value
        elif token in (
            "--out-dir",
            "-d",
            "--out-name",
            "--scope",
            "-s",
            "--target",
            "-t",
            "--mode",
            "-m",
        ):
            if index + 1 < len(tokens):
                options[token] = tokens[index + 1]
            index += 1
        elif not token.startswith("-"):
            crate_path = token
        index += 1
    crate_rel = _resolve_rel(package_dir, crate_path) if crate_path != "." else None
    if crate_rel is None:
        crate_rel = "" if str(package_dir) in ("", ".") else package_dir.as_posix()
    out_dir = options.get("--out-dir") or options.get("-d")
    out_rel = None
    if out_dir:
        out_rel = _resolve_rel(PurePosixPath(crate_rel or "."), out_dir)
    scope = options.get("--scope") or options.get("-s")
    return crate_rel, out_rel, options.get("--out-name"), scope


def _cargo_depends_on(data: ManifestData, crate: str) -> bool:
    """True when *crate* is a dependency in any `[dependencies]` table."""
    tables: list[object] = [data.get("dependencies")]
    target = data.get("target")
    if isinstance(target, dict):
        tables.extend(
            spec.get("dependencies") for spec in target.values() if isinstance(spec, dict)
        )
    return any(isinstance(table, dict) and crate in table for table in tables)


def _wasm_package_facts(
    crate_rel: str, package_name: str, hints: _WasmPackageHints
) -> tuple[list[str], list[str]]:
    """JavaScript package names and output directories of a wasm-pack crate.

    wasm-pack writes `<crate>/pkg` named after the crate unless a script
    passes `--out-dir` / `--scope`; a `file:` dependency on one of those
    directories adds the name the consumer imports it by.
    """
    out_dirs = [f"{crate_rel}/pkg" if crate_rel else "pkg"]
    names = [package_name]
    for build_crate, out_dir, _out_name, scope in hints.builds:
        if build_crate != crate_rel:
            continue
        if out_dir and out_dir not in out_dirs:
            out_dirs.append(out_dir)
        if scope:
            scoped = f"@{scope.lstrip('@')}/{package_name}"
            if scoped not in names:
                names.append(scoped)
    for name, path in hints.file_dependencies:
        if path in out_dirs and name not in names:
            names.append(name)
    return names, out_dirs


def _uniffi_facts(repo_root: Path, crate_rel: str, lib_name: str) -> ManifestData:
    """The names UniFFI generates foreign bindings under: the UDL
    ``namespace`` (else the library name; ``setup_scaffolding!("ns")`` is
    read from the source later), and the Kotlin package / Swift module
    ``uniffi.toml`` may override (``uniffi.<namespace>`` / the namespace by
    default)."""
    crate_dir = repo_root / crate_rel if crate_rel else repo_root
    namespace = None
    src_dir = crate_dir / "src"
    for udl in sorted(src_dir.glob("*.udl")) if src_dir.is_dir() else []:
        try:
            match = re.search(
                r"^\s*namespace\s+(\w+)", udl.read_text(encoding="utf-8", errors="replace"), re.M
            )
        except OSError:
            continue
        if match:
            namespace = match.group(1)
            break
    facts: ManifestData = {"namespace": namespace or lib_name}
    config = _load_toml(crate_dir / "uniffi.toml") or {}
    bindings = config.get("bindings")
    bindings = bindings if isinstance(bindings, dict) else {}
    for language, key, fact in (
        ("kotlin", "package_name", "kotlin_package"),
        ("swift", "module_name", "swift_module"),
    ):
        section = bindings.get(language)
        value = section.get(key) if isinstance(section, dict) else None
        if isinstance(value, str) and value.strip():
            facts[fact] = value.strip()
    return facts


def _node_addon_facts(
    repo_root: Path, crate_rel: str, addon: str, hints: _WasmPackageHints
) -> ManifestData | None:
    """How JavaScript reaches a napi-rs / neon crate, from its package.json.

    The package sits in the crate directory or the one above it. JavaScript
    imports the addon by the package name (or a ``file:`` dependency on the
    package directory), through the generated glue (``main`` / ``types``,
    ``index.js`` / ``index.d.ts`` by default for napi-rs), or by requiring
    the built ``.node`` file: neon's ``main`` (``index.node``), or napi-rs's
    ``<binaryName>.<platform>.node``.
    """
    crate_dir = PurePosixPath(crate_rel) if crate_rel else PurePosixPath(".")
    candidates = [crate_dir] if str(crate_dir) == "." else [crate_dir, crate_dir.parent]
    for package_dir in candidates:
        package_rel = "package.json" if str(package_dir) == "." else f"{package_dir}/package.json"
        data = _load_json(repo_root / package_rel)
        if data is not None:
            break
    else:
        return None
    dir_rel = _dir_rel(package_dir) or ""
    names: list[str] = []
    name = data.get("name")
    if isinstance(name, str) and name.strip():
        names.append(name.strip())
    for dep_name, path in hints.file_dependencies:
        if path == dir_rel and dep_name not in names:
            names.append(dep_name)
    entries: list[str] = []
    outputs: list[str] = []
    fields = [data.get("main"), data.get("types"), data.get("typings")]
    if addon == "napi":
        fields += ["index.js", "index.d.ts"]
    else:
        fields.append("index.node")
    for value in fields:
        if not isinstance(value, str) or not value.strip():
            continue
        rel = _resolve_rel(package_dir, value.strip())
        if rel is None:
            continue
        bucket = outputs if rel.endswith(".node") else entries
        if rel not in bucket:
            bucket.append(rel)
    binary_names: list[str] = []
    if addon == "napi":
        # `napi.binaryName` (napi-rs 3) or `napi.name` (2), else `index`.
        napi = data.get("napi")
        napi = napi if isinstance(napi, dict) else {}
        configured = napi.get("binaryName") or napi.get("name")
        binary_names.append(
            configured.strip() if isinstance(configured, str) and configured.strip() else "index"
        )
    return {
        "js_packages": names,
        "js_entry_files": entries,
        "node_outputs": outputs,
        "node_binary_names": binary_names,
    }


def _extract_cargo_crate_root(
    repo_root: Path,
    cargo_rel: str,
    maturin_modules: dict[str, str | None],
    wasm_hints: _WasmPackageHints,
    result: ManifestBridgeResult,
) -> None:
    """Emit Cargo.toml -> library root for crates another language loads.

    Only crates maturin builds or that declare a ``cdylib`` count: those are
    the ones Python imports as an extension module or loads with ``ctypes``,
    and JavaScript imports as a wasm-bindgen package. The edge carries what
    native-binding resolution matches against -- the library name
    (``libNAME.so``), the crate directory, the Python module name maturin
    installs, and for wasm-bindgen crates the JavaScript package names and
    wasm-pack output directories.
    """
    data = _load_toml(repo_root / cargo_rel)
    if data is None:
        return
    package = data.get("package")
    lib = data.get("lib")
    lib = lib if isinstance(lib, dict) else {}
    crate_types = lib.get("crate-type")
    crate_types = (
        [t for t in crate_types if isinstance(t, str)] if isinstance(crate_types, list) else []
    )
    built_by_maturin = cargo_rel in maturin_modules
    if not built_by_maturin and "cdylib" not in crate_types:
        return

    lib_name = lib.get("name")
    if not isinstance(lib_name, str) or not lib_name.strip():
        package_name = package.get("name") if isinstance(package, dict) else None
        if not isinstance(package_name, str) or not package_name.strip():
            return
        lib_name = package_name
    package_name = package.get("name") if isinstance(package, dict) else None
    lib_name = lib_name.strip().replace("-", "_")

    crate_dir = PurePosixPath(cargo_rel).parent
    declared_path = lib.get("path")
    if isinstance(declared_path, str) and declared_path.strip():
        root_rel = _resolve_rel(crate_dir, declared_path.strip())
        evidence_source, confidence, tier = "lib.path", CONFIDENCE_EXACT, "EXACT"
    else:
        # Cargo's default library root.
        root_rel = _resolve_rel(crate_dir, "src/lib.rs")
        evidence_source, confidence, tier = "cargo default src/lib.rs", CONFIDENCE_HIGH, "HIGH"
    if root_rel is None:
        return
    root_abs = _contained_path(repo_root, root_rel)
    if root_abs is None or not root_abs.is_file():
        return

    _ensure_file_node(result, cargo_rel, language="toml")
    extra = _bridge_extra(
        relationship_role="builds_from_source",
        bridge_kind="build_config",
        evidence_kind="manifest",
        evidence_source=evidence_source,
        source_language="toml",
        target_language="rust",
        confidence=confidence,
        confidence_tier=tier,
    )
    extra["manifest_kind"] = "cargo"
    extra["lib_name"] = lib_name
    extra["crate_types"] = crate_types
    extra["crate_dir"] = "" if str(crate_dir) == "." else crate_dir.as_posix()
    if built_by_maturin:
        # maturin installs the extension as `module-name`, or the library name.
        extra["python_module"] = maturin_modules[cargo_rel] or lib_name
    if _cargo_depends_on(data, "wasm-bindgen") and isinstance(package_name, str):
        js_packages, out_dirs = _wasm_package_facts(
            extra["crate_dir"], package_name.strip(), wasm_hints
        )
        extra["wasm_bindgen"] = True
        extra["js_packages"] = js_packages
        extra["wasm_out_dirs"] = out_dirs
    if _cargo_depends_on(data, "uniffi"):
        extra["uniffi"] = _uniffi_facts(repo_root, extra["crate_dir"], lib_name)
    addon = next((kind for kind in ("napi", "neon") if _cargo_depends_on(data, kind)), None)
    if addon is not None:
        facts = _node_addon_facts(repo_root, extra["crate_dir"], addon, wasm_hints)
        if facts is not None:
            extra["node_addon"] = addon
            extra.update(facts)
    result.edges.append(
        EdgeInfo(
            kind="CROSS_ARTIFACT",
            source=cargo_rel,
            target=root_rel,
            file_path=cargo_rel,
            line=0,
            extra=extra,
        )
    )


_GO_WASM_BUILD_RE = re.compile(r"\b(?P<tool>tinygo|go)\s+build\b(?P<args>[^;&|\n]*)", re.IGNORECASE)
_ASC_RE = re.compile(r"(?:^|[\s;&|(])(?:npx\s+)?asc\s+(?P<args>[^;&|\n]*)")
_WASM_TARGETS = {"wasm", "wasi", "wasip1", "wasip2", "wasm-unknown"}


@dataclass
class _WasmProducer:
    """A WebAssembly module some build command produces from repo sources."""

    config_rel: str
    producer: str  # "go" | "tinygo" | "assemblyscript"
    root_rel: str
    outputs: list[str] = field(default_factory=list)
    export_dir: str | None = None
    entry_files: list[str] = field(default_factory=list)


def _extract_wasm_producers(
    repo_root: Path,
    package_jsons: list[str],
    build_scripts: list[str],
    asconfigs: list[str],
    result: ManifestBridgeResult,
) -> None:
    """Emit build config -> source edges for Go / TinyGo / AssemblyScript
    WebAssembly builds, carrying the `.wasm` outputs JavaScript loads and
    where the exported functions live."""
    producers: dict[tuple[str, str], _WasmProducer] = {}

    def add(producer: _WasmProducer) -> None:
        key = (producer.config_rel, producer.root_rel)
        existing = producers.get(key)
        if existing is None:
            producers[key] = producer
            return
        for output in producer.outputs:
            if output not in existing.outputs:
                existing.outputs.append(output)

    for command_file, commands in _build_commands(repo_root, package_jsons, build_scripts):
        cwd = PurePosixPath(command_file).parent
        for command in commands:
            for producer in _wasm_producers_in_command(repo_root, command_file, cwd, command):
                add(producer)
    for config_rel in asconfigs:
        producer = _assemblyscript_asconfig(repo_root, config_rel)
        if producer is not None:
            add(producer)

    for producer in producers.values():
        if not producer.outputs:
            continue
        language = "json" if producer.config_rel.endswith(".json") else "make"
        _ensure_file_node(result, producer.config_rel, language=language)
        extra = _bridge_extra(
            relationship_role="builds_from_source",
            bridge_kind="build_config",
            evidence_kind="config",
            evidence_source=f"{producer.producer} build",
            source_language=language,
            target_language="go" if producer.producer in ("go", "tinygo") else "typescript",
            confidence=CONFIDENCE_HIGH,
            confidence_tier="HIGH",
        )
        extra["manifest_kind"] = "wasm_build"
        extra["wasm_producer"] = producer.producer
        extra["wasm_outputs"] = producer.outputs
        if producer.export_dir is not None:
            extra["export_dir"] = producer.export_dir
        if producer.entry_files:
            extra["entry_files"] = producer.entry_files
        result.edges.append(
            EdgeInfo(
                kind="CROSS_ARTIFACT",
                source=producer.config_rel,
                target=producer.root_rel,
                file_path=producer.config_rel,
                line=0,
                extra=extra,
            )
        )


def _extract_native_libraries(
    repo_root: Path,
    *,
    cmake_lists: list[str],
    meson_builds: list[str],
    makefiles: list[str],
    command_sources: Iterator[tuple[str, list[str]]],
    binding_gyps: list[str],
    cargo_build_scripts: list[str],
    zig_builds: list[str],
    result: ManifestBridgeResult,
) -> None:
    """Emit build file -> source edges for C / C++ shared libraries.

    The edge carries the library name loaders are matched by (``libNAME.so``)
    and every source file compiled into it, where native-binding resolution
    looks for the exported C symbols.
    """
    # Imported here: the module reuses this module's path and CLI helpers.
    from .native_library_manifests import discover_native_libraries

    libraries = discover_native_libraries(
        repo_root,
        cmake_lists=cmake_lists,
        meson_builds=meson_builds,
        makefiles=makefiles,
        command_sources=command_sources,
        binding_gyps=binding_gyps,
        build_scripts=cargo_build_scripts,
        zig_builds=zig_builds,
    )
    for library in libraries:
        source_language = library.build_system
        if library.build_system in ("cc", "cxx"):
            source_language = "rust"  # build.rs
        elif library.build_system in ("zig", "zig-c"):
            source_language = "zig"  # build.zig
        _ensure_file_node(result, library.config_rel, language=source_language)
        # Command lines are build configuration; CMake / Meson / gyp declare
        # the library in a manifest.
        from_command = library.build_system in ("make", "emscripten", "cc", "cxx", "zig", "zig-c")
        extra = _bridge_extra(
            relationship_role="builds_from_source",
            bridge_kind="build_config",
            evidence_kind="config" if from_command else "manifest",
            evidence_source=f"{library.build_system} library",
            source_language=source_language,
            target_language=library.language,
            confidence=CONFIDENCE_HIGH,
            confidence_tier="HIGH",
        )
        extra["manifest_kind"] = "native_library"
        extra["build_system"] = library.build_system
        extra["lib_name"] = library.lib_name
        extra["source_files"] = library.sources
        if library.build_system == "node-gyp":
            extra.update(_gyp_addon_facts(repo_root, library))
        elif library.build_system == "emscripten":
            extra["wasm_outputs"] = library.outputs
            if library.wasm_exports is not None:
                extra["wasm_exports"] = library.wasm_exports
        result.edges.append(
            EdgeInfo(
                kind="CROSS_ARTIFACT",
                source=library.config_rel,
                target=library.sources[0],
                file_path=library.config_rel,
                line=0,
                extra=extra,
            )
        )


def _gyp_addon_facts(repo_root: Path, library: NativeLibrary) -> ManifestData:
    """How JavaScript reaches a node-gyp addon: the package.json beside
    ``binding.gyp`` names the package and its glue (``main``), and the
    addon is loaded from ``build/Release/NAME.node`` or ``bindings("NAME")``."""
    package_dir = PurePosixPath(library.config_rel).parent
    package_rel = "package.json" if str(package_dir) == "." else f"{package_dir}/package.json"
    data = _load_json(repo_root / package_rel) or {}
    name = data.get("name")
    main = data.get("main")
    entry = _resolve_rel(package_dir, main) if isinstance(main, str) and main.strip() else None
    return {
        "node_addon": "node-gyp",
        "js_packages": [name.strip()] if isinstance(name, str) and name.strip() else [],
        "js_entry_files": [entry] if entry and not entry.endswith(".node") else [],
        "node_outputs": library.outputs,
        "node_binary_names": [],
    }


def _build_commands(
    repo_root: Path, package_jsons: list[str], build_scripts: list[str]
) -> Iterator[tuple[str, list[str]]]:
    """(file, command lines) from package.json scripts, Makefiles, justfiles."""
    for rel in package_jsons:
        data = _load_json(repo_root / rel)
        scripts = data.get("scripts") if data else None
        if isinstance(scripts, dict):
            yield rel, [script for script in scripts.values() if isinstance(script, str)]
    for rel in build_scripts:
        try:
            text = (repo_root / rel).read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        yield rel, text.replace("\\\n", " ").splitlines()


def _split_command(text: str) -> list[str]:
    try:
        return shlex.split(text, comments=False)
    except ValueError:
        return text.split()


def _wasm_producers_in_command(
    repo_root: Path, command_file: str, cwd: PurePosixPath, command: str
) -> Iterator[_WasmProducer]:
    for match in _GO_WASM_BUILD_RE.finditer(command):
        tool = match.group("tool").lower()
        tokens = _split_command(match.group("args"))
        options, positional = _command_options(
            tokens, {"-o", "-target", "-tags", "-ldflags", "-gcflags", "-scheduler", "-gc", "-opt"}
        )
        if tool == "go" and "GOARCH=wasm" not in command.replace(" ", ""):
            continue
        if tool == "tinygo" and options.get("-target", "").lower() not in _WASM_TARGETS:
            continue
        output = options.get("-o")
        if not output or not output.endswith(".wasm"):
            continue
        package = positional[-1] if positional else "."
        if not package.startswith("."):
            continue  # a module import path, not a directory
        package_rel = _resolve_rel(cwd, package) if package != "." else _dir_rel(cwd)
        output_rel = _resolve_rel(cwd, output)
        if package_rel is None or output_rel is None:
            continue
        root = _go_main_file(repo_root, package_rel)
        if root is None:
            continue
        yield _WasmProducer(
            config_rel=command_file,
            producer=tool,
            root_rel=root,
            outputs=[output_rel],
            export_dir=package_rel,
        )
    for match in _ASC_RE.finditer(command):
        tokens = _split_command(match.group("args"))
        options, positional = _command_options(tokens, {"--outFile", "-o", "--target", "--config"})
        output = options.get("--outFile") or options.get("-o")
        entries = [
            rel
            for token in positional
            if token.endswith(".ts") and (rel := _resolve_rel(cwd, token)) is not None
        ]
        entries = [rel for rel in entries if (repo_root / rel).is_file()]
        if not output or not entries:
            continue  # `asc --target release` reads asconfig.json instead
        output_rel = _resolve_rel(cwd, output)
        if output_rel is None:
            continue
        yield _WasmProducer(
            config_rel=command_file,
            producer="assemblyscript",
            root_rel=entries[0],
            outputs=[output_rel],
            entry_files=entries,
        )


def _command_options(tokens: list[str], valued: set[str]) -> tuple[dict[str, str], list[str]]:
    """Split CLI tokens into ``{flag: value}`` and positional arguments."""
    options: dict[str, str] = {}
    positional: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token.startswith("-"):
            flag, eq, value = token.partition("=")
            if eq:
                options[flag] = value
            elif flag in valued and index + 1 < len(tokens):
                options[flag] = tokens[index + 1]
                index += 1
            else:
                options[flag] = ""
        elif "=" not in token or token.startswith("."):
            positional.append(token)
        index += 1
    return options, positional


def _dir_rel(path: PurePosixPath) -> str | None:
    text = path.as_posix()
    return "" if text in ("", ".") else text


def _go_main_file(repo_root: Path, package_rel: str) -> str | None:
    """The file of a Go package directory that declares `func main`, else the
    first `.go` file (tests excluded)."""
    directory = repo_root / package_rel if package_rel else repo_root
    if not directory.is_dir():
        return None
    files = sorted(
        path
        for path in directory.iterdir()
        if path.suffix == ".go" and not path.name.endswith("_test.go") and path.is_file()
    )
    if not files:
        return None
    for path in files:
        try:
            if re.search(
                r"^func\s+main\s*\(", path.read_text(encoding="utf-8", errors="replace"), re.M
            ):
                return path.relative_to(repo_root).as_posix()
        except OSError:
            continue
    return files[0].relative_to(repo_root).as_posix()


def _assemblyscript_asconfig(repo_root: Path, config_rel: str) -> _WasmProducer | None:
    """`asconfig.json`: `entries` and every target's `outFile`."""
    data = _load_json(repo_root / config_rel)
    if data is None:
        return None
    base = PurePosixPath(config_rel).parent
    entries_raw = data.get("entries")
    entries = [
        rel
        for entry in (entries_raw if isinstance(entries_raw, list) else [])
        if isinstance(entry, str)
        and (rel := _resolve_rel(base, entry)) is not None
        and (repo_root / rel).is_file()
    ]
    outputs: list[str] = []
    targets = data.get("targets")
    for target in targets.values() if isinstance(targets, dict) else []:
        out_file = target.get("outFile") if isinstance(target, dict) else None
        if isinstance(out_file, str) and (rel := _resolve_rel(base, out_file)) is not None:
            if rel not in outputs:
                outputs.append(rel)
    if not entries or not outputs:
        return None
    return _WasmProducer(
        config_rel=config_rel,
        producer="assemblyscript",
        root_rel=entries[0],
        outputs=outputs,
        entry_files=entries,
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
        output_resolved = _resolve_rel(config_dir, output.strip())
        if schema_rel is None or output_resolved is None:
            logger.debug(
                "Skipping openapitools generator %s: path escapes repository root",
                gen_name,
            )
            continue
        output_rel = output_resolved.rstrip("/")
        schema_abs = _contained_path(repo_root, schema_rel)
        output_abs = _contained_path(repo_root, output_rel)
        if schema_abs is None or not schema_abs.is_file():
            logger.debug(
                "Skipping openapitools generator %s: missing schema %s",
                gen_name,
                schema_rel,
            )
            continue
        if output_abs is None or not output_abs.exists():
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
        output_resolved = _resolve_rel(package_dir, _strip_quotes(output_match.group("path")))
        if schema_rel is None or output_resolved is None:
            continue
        output_rel = output_resolved.rstrip("/")
        schema_abs = _contained_path(repo_root, schema_rel)
        output_abs = _contained_path(repo_root, output_rel)
        if (
            schema_abs is None
            or output_abs is None
            or not schema_abs.is_file()
            or not output_abs.exists()
        ):
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
        pkg_json = _contained_path(repo_root, f"{root}/package.json")
        if pkg_json is None or not pkg_json.is_file():
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
    deps: ManifestData = {}
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
        gen_pkg = _contained_path(repo_root, f"{gen_root}/package.json")
        generated_target = (
            f"{gen_root}/package.json" if gen_pkg is not None and gen_pkg.is_file() else gen_root
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
    direct = _contained_path(repo_root, f"{output_rel}/package.json")
    if direct is not None and direct.is_file():
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
) -> ManifestData:
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


def _resolve_rel(base_dir: PurePosixPath, declared: str) -> str | None:
    """Resolve *declared* against *base_dir* as a repo-root-relative path.

    Absolute inputs are treated as repo-root-relative by stripping the leading
    slash.  Returns ``None`` when lexical normalization would escape the
    repository root via ``..`` (path traversal).
    """
    raw = declared.strip()
    if not raw:
        return None

    declared_path = PurePosixPath(raw)
    if declared_path.is_absolute() or raw.startswith(("/", "\\")):
        # Treat absolute-looking paths as repo-root-relative by stripping root.
        candidate = PurePosixPath(raw.lstrip("/\\"))
    elif str(base_dir) in ("", "."):
        candidate = declared_path
    else:
        candidate = base_dir / declared_path

    parts: list[str] = []
    for part in candidate.parts:
        if part in ("", ".", "/"):
            continue
        if part == "..":
            if not parts:
                return None
            parts.pop()
            continue
        parts.append(part)
    if not parts:
        return None
    return "/".join(parts)


def _contained_path(repo_root: Path, rel_path: str) -> Path | None:
    """Join *rel_path* under *repo_root*, rejecting escapes after resolve."""
    if not rel_path or rel_path.startswith(("/", "\\")):
        return None
    # Lexical rejection before touching the filesystem.
    if _resolve_rel(PurePosixPath("."), rel_path) is None:
        return None
    root = repo_root.resolve()
    candidate = (root / rel_path).resolve()
    if not candidate.is_relative_to(root):
        return None
    return candidate


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


def _load_toml(path: Path) -> ManifestData | None:
    try:
        with path.open("rb") as fh:
            data = tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        logger.debug("Failed to parse TOML %s: %s", path, exc)
        return None
    return data if isinstance(data, dict) else None


def _load_json(path: Path) -> ManifestData | None:
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
        abs_path = _contained_path(repo_root, node.file_path)
        if abs_path is None:
            continue
        try:
            text = abs_path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        node.line_end = max(1, text.count("\n") + (0 if text.endswith("\n") else 1 if text else 1))
