"""MCP prompt templates for Dagayn.

Provides 5 pre-built prompt workflows, all starting from
get_minimal_context_tool and preferring detail_level="minimal" first.

1. review_changes   - pre-commit review using review_tool
2. architecture_map - architecture docs using communities, flows, Mermaid
3. debug_issue      - guided debugging using search, flow tracing
4. onboard_developer - new dev orientation using architecture and flows
5. pre_merge_check  - PR readiness from review findings, tests, dead code
"""

from __future__ import annotations

from typing import Literal, TypedDict


class PromptMessage(TypedDict):
    role: Literal["user"]
    content: str


_TOKEN_EFFICIENCY_PREAMBLE = (  # nosec B105 — prompt template, not a password
    """\
## Rules for Token-Efficient Graph Usage
1. ALWAYS call `get_minimal_context_tool` first with a task description.
2. Start with `detail_level="minimal"` where a tool offers it; escalate to \
`"standard"` only for the entities that need deeper inspection, or when you \
need a complete list.
3. Prefer targeted queries (query_graph_tool with a specific symbol) over broad \
architecture_analysis drill-downs.
4. Graph reach is not correctness: treat reason codes, blast radius, flows, and \
search hits as leads, and confirm a claim with \
`query_graph_tool(pattern="source_of")` or a reproduction before stating it.
"""
)


def review_changes_prompt(base: str = "HEAD~1") -> list[PromptMessage]:
    """Pre-commit review workflow.

    Args:
        base: Git ref to diff against. Default: HEAD~1.
    """
    return [
        {
            "role": "user",
            "content": (
                f"{_TOKEN_EFFICIENCY_PREAMBLE}\n"
                f"## Review Workflow\n"
                f'1. Call `get_minimal_context_tool(task="review changes against '
                f'{base}")`.\n'
                f'2. Call `review_tool(mode="changes", base="{base}", detail_level="minimal")` '
                f"and read `findings`: what to check that the diff does not show. "
                f"An empty list means nothing beyond the diff needs checking: "
                f"report the summary and stop.\n"
                f"3. For each finding:\n"
                f"   a. dangling_reference or unchanged_caller: open each listed site "
                f'with `query_graph_tool(pattern="source_of", target=<site>)`.\n'
                f"   b. contract_doc_not_updated or bridge_touched: read the doc section "
                f"or the other side of the bridge with `source_of`.\n"
                f"   c. untested_change: confirm with "
                f'`query_graph_tool(pattern="tests_for", target=<function>, '
                f'detail_level="minimal")`.\n'
                f"   d. tests_to_run: run its `command`.\n"
                f'4. Call `review_tool(mode="affected_flows", base="{base}", '
                f'detail_level="minimal")` '
                f"only if a finding raises a flow question.\n"
                f"5. Summarize: what changed, each confirmed finding with its action, "
                f"and the tests to run.\n\n"
                f'Do NOT call review_tool(mode="context") unless you need '
                f"source code snippets for a specific function."
            ),
        }
    ]


def architecture_map_prompt() -> list[PromptMessage]:
    """Architecture documentation workflow."""
    return [
        {
            "role": "user",
            "content": (
                f"{_TOKEN_EFFICIENCY_PREAMBLE}\n"
                "## Architecture Mapping Workflow\n"
                '1. Call `get_minimal_context_tool(task="map architecture")`.\n'
                '2. Call `architecture_analysis_tool(mode="overview", '
                'detail_level="minimal")` for community coupling summary.\n'
                '3. Call `flow_tool(mode="list", detail_level="minimal")` for critical '
                "flow names + criticality scores.\n"
                '4. Only call `architecture_analysis_tool(mode="community", '
                "community_name=<X>)` for the 1-2 communities the user is most "
                "interested in.\n"
                "5. Produce a concise Mermaid diagram showing communities as "
                "boxes and key flows as arrows."
            ),
        }
    ]


def debug_issue_prompt(description: str = "") -> list[PromptMessage]:
    """Guided debugging workflow.

    Args:
        description: Description of the issue to debug.
    """
    desc_part = description or "<description>"
    return [
        {
            "role": "user",
            "content": (
                f"{_TOKEN_EFFICIENCY_PREAMBLE}\n"
                "## Debug Workflow\n"
                f'1. Call `get_minimal_context_tool(task="debug: '
                f'{desc_part}")`.\n'
                "2. Call `semantic_search_nodes_tool(query=<keywords from "
                'description>, detail_level="minimal", limit=5)`.\n'
                "3. For the top 1-2 results, call "
                '`query_graph_tool(pattern="callers_of", target=<name>, '
                'detail_level="minimal")`.\n'
                "4. If the issue involves a reachable set from an entry point: call "
                '`flow_tool(mode="get", flow_name=<relevant flow>)` for the single most '
                "relevant flow.\n"
                '5. Only call `review_tool(mode="context")` or `review_tool(mode="impact")` '
                "if you need to trace the blast radius of a specific change."
            ),
        }
    ]


def onboard_developer_prompt() -> list[PromptMessage]:
    """New developer orientation workflow."""
    return [
        {
            "role": "user",
            "content": (
                f"{_TOKEN_EFFICIENCY_PREAMBLE}\n"
                "## Onboarding Workflow\n"
                '1. Call `get_minimal_context_tool(task="onboard developer")`.\n'
                '2. Call `architecture_analysis_tool(mode="overview", '
                'detail_level="minimal")` for the 30-second mental model.\n'
                '3. Call `architecture_analysis_tool(mode="communities", '
                'detail_level="minimal")` — '
                "present as a table of module names + sizes.\n"
                '4. Call `flow_tool(mode="list", detail_level="minimal")` — highlight '
                "the top 3 critical flows.\n"
                "5. Only drill into a specific community or flow if the "
                "developer asks."
            ),
        }
    ]


def pre_merge_check_prompt(base: str = "HEAD~1") -> list[PromptMessage]:
    """PR readiness check workflow.

    Args:
        base: Git ref to diff against. Default: HEAD~1.
    """
    return [
        {
            "role": "user",
            "content": (
                f"{_TOKEN_EFFICIENCY_PREAMBLE}\n"
                "## Pre-Merge Check Workflow\n"
                f'1. Call `get_minimal_context_tool(task="pre-merge check against {base}")`.\n'
                f'2. Call `review_tool(mode="changes", base="{base}", detail_level="minimal")` '
                "and read `findings` and `findings_omitted`. An empty list means "
                "nothing beyond the diff needs checking.\n"
                "3. For each dangling_reference or unchanged_caller finding: open its "
                'sites with `query_graph_tool(pattern="source_of", target=<site>)`; '
                "a site still using the old name or signature is a NO-GO.\n"
                "4. For each untested_change finding: call "
                '`query_graph_tool(pattern="tests_for", '
                'target=<each untested function>, detail_level="minimal")` '
                "for up to 3 functions.\n"
                "5. Run the `command` of each tests_to_run finding; check each "
                "contract_doc_not_updated and bridge_touched finding.\n"
                '6. Call `refactor_tool(mode="dead_code")` to check for newly dead code.\n'
                "7. Output: GO/NO-GO recommendation with 1-sentence "
                "justification + list of required follow-ups."
            ),
        }
    ]
