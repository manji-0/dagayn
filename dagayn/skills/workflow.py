"""How an agent moves through dagayn's tools: the summary every dagayn skill,
the installed agent instructions, and the ``workflow`` section of
``docs/LLM-OPTIMIZED-REFERENCE.md`` share.

The contract behind it is ``docs/plans/AGENT-WORKFLOW-TARGET.md``. Skills
carry this summary verbatim between the markers below; ``tests/test_skills.py``
keeps the copies identical, so edit it here and in the skills together.
"""

from __future__ import annotations

WORKFLOW_START = "<!-- dagayn workflow -->"
WORKFLOW_END = "<!-- /dagayn workflow -->"

WORKFLOW = """\
Orient → locate → read → trace → judge → confirm. Enter at the phase the task
needs, and close the loop after an edit with `review_tool`.

1. **Orient**: `get_minimal_context_tool(task=...)` reports `sync.state` and
   the first calls for the task.
2. **Locate**: `semantic_search_nodes_tool` turns a description into a
   `qualified_name`.
3. **Read**: `query_graph_tool(pattern="source_of")` returns the live span.
4. **Trace**: `query_graph_tool` (`callers_of`, `callees_of`, `tests_for`,
   `docs_for`) and `flow_tool(mode="entry_points")`.
5. **Judge**: `review_tool`, `architecture_analysis_tool`, or
   `refactor_tool` answer with `findings`; an empty list means nothing to act
   on.
6. **Confirm**: `source_of` on each finding's place, a test, or a
   reproduction.

Every reply ends with `next`: at most three calls with complete arguments and
a `why`. Follow it unless the task points elsewhere; `[]` means the answer is
complete. `status="ambiguous"` puts one retry per candidate in `next`, and
`missingness` lists only the gaps that limit that reply. Full contract:
`get_docs_section_tool(section_name="workflow")`."""

WORKFLOW_BLOCK = f"{WORKFLOW_START}\n{WORKFLOW}\n{WORKFLOW_END}"
