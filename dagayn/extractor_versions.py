"""Extractor output versions recorded in graph metadata.

The Rust parser declares a version per extractor
(``crates/dagayn-parser/src/extractor_version.rs``). A graph stores the
versions it was parsed with under :data:`EXTRACTOR_VERSIONS_KEY`, as
``name=version`` pairs (``javascript=1``). When the running extractor is
newer than the stored stamp, files the extractor owns hold nodes and edges
that a fresh parse would no longer produce (renamed qualified names, for
example), so:

* :func:`outdated_extractors` names the extractors whose stamp is behind and
  that parsed files in the graph; a graph built before stamps existed counts
  as version 0,
* the incremental update re-parses every indexed file of those extractors
  and then records the current stamp, and
* the sync assessment reports ``commit_drift`` with ``extractor_drift`` until
  that update has run.
"""

from __future__ import annotations

import logging
from collections.abc import Callable, Iterable, Mapping
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

#: Graph metadata key holding the extractor versions the graph was parsed with.
EXTRACTOR_VERSIONS_KEY = "extractor_versions"


def current_extractor_versions() -> dict[str, tuple[int, frozenset[str]]]:
    """Return ``{extractor: (version, languages)}`` from the native parser.

    Empty when the native extension predates version stamps, which disables
    the whole mechanism rather than failing a build.
    """
    try:
        from . import _core
    except ImportError:
        return {}
    reader: Callable[[], list[tuple[str, int, list[str]]]] | None = getattr(
        _core, "extractor_versions", None
    )
    if not callable(reader):
        return {}
    return {
        str(name): (int(version), frozenset(str(language) for language in languages))
        for name, version, languages in reader()
    }


def format_extractor_versions(versions: Mapping[str, int]) -> str:
    """Serialize versions as sorted ``name=version`` pairs."""
    return ",".join(f"{name}={versions[name]}" for name in sorted(versions))


def parse_extractor_versions(raw: str | None) -> dict[str, int]:
    """Parse a stored stamp; malformed entries are ignored (read as version 0)."""
    versions: dict[str, int] = {}
    for item in (raw or "").split(","):
        name, sep, version = item.strip().partition("=")
        if not sep or not name:
            continue
        try:
            versions[name] = int(version)
        except ValueError:
            continue
    return versions


def outdated_extractors(store: Any, languages: Iterable[str] | None = None) -> list[str]:
    """Extractors whose stored version differs from the running one.

    A graph without a stamp was built before stamps existed and counts as
    version 0. Only extractors that parsed something in this graph count: an
    extractor whose languages are absent from *languages* (the graph's
    languages, read from ``store.get_stats()`` when not given) has no output
    to be stale. The statistics are only read when a stamp is behind, so the
    common case costs one metadata read.
    """
    current = current_extractor_versions()
    if not current:
        return []
    try:
        stored = parse_extractor_versions(store.get_metadata(EXTRACTOR_VERSIONS_KEY))
    except Exception:  # noqa: BLE001 — a metadata read failure is not drift
        logger.debug("Could not read extractor versions", exc_info=True)
        return []
    behind = sorted(
        name for name, (version, _) in current.items() if stored.get(name, 0) != version
    )
    if not behind:
        return []
    if languages is None:
        try:
            languages = getattr(store.get_stats(), "languages", None) or []
        except Exception:  # noqa: BLE001 — cannot tell; assume the output exists
            logger.debug("Could not read graph languages", exc_info=True)
            return behind
    present = {str(language) for language in languages}
    return [name for name in behind if current[name][1] & present]


def record_extractor_versions(store: Any) -> None:
    """Stamp the graph with the running extractor versions."""
    current = current_extractor_versions()
    if not current:
        return
    store.set_metadata(
        EXTRACTOR_VERSIONS_KEY,
        format_extractor_versions({name: version for name, (version, _) in current.items()}),
    )


def files_for_extractors(
    repo_root: Path, paths: Iterable[str], extractors: Iterable[str]
) -> list[str]:
    """Return the repo-relative *paths* whose language one of *extractors* parses."""
    from .parser.dispatch import detect_language

    current = current_extractor_versions()
    languages: set[str] = set()
    for name in extractors:
        entry = current.get(name)
        if entry is not None:
            languages.update(entry[1])
    if not languages:
        return []
    return sorted(path for path in paths if detect_language(repo_root / path) in languages)
