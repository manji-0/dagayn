"""How much to trust a graph answer: the summary every dagayn skill and the
installed agent instructions share.

The full rules live in the ``trust`` section of
``docs/LLM-OPTIMIZED-REFERENCE.md`` (``get_docs_section_tool``). Skills carry
this summary verbatim between the markers below; ``tests/test_skills.py``
keeps the copies identical, so edit it here and in the skills together.
"""

from __future__ import annotations

TRUST_TIERS_START = "<!-- dagayn trust tiers -->"
TRUST_TIERS_END = "<!-- /dagayn trust tiers -->"

TRUST_TIERS = """\
Reach comes from the graph, correctness from `source_of`, and user-visible
effect from a reproduction; keep them apart in a claim. Full rules:
`get_docs_section_tool(section_name="trust")`.

- **Highest** — on a current graph (`sync.state` is `commit_synced` or
  `worktree_ahead`): `HIGH` and `EXTRACTED` edges, whether the parser or a
  SCIP index (`resolved_by: "scip"`) settled them; `source_of` spans;
  authored doc contracts (`implemented_by` / `implements_contract` links whose
  target exists — `evidence_type=authored` alone is on every Markdown result).
- **Medium** — structure, not correctness: `MEDIUM` (inferred) edges,
  `reason_codes`, blast radius, flows, communities, metrics, suggestions,
  explanatory doc links, and search hits.
- **Low** — a hypothesis until `source_of` or a reproduction confirms it:
  `LOW` edges, `heuristic_reachable` doc links whatever their edge confidence,
  a file-level `tests_for` of 0, `truncated` or `ambiguous` results, and
  answers about files changed since the graph was built (`sync.state`
  `commit_drift` or `worktree_behind`)."""

TRUST_TIERS_BLOCK = f"{TRUST_TIERS_START}\n{TRUST_TIERS}\n{TRUST_TIERS_END}"
