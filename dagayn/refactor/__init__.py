"""Graph-powered refactoring operations.

Provides dead code detection, refactoring suggestions, and the pending store
of rename previews. Previews and their application to source files come from
the Rust ``refactor_tool`` and ``apply_refactor_tool``: a preview-then-apply
workflow with expiry enforcement and path traversal prevention.
"""

from .dead_code import dead_code_report, find_dead_code
from .pending import REFACTOR_EXPIRY_SECONDS, _cleanup_expired, _pending_refactors, _refactor_lock
from .suggestions import suggest_refactorings

__all__ = [
    "REFACTOR_EXPIRY_SECONDS",
    "_cleanup_expired",
    "_pending_refactors",
    "_refactor_lock",
    "dead_code_report",
    "find_dead_code",
    "suggest_refactorings",
]
