from __future__ import annotations

import io
import tarfile
from pathlib import Path

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

    def __exit__(self, exc_type, exc, tb):
        self.close()
        return False


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
