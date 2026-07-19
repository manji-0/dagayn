"""Unit tests for tools/embedding_material_model_benchmark helpers."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

_SCRIPT = Path(__file__).resolve().parents[1] / "tools" / "embedding_material_model_benchmark.py"


def _load_benchmark_module():
    spec = importlib.util.spec_from_file_location(
        "embedding_material_model_benchmark",
        _SCRIPT,
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_provider_accepts_openai_compatible_spec():
    mod = _load_benchmark_module()
    provider = mod._provider(
        "openai:bge-m3-gguf-q8_0@http://127.0.0.1:18080/v1",
        openai_batch_size=4,
    )
    assert provider.name.startswith("openai:bge-m3-gguf-q8_0@")
    assert "127.0.0.1:18080" in provider.name


def test_provider_rejects_removed_local_spec():
    mod = _load_benchmark_module()
    with pytest.raises(ValueError, match="in-process local: specs were removed"):
        mod._provider("local:BAAI/bge-m3", openai_batch_size=4)


def test_default_models_are_openai_compatible_only():
    mod = _load_benchmark_module()
    assert mod.DEFAULT_MODELS
    assert all(spec.startswith("openai:") for spec in mod.DEFAULT_MODELS)
    assert all("@" in spec for spec in mod.DEFAULT_MODELS)
