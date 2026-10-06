from __future__ import annotations

import gzip
import hashlib
import io
import json
import os
import tarfile
from pathlib import Path

import pytest

from dagayn import vendor_grammars


def _make_tarball(files: dict[str, bytes]) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for name, content in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
    return buffer.getvalue()


class _Response(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()


def _hide_packaged_grammars(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(
        vendor_grammars,
        "get_packaged_grammar_root",
        lambda: tmp_path / "packaged-missing",
    )


def _write_required_fixture(source_dir: Path, spec: vendor_grammars.GrammarSpec) -> None:
    for rel_path in spec.required_paths:
        path = source_dir / rel_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(Path(rel_path).name, encoding="utf-8")
    vendor_grammars._write_source_marker(spec, source_dir)


def test_ensure_vendor_grammar_source_downloads_and_injects_markdown_binding(
    monkeypatch,
    tmp_path: Path,
):
    monkeypatch.setenv("DAGAYN_GRAMMAR_CACHE_DIR", str(tmp_path / "cache"))
    _hide_packaged_grammars(monkeypatch, tmp_path)

    subdir = vendor_grammars.GRAMMAR_SPECS["markdown"].source_subdirectory
    prefix = f"tree-sitter-markdown-archive/{subdir}/"
    tarball = _make_tarball(
        {
            f"{prefix}src/parser.c": b"parser",
            f"{prefix}src/scanner.c": b"scanner",
            f"{prefix}src/tree_sitter/alloc.h": b"alloc",
            f"{prefix}src/tree_sitter/array.h": b"array",
            f"{prefix}src/tree_sitter/parser.h": b"header",
        }
    )
    download_calls = {"count": 0}

    def fake_urlopen(request):
        assert request.full_url == vendor_grammars.GRAMMAR_SPECS["markdown"].archive_url
        download_calls["count"] += 1
        return _Response(tarball)

    monkeypatch.setattr(vendor_grammars, "urlopen", fake_urlopen)

    source_dir = vendor_grammars.ensure_vendor_grammar_source("markdown")

    assert source_dir.exists()
    assert (source_dir / "src" / "parser.c").read_text(encoding="utf-8") == "parser"
    binding_c = source_dir / "bindings" / "python" / "binding.c"
    assert binding_c.exists()
    assert "tree_sitter_markdown" in binding_c.read_text(encoding="utf-8")
    assert download_calls["count"] == 1


def test_ensure_vendor_grammar_source_reuses_cached_directory(monkeypatch, tmp_path: Path):
    cache_dir = tmp_path / "cache"
    monkeypatch.setenv("DAGAYN_GRAMMAR_CACHE_DIR", str(cache_dir))
    _hide_packaged_grammars(monkeypatch, tmp_path)

    spec = vendor_grammars.GRAMMAR_SPECS["terraform"]
    source_dir = cache_dir / spec.cache_dir_name
    _write_required_fixture(source_dir, spec)

    def fail_urlopen(_request):
        raise AssertionError("cache hit should not download")

    monkeypatch.setattr(vendor_grammars, "urlopen", fail_urlopen)

    assert vendor_grammars.ensure_vendor_grammar_source("terraform") == source_dir


def test_ensure_vendor_grammar_source_prefers_packaged_directory(monkeypatch, tmp_path: Path):
    packaged_root = tmp_path / "packaged"
    monkeypatch.setattr(vendor_grammars, "get_packaged_grammar_root", lambda: packaged_root)
    monkeypatch.setenv("DAGAYN_GRAMMAR_CACHE_DIR", str(tmp_path / "cache"))

    source_dir = packaged_root / "markdown"
    _write_required_fixture(source_dir, vendor_grammars.GRAMMAR_SPECS["markdown"])

    def fail_urlopen(_request):
        raise AssertionError("packaged grammar should not download")

    monkeypatch.setattr(vendor_grammars, "urlopen", fail_urlopen)

    assert vendor_grammars.ensure_vendor_grammar_source("markdown") == source_dir


def test_stage_packaged_vendor_grammar_sources_copies_required_files(monkeypatch, tmp_path: Path):
    monkeypatch.setenv("DAGAYN_GRAMMAR_CACHE_DIR", str(tmp_path / "cache"))
    _hide_packaged_grammars(monkeypatch, tmp_path)

    subdir = vendor_grammars.GRAMMAR_SPECS["markdown"].source_subdirectory
    prefix = f"tree-sitter-markdown-archive/{subdir}/"
    tarball = _make_tarball(
        {
            f"{prefix}src/parser.c": b"parser",
            f"{prefix}src/scanner.c": b"scanner",
            f"{prefix}src/tree_sitter/alloc.h": b"alloc",
            f"{prefix}src/tree_sitter/array.h": b"array",
            f"{prefix}src/tree_sitter/parser.h": b"header",
        }
    )

    def fake_urlopen(_request):
        return _Response(tarball)

    monkeypatch.setattr(vendor_grammars, "urlopen", fake_urlopen)

    staged = vendor_grammars.stage_packaged_vendor_grammar_sources(
        tmp_path / "bundle", ["markdown"]
    )
    source_dir = staged["markdown"]
    assert source_dir == (tmp_path / "bundle" / "markdown")
    for path in vendor_grammars.GRAMMAR_SPECS["markdown"].required_paths:
        assert (source_dir / path).exists()


def _terraform_tarball(parser: bytes = b"parser") -> bytes:
    return _make_tarball(
        {
            "tree-sitter-terraform-archive/src/parser.c": parser,
            "tree-sitter-terraform-archive/src/scanner.c": b"scanner",
            "tree-sitter-terraform-archive/src/tree_sitter/alloc.h": b"alloc",
            "tree-sitter-terraform-archive/src/tree_sitter/array.h": b"array",
            "tree-sitter-terraform-archive/src/tree_sitter/parser.h": b"header",
        }
    )


def _serve_terraform(monkeypatch) -> None:
    tarball = _terraform_tarball()
    monkeypatch.setattr(vendor_grammars, "urlopen", lambda _request: _Response(tarball))


def test_packaged_directory_from_another_pin_is_not_reused(monkeypatch, tmp_path: Path):
    packaged_root = tmp_path / "packaged"
    monkeypatch.setattr(vendor_grammars, "get_packaged_grammar_root", lambda: packaged_root)
    monkeypatch.setenv("DAGAYN_GRAMMAR_CACHE_DIR", str(tmp_path / "cache"))
    spec = vendor_grammars.GRAMMAR_SPECS["terraform"]
    stale = packaged_root / "terraform"
    _write_required_fixture(stale, spec)
    (stale / vendor_grammars.SOURCE_MARKER).write_text("0" * 40 + "\n", encoding="utf-8")
    _serve_terraform(monkeypatch)

    source_dir = vendor_grammars.ensure_vendor_grammar_source("terraform")

    assert source_dir == tmp_path / "cache" / spec.cache_dir_name
    assert (source_dir / "src" / "parser.c").read_bytes() == b"parser"
    assert (source_dir / vendor_grammars.SOURCE_MARKER).read_text(encoding="utf-8").strip() == (
        spec.commit
    )


def _write_patched_terraform(vendor_root: Path, parser: bytes, *, commit: str | None = None) -> str:
    patch_dir = vendor_root / "grammar-patches" / "terraform"
    patch_dir.mkdir(parents=True)
    (patch_dir / "0001-fix.patch").write_text("Fix.\n", encoding="utf-8")
    generated = vendor_root / "grammars" / "terraform"
    (generated / "src").mkdir(parents=True)
    (generated / "src" / "parser.c.gz").write_bytes(gzip.compress(parser, mtime=0))
    digest = vendor_grammars.grammar_patch_digest("terraform")
    assert digest is not None
    stamp = {
        "commit": commit or vendor_grammars.GRAMMAR_SPECS["terraform"].commit,
        "patches_sha256": digest,
        "files": {"src/parser.c": hashlib.sha256(parser).hexdigest()},
    }
    (generated / vendor_grammars.STAMP_NAME).write_text(json.dumps(stamp), encoding="utf-8")
    return digest


def test_patched_grammar_overlays_generated_files(monkeypatch, tmp_path: Path):
    monkeypatch.setattr(vendor_grammars, "VENDOR_ROOT", tmp_path / "vendor")
    monkeypatch.setenv("DAGAYN_GRAMMAR_CACHE_DIR", str(tmp_path / "cache"))
    _hide_packaged_grammars(monkeypatch, tmp_path)
    digest = _write_patched_terraform(tmp_path / "vendor", b"patched parser")
    _serve_terraform(monkeypatch)

    source_dir = vendor_grammars.ensure_vendor_grammar_source("terraform")

    assert source_dir.name.endswith(f"-patched-{digest[:12]}")
    assert (source_dir / "src" / "parser.c").read_bytes() == b"patched parser"
    assert (source_dir / "src" / "scanner.c").read_bytes() == b"scanner"
    assert (source_dir / vendor_grammars.SOURCE_MARKER).read_text(encoding="utf-8").strip() == (
        f"{vendor_grammars.GRAMMAR_SPECS['terraform'].commit}+patches.{digest}"
    )


def test_patched_grammar_generated_for_another_pin_fails(monkeypatch, tmp_path: Path):
    monkeypatch.setattr(vendor_grammars, "VENDOR_ROOT", tmp_path / "vendor")
    monkeypatch.setenv("DAGAYN_GRAMMAR_CACHE_DIR", str(tmp_path / "cache"))
    _hide_packaged_grammars(monkeypatch, tmp_path)
    _write_patched_terraform(tmp_path / "vendor", b"patched parser", commit="0" * 40)
    _serve_terraform(monkeypatch)

    with pytest.raises(OSError, match="regenerate_patched_grammars.py terraform"):
        vendor_grammars.ensure_vendor_grammar_source("terraform")


def test_patched_grammar_with_edited_patch_fails_check(monkeypatch, tmp_path: Path):
    monkeypatch.setattr(vendor_grammars, "VENDOR_ROOT", tmp_path / "vendor")
    _write_patched_terraform(tmp_path / "vendor", b"patched parser")
    patch = tmp_path / "vendor" / "grammar-patches" / "terraform" / "0001-fix.patch"
    patch.write_text("Fix, edited.\n", encoding="utf-8")

    problems = vendor_grammars.check_patched_grammar("terraform")

    assert problems and "does not match the current patches" in problems[0]


def test_committed_patched_grammars_match_their_stamps():
    for language in vendor_grammars.GRAMMAR_SPECS:
        assert vendor_grammars.check_patched_grammar(language) == []


def test_inject_assets_keeps_an_unchanged_binding(tmp_path: Path):
    spec = vendor_grammars.GRAMMAR_SPECS["terraform"]
    vendor_grammars._inject_assets(spec, tmp_path)
    binding = tmp_path / "bindings" / "python" / "binding.c"
    os.utime(binding, (1, 1))

    vendor_grammars._inject_assets(spec, tmp_path)

    assert binding.stat().st_mtime == 1
