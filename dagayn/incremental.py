"""Incremental graph update logic.

Detects changed files via git diff, re-parses only changed + impacted files,
and updates the graph accordingly. Also supports CLI invocation for hooks.
"""

from __future__ import annotations

import subprocess

from . import incremental_files as _incremental_files
from .incremental_build import (
    DependentList,
    StoreBatch,
    _init_worker,
    _parse_single_file,
    _rust_backend_available,
    _rust_backend_explicitly_requested,
    _serialize_store_batch,
    _single_hop_dependents,
    _split_rust_parser_files,
    find_dependents,
    find_dependents_for_files,
    full_build,
    incremental_update,
    watch,
)
from .incremental_files import (
    _RECURSE_SUBMODULES,
    DEFAULT_IGNORE_PATTERNS,
    _backend_selection,
    _git_branch_info,
    _is_binary,
    _load_ignore_patterns,
    _relativize_parsed_entities,
    _rust_backend_enabled,
    _should_ignore,
    _svn_revision_info,
    collect_all_files,
    detect_vcs,
    ensure_repo_gitignore_excludes_crg,
    find_project_root,
    find_repo_root,
    find_svn_root,
    get_all_tracked_files,
    get_changed_file_sources,
    get_changed_files,
    get_data_dir,
    get_db_path,
    get_staged_and_unstaged,
)

_incremental_files.subprocess = subprocess

__all__ = [
    "DEFAULT_IGNORE_PATTERNS",
    "DependentList",
    "StoreBatch",
    "_backend_selection",
    "_git_branch_info",
    "_init_worker",
    "_RECURSE_SUBMODULES",
    "_rust_backend_available",
    "_rust_backend_enabled",
    "_rust_backend_explicitly_requested",
    "_serialize_store_batch",
    "_split_rust_parser_files",
    "_is_binary",
    "_load_ignore_patterns",
    "_parse_single_file",
    "_relativize_parsed_entities",
    "_should_ignore",
    "_single_hop_dependents",
    "_svn_revision_info",
    "collect_all_files",
    "detect_vcs",
    "ensure_repo_gitignore_excludes_crg",
    "find_dependents",
    "find_dependents_for_files",
    "find_project_root",
    "find_repo_root",
    "find_svn_root",
    "full_build",
    "get_all_tracked_files",
    "get_changed_file_sources",
    "get_changed_files",
    "get_data_dir",
    "get_db_path",
    "get_staged_and_unstaged",
    "incremental_update",
    "watch",
]
