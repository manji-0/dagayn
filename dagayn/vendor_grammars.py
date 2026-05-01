from __future__ import annotations

import os
import shutil
import sys
import tarfile
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable
from urllib.request import Request, urlopen


def _generate_binding_c(language: str) -> str:
    sym = f"tree_sitter_{language}"
    init = f"PyInit_{language}"
    return f"""#define PY_SSIZE_T_CLEAN
#include <Python.h>

typedef struct TSLanguage TSLanguage;

const TSLanguage *{sym}(void);

static PyObject *language(PyObject *self, PyObject *args) {{
    (void)self;
    (void)args;
    return PyCapsule_New(
        (void *){sym}(),
        "tree_sitter.Language",
        NULL
    );
}}

static PyMethodDef methods[] = {{
    {{"language", language, METH_NOARGS, "Return the tree-sitter Language capsule."}},
    {{NULL, NULL, 0, NULL}},
}};

static struct PyModuleDef module = {{
    PyModuleDef_HEAD_INIT,
    "{language}",
    NULL,
    -1,
    methods,
}};

PyMODINIT_FUNC {init}(void) {{
    return PyModule_Create(&module);
}}
"""


@dataclass(frozen=True)
class GrammarSpec:
    language: str
    owner: str
    repo: str
    commit: str
    required_paths: tuple[str, ...]
    inject_python_binding: bool = False
    source_subdirectory: str | None = None
    parser_subdirectory: str | None = None

    @property
    def archive_url(self) -> str:
        return f"https://codeload.github.com/{self.owner}/{self.repo}/tar.gz/{self.commit}"

    @property
    def cache_dir_name(self) -> str:
        return f"{self.repo}-{self.commit}"


GRAMMAR_SPECS: dict[str, GrammarSpec] = {
    "markdown": GrammarSpec(
        language="markdown",
        owner="manji-0",
        repo="tree-sitter-markdown",
        commit="13a2b8bb44965b75ddba5e70f16411c18e6f09fe",
        required_paths=(
            "src/parser.c",
            "src/scanner.c",
            "src/tree_sitter/alloc.h",
            "src/tree_sitter/array.h",
            "src/tree_sitter/parser.h",
            "bindings/python/binding.c",
        ),
        inject_python_binding=True,
        source_subdirectory="vendor/tree-sitter-markdown/tree-sitter-markdown",
    ),
    "terraform": GrammarSpec(
        language="terraform",
        owner="manji-0",
        repo="tree-sitter-terraform",
        commit="5a5b258a71290999ce58797eafeaa098b2d450b9",
        required_paths=(
            "src/parser.c",
            "src/scanner.c",
            "src/tree_sitter/alloc.h",
            "src/tree_sitter/array.h",
            "src/tree_sitter/parser.h",
            "bindings/python/binding.c",
        ),
        inject_python_binding=True,
    ),
    "rust": GrammarSpec(
        language="rust",
        owner="tree-sitter",
        repo="tree-sitter-rust",
        commit="77a3747266f4d621d0757825e6b11edcbf991ca5",
        required_paths=(
            "src/parser.c",
            "src/scanner.c",
            "src/tree_sitter/alloc.h",
            "src/tree_sitter/array.h",
            "src/tree_sitter/parser.h",
            "bindings/python/binding.c",
        ),
        inject_python_binding=True,
    ),
    "python": GrammarSpec(
        language="python",
        owner="tree-sitter",
        repo="tree-sitter-python",
        commit="26855eabccb19c6abf499fbc5b8dc7cc9ab8bc64",
        required_paths=(
            "src/parser.c",
            "src/scanner.c",
            "src/tree_sitter/alloc.h",
            "src/tree_sitter/array.h",
            "src/tree_sitter/parser.h",
            "bindings/python/binding.c",
        ),
        inject_python_binding=True,
    ),
    "javascript": GrammarSpec(
        language="javascript",
        owner="tree-sitter",
        repo="tree-sitter-javascript",
        commit="58404d8cf191d69f2674a8fd507bd5776f46cb11",
        required_paths=(
            "src/parser.c",
            "src/scanner.c",
            "src/tree_sitter/alloc.h",
            "src/tree_sitter/array.h",
            "src/tree_sitter/parser.h",
            "bindings/python/binding.c",
        ),
        inject_python_binding=True,
    ),
    "typescript": GrammarSpec(
        language="typescript",
        owner="tree-sitter",
        repo="tree-sitter-typescript",
        commit="75b3874edb2dc714fb1fd77a32013d0f8699989f",
        required_paths=(
            "typescript/src/parser.c",
            "typescript/src/scanner.c",
            "typescript/src/tree_sitter/alloc.h",
            "typescript/src/tree_sitter/array.h",
            "typescript/src/tree_sitter/parser.h",
            "common/scanner.h",
            "bindings/python/binding.c",
        ),
        inject_python_binding=True,
        parser_subdirectory="typescript",
    ),
    "tsx": GrammarSpec(
        language="tsx",
        owner="tree-sitter",
        repo="tree-sitter-typescript",
        commit="75b3874edb2dc714fb1fd77a32013d0f8699989f",
        required_paths=(
            "tsx/src/parser.c",
            "tsx/src/scanner.c",
            "tsx/src/tree_sitter/alloc.h",
            "tsx/src/tree_sitter/array.h",
            "tsx/src/tree_sitter/parser.h",
            "common/scanner.h",
            "bindings/python/binding.c",
        ),
        inject_python_binding=True,
        parser_subdirectory="tsx",
    ),
    "bash": GrammarSpec(
        language="bash",
        owner="tree-sitter",
        repo="tree-sitter-bash",
        commit="a06c2e4415e9bc0346c6b86d401879ffb44058f7",
        required_paths=(
            "src/parser.c",
            "src/scanner.c",
            "src/tree_sitter/parser.h",
            "bindings/python/binding.c",
        ),
        inject_python_binding=True,
    ),
}


def get_grammar_cache_root() -> Path:
    override = os.environ.get("DAGAYN_GRAMMAR_CACHE_DIR")
    if override:
        return Path(override).expanduser().resolve()
    if os.name == "nt" and (local_app_data := os.environ.get("LOCALAPPDATA")):
        return Path(local_app_data) / "dagayn" / "grammars"
    home = Path.home()
    if os.name == "nt":
        return home / "AppData" / "Local" / "dagayn" / "grammars"
    if sys.platform == "darwin":
        return home / "Library" / "Caches" / "dagayn" / "grammars"
    xdg_cache = os.environ.get("XDG_CACHE_HOME")
    if xdg_cache:
        return Path(xdg_cache).expanduser().resolve() / "dagayn" / "grammars"
    return home / ".cache" / "dagayn" / "grammars"


def get_packaged_grammar_root() -> Path:
    return Path(__file__).resolve().parent / "_vendor_grammars"


def get_packaged_grammar_source(language: str) -> Path | None:
    spec = GRAMMAR_SPECS[language]
    target_dir = get_packaged_grammar_root() / language
    if _is_ready(spec, target_dir):
        _inject_assets(spec, target_dir)
        return target_dir
    return None


def ensure_vendor_grammar_source(language: str) -> Path:
    if packaged := get_packaged_grammar_source(language):
        return packaged

    spec = GRAMMAR_SPECS[language]
    cache_root = get_grammar_cache_root()
    target_dir = cache_root / spec.cache_dir_name

    if _is_ready(spec, target_dir):
        _inject_assets(spec, target_dir)
        return target_dir

    if target_dir.exists():
        shutil.rmtree(target_dir)

    cache_root.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix=f"{spec.language}-grammar-", dir=cache_root) as tmp_dir:
        tmp_root = Path(tmp_dir)
        archive_path = tmp_root / "source.tar.gz"
        extracted_dir = tmp_root / "extracted"
        allowed = frozenset(Path(p).parts[0] for p in spec.required_paths)
        _download_archive(spec, archive_path)
        _extract_archive(
            archive_path,
            extracted_dir,
            source_subdirectory=spec.source_subdirectory,
            allowed_toplevel_dirs=allowed,
        )
        _inject_assets(spec, extracted_dir)
        _validate_required_paths(spec, extracted_dir)

        if target_dir.exists():
            if _is_ready(spec, target_dir):
                return target_dir
            shutil.rmtree(target_dir)
        shutil.move(str(extracted_dir), str(target_dir))

    return target_dir


def ensure_all_vendor_grammar_sources(
    languages: Iterable[str] | None = None,
) -> dict[str, Path]:
    requested = tuple(languages or GRAMMAR_SPECS)
    return {language: ensure_vendor_grammar_source(language) for language in requested}


def stage_packaged_vendor_grammar_sources(
    destination_root: Path,
    languages: Iterable[str] | None = None,
) -> dict[str, Path]:
    requested = tuple(languages or GRAMMAR_SPECS)
    destination_root.mkdir(parents=True, exist_ok=True)

    staged: dict[str, Path] = {}
    for language in requested:
        spec = GRAMMAR_SPECS[language]
        source_dir = ensure_vendor_grammar_source(language)
        target_dir = destination_root / language
        if target_dir.exists():
            shutil.rmtree(target_dir)
        allowed = frozenset(Path(p).parts[0] for p in spec.required_paths)
        source_dir_str = str(source_dir)

        def _ignore_extras(directory: str, contents: list[str]) -> set[str]:
            if directory != source_dir_str:
                return set()
            return {
                name for name in contents if name not in allowed and (source_dir / name).is_dir()
            }

        shutil.copytree(source_dir, target_dir, ignore=_ignore_extras)
        _inject_assets(spec, target_dir)
        _validate_required_paths(spec, target_dir)
        staged[language] = target_dir
    return staged


def _download_archive(spec: GrammarSpec, archive_path: Path) -> None:
    url = spec.archive_url
    if not url.startswith("https://"):
        raise ValueError(f"refusing to fetch grammar from non-HTTPS URL: {url}")
    request = Request(url, headers={"User-Agent": "dagayn-grammar-fetch/1.0"})
    with urlopen(request) as response, archive_path.open("wb") as fh:  # nosec B310
        shutil.copyfileobj(response, fh)


def _extract_archive(
    archive_path: Path,
    destination: Path,
    *,
    source_subdirectory: str | None = None,
    allowed_toplevel_dirs: frozenset[str] | None = None,
) -> None:
    subdir_parts = Path(source_subdirectory).parts if source_subdirectory else ()
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive_path, "r:gz") as archive:
        members = [member for member in archive.getmembers() if member.isdir() or member.isfile()]
        root_name = _archive_root_name(members)
        for member in members:
            rel_path = _relative_member_path(member.name, root_name)
            if rel_path is None:
                continue
            if subdir_parts:
                if rel_path.parts[: len(subdir_parts)] != subdir_parts:
                    continue
                remaining = rel_path.parts[len(subdir_parts) :]
                if not remaining:
                    continue
                rel_path = Path(*remaining)
            if allowed_toplevel_dirs is not None and rel_path.parts[0] not in allowed_toplevel_dirs:
                continue
            target = destination / rel_path
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            source = archive.extractfile(member)
            if source is None:
                raise OSError(f"failed to read archive member {member.name}")
            with source, target.open("wb") as fh:
                shutil.copyfileobj(source, fh)


def _archive_root_name(members: list[tarfile.TarInfo]) -> str:
    roots = {Path(member.name).parts[0] for member in members if Path(member.name).parts}
    if len(roots) != 1:
        raise OSError(f"unexpected archive layout: {sorted(roots)}")
    return next(iter(roots))


def _relative_member_path(member_name: str, root_name: str) -> Path | None:
    parts = Path(member_name).parts
    if not parts:
        return None
    if parts[0] != root_name:
        raise OSError(f"unexpected archive member root: {member_name}")
    rel_parts = parts[1:]
    if not rel_parts:
        return None
    if any(part == ".." for part in rel_parts):
        raise OSError(f"unsafe archive path: {member_name}")
    return Path(*rel_parts)


def _inject_assets(spec: GrammarSpec, destination: Path) -> None:
    if not spec.inject_python_binding:
        return
    binding_path = destination / "bindings" / "python" / "binding.c"
    binding_path.parent.mkdir(parents=True, exist_ok=True)
    binding_path.write_text(_generate_binding_c(spec.language), encoding="utf-8")


def _validate_required_paths(spec: GrammarSpec, destination: Path) -> None:
    missing = [path for path in spec.required_paths if not (destination / path).exists()]
    if missing:
        raise OSError(
            f"fetched {spec.language} grammar is missing required files: {', '.join(missing)}"
        )


def _is_ready(spec: GrammarSpec, destination: Path) -> bool:
    return destination.exists() and all(
        (destination / path).exists() for path in spec.required_paths
    )


def main() -> int:
    for language, path in ensure_all_vendor_grammar_sources().items():
        print(f"{language}: {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
