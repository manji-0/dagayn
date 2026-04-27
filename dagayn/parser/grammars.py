"""Vendored grammar loading and tree-sitter parser factory."""

from __future__ import annotations

import hashlib
import importlib.util
import logging
import shlex
import subprocess
import sys
import sysconfig
from pathlib import Path
from typing import Any, Optional

try:
    import tree_sitter_language_pack as tslp
except ImportError:  # pragma: no cover
    tslp: Any | None = None

try:
    from tree_sitter import Language as _TreeSitterLanguage
    from tree_sitter import Parser as _TreeSitterParser
except ImportError:  # pragma: no cover
    Language: Any | None = None
    Parser: Any | None = None
else:
    Language: Any | None = _TreeSitterLanguage
    Parser: Any | None = _TreeSitterParser

from ..vendor_grammars import ensure_vendor_grammar_source

_MARKDOWN_BINDING_MODULE = "markdown"
_TERRAFORM_BINDING_MODULE = "terraform"

logger = logging.getLogger(__name__)


def file_hash(path: Path) -> str:
    """SHA-256 hash of file contents."""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _set_tree_sitter_language(parser: Any, language_obj: Any) -> None:
    try:
        parser.language = language_obj
    except AttributeError:
        getattr(parser, "set_language")(language_obj)


def _binding_dir(vendor_dir: Path) -> Path:
    return vendor_dir / "bindings" / "python"


def _binding_candidates(vendor_dir: Path, module_name: str) -> tuple[Path, Path]:
    binding_dir = _binding_dir(vendor_dir)
    return (
        binding_dir / f"{module_name}.abi3.so",
        binding_dir / f"{module_name}.so",
    )


def _ensure_compiled_vendored_binding(
    *,
    language: str,
    module_name: str,
    display_name: str,
) -> Optional[Path]:
    try:
        vendor_dir = ensure_vendor_grammar_source(language)
    except (KeyError, OSError) as exc:
        logger.warning("failed to prepare pinned %s grammar sources: %s", display_name, exc)
        return None

    binding_dir = _binding_dir(vendor_dir)
    for candidate in _binding_candidates(vendor_dir, module_name):
        if candidate.exists():
            return candidate

    binding_c = binding_dir / "binding.c"
    parser_c = vendor_dir / "src" / "parser.c"
    scanner_c = vendor_dir / "src" / "scanner.c"
    header_dir = vendor_dir / "src" / "tree_sitter"
    output_path = binding_dir / f"{module_name}.abi3.so"

    required = (binding_c, parser_c, header_dir)
    if not all(path.exists() for path in required):
        return None
    if sys.platform.startswith("win"):
        logger.warning("pinned %s binding auto-build is unsupported on Windows", display_name)
        return None

    include_dirs = []
    include_path = sysconfig.get_paths().get("include")
    platinclude_path = sysconfig.get_paths().get("platinclude")
    for include_dir in (include_path, platinclude_path):
        if include_dir and include_dir not in include_dirs:
            include_dirs.append(include_dir)

    cmd = [
        "-shared",
        "-fPIC",
        "-O2",
        "-std=c11",
        "-DPy_LIMITED_API=0x030A0000",
        "-I",
        str(vendor_dir / "src"),
        "-I",
        str(header_dir),
    ]
    compiler = shlex.split(sysconfig.get_config_var("CC") or "cc")
    for include_dir in include_dirs:
        cmd.extend(["-I", include_dir])
    if sys.platform == "darwin":
        cmd.extend(["-undefined", "dynamic_lookup"])

    sources = [str(binding_c), str(parser_c)]
    if scanner_c.exists():
        sources.append(str(scanner_c))
    cmd = compiler + cmd + sources + ["-o", str(output_path)]

    binding_dir.mkdir(parents=True, exist_ok=True)
    try:
        subprocess.run(
            cmd,
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        logger.warning("failed to build pinned %s binding: %s", display_name, exc)
        if isinstance(exc, subprocess.CalledProcessError):
            logger.debug("%s binding build stdout: %s", display_name, exc.stdout)
            logger.debug("%s binding build stderr: %s", display_name, exc.stderr)
        return None
    return output_path if output_path.exists() else None


def _load_vendored_markdown_language() -> Any | None:
    if Language is None:
        return None
    module = _load_vendored_markdown_binding()
    if module is None or not hasattr(module, "language"):
        return None
    try:
        return Language(module.language())
    except (TypeError, ValueError) as exc:
        logger.warning("failed to load vendored Markdown language capsule: %s", exc)
        return None


def _load_vendored_markdown_binding() -> Any | None:
    binding_path = _ensure_compiled_vendored_binding(
        language="markdown",
        module_name=_MARKDOWN_BINDING_MODULE,
        display_name="Markdown",
    )
    if binding_path is None:
        return None
    try:
        spec = importlib.util.spec_from_file_location(
            _MARKDOWN_BINDING_MODULE,
            binding_path,
        )
        if spec is None or spec.loader is None:
            raise ImportError(f"could not create import spec for {binding_path}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    except (ImportError, OSError) as exc:
        logger.warning("failed to import vendored Markdown binding %s: %s", binding_path, exc)
        return None


def get_markdown_parser() -> Any | None:
    """Prefer dagayn's pinned Markdown grammar, then fall back to tslp."""
    language_obj = _load_vendored_markdown_language()
    if language_obj is not None and Parser is not None:
        try:
            parser_factory: Any = Parser
            parser = parser_factory()
            _set_tree_sitter_language(parser, language_obj)
            return parser
        except (AttributeError, TypeError, ValueError) as exc:
            logger.warning("failed to initialize vendored Markdown parser: %s", exc)
    try:
        if tslp is None:
            raise ImportError("tree_sitter_language_pack is not installed")
        return tslp.get_parser("markdown")
    except (LookupError, ValueError, ImportError) as exc:
        logger.debug("fallback Markdown parser unavailable: %s", exc)
        return None


def _load_vendored_terraform_language() -> Any | None:
    if Language is None:
        return None
    module = _load_vendored_terraform_binding()
    if module is None or not hasattr(module, "language"):
        return None
    try:
        return Language(module.language())
    except (TypeError, ValueError) as exc:
        logger.warning("failed to load vendored Terraform language capsule: %s", exc)
        return None


def _load_vendored_terraform_binding() -> Any | None:
    binding_path = _ensure_compiled_vendored_binding(
        language="terraform",
        module_name=_TERRAFORM_BINDING_MODULE,
        display_name="Terraform",
    )
    if binding_path is None:
        return None
    try:
        spec = importlib.util.spec_from_file_location(
            _TERRAFORM_BINDING_MODULE,
            binding_path,
        )
        if spec is None or spec.loader is None:
            raise ImportError(f"could not create import spec for {binding_path}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    except (ImportError, OSError) as exc:
        logger.warning("failed to import vendored Terraform binding %s: %s", binding_path, exc)
        return None


def get_terraform_parser() -> Any | None:
    """Prefer dagayn's pinned Terraform grammar, then fall back to tslp."""
    language_obj = _load_vendored_terraform_language()
    if language_obj is not None and Parser is not None:
        try:
            parser_factory: Any = Parser
            parser = parser_factory()
            _set_tree_sitter_language(parser, language_obj)
            return parser
        except (AttributeError, TypeError, ValueError) as exc:
            logger.warning("failed to initialize vendored Terraform parser: %s", exc)
    try:
        if tslp is None:
            raise ImportError("tree_sitter_language_pack is not installed")
        return tslp.get_parser("terraform")
    except (LookupError, ValueError, ImportError) as exc:
        logger.debug("fallback Terraform parser unavailable: %s", exc)
        return None


def get_parser(language: str, cache: dict[str, Any]) -> Any | None:
    """Get or create a tree-sitter parser for language, storing it in cache."""
    if language in cache:
        return cache[language]

    if language == "markdown":
        parser = get_markdown_parser()
        if parser is None:
            return None
        cache[language] = parser
        return parser

    if language == "terraform":
        parser = get_terraform_parser()
        if parser is None:
            return None
        cache[language] = parser
        return parser

    try:
        if tslp is None:
            raise ImportError("tree_sitter_language_pack is not installed")
        cache[language] = tslp.get_parser(language)
    except (LookupError, ValueError, ImportError) as exc:
        logger.debug("tree-sitter parser unavailable for %s: %s", language, exc)
        return None
    return cache[language]
