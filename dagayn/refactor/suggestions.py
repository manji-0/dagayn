"""Community-driven refactoring suggestions.

The suggestions come from ``dagayn_tools::suggestions`` in Rust, through the
store's ``suggest_refactorings_json``: the native ``refactor_tool`` and this
module produce them from one implementation.
"""

from __future__ import annotations

import json
from typing import Any

from ..graph import GraphStore

type SuggestionValue = Any
type SuggestionPayload = dict[str, SuggestionValue]


def suggest_refactorings(store: GraphStore) -> list[SuggestionPayload]:
    """Produce community-driven refactoring suggestions.

    Currently four categories:
    - **move**: Functions in Community A only called by Community B.
    - **remove**: Dead code, as :func:`dagayn.refactor.dead_code.find_dead_code`
      reports it.
    - **split**: Large complex functions/classes worth decomposing.
    - **document**: Public or complex code with low explanation density.

    Returns:
        List of suggestion dicts with type, description, symbols, rationale,
        before the stability policy ``refactor_tool`` applies.
    """
    suggestions: list[SuggestionPayload] = json.loads(store.suggest_refactorings_json())
    return suggestions
