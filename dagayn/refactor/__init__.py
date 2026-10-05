"""Graph-powered refactoring operations.

Provides dead code detection, refactoring suggestions, and safe application
of refactoring edits to source files. Rename previews come from the Rust
``refactor_tool``; all file writes go through a preview-then-apply workflow
with expiry enforcement and path traversal prevention.
"""

from .apply import apply_refactor
from .dead_code import dead_code_report, find_dead_code
from .pending import REFACTOR_EXPIRY_SECONDS, _cleanup_expired, _pending_refactors, _refactor_lock
from .suggestions import suggest_refactorings

__all__ = [
    "REFACTOR_EXPIRY_SECONDS",
    "_cleanup_expired",
    "_pending_refactors",
    "_refactor_lock",
    "apply_refactor",
    "dead_code_report",
    "find_dead_code",
    "suggest_refactorings",
]
