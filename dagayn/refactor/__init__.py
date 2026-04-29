"""Graph-powered refactoring operations.

Provides rename previews, dead code detection, refactoring suggestions,
and safe application of refactoring edits to source files. All file writes
go through a preview-then-apply workflow with expiry enforcement and path
traversal prevention.
"""

from .apply import apply_refactor
from .dead_code import find_dead_code
from .pending import REFACTOR_EXPIRY_SECONDS, _cleanup_expired, _pending_refactors, _refactor_lock
from .rename import rename_preview
from .suggestions import suggest_refactorings

__all__ = [
    "REFACTOR_EXPIRY_SECONDS",
    "_cleanup_expired",
    "_pending_refactors",
    "_refactor_lock",
    "apply_refactor",
    "find_dead_code",
    "rename_preview",
    "suggest_refactorings",
]
