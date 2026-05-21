# Agent Interaction Criteria Review, 2026-05-05

<!-- derived-from ./mcp-tool-heuristic-review-2026-05-05.md -->
<!-- constrained-by ../COMMANDS.md#mcp-tools -->
<!-- Method context: ../../AGENTS.md#how-agents-should-work-with-this-repo; not a graph dependency because AGENTS.md is root-level agent guidance. -->

## Scope

This review covers the content dagayn gives to AI agents: repo-local
`AGENTS.md`, generated instruction sections installed through `dagayn init`,
and packaged `skills/*/SKILL.md` workflows.

The goal is to make agent behavior logically defensible: graph tools should be
used first because they expose structure cheaply, but agents must still judge
each insight by its metric, threshold, reason code, truncation state, and source
verification needs.

## Evaluation Criteria

| Area | Quantitative Criteria | Qualitative Criteria |
| --- | --- | --- |
| Graph-first navigation | Node/edge counts, changed-file count, impacted-node count, result count, token budget | Use graph tools before broad reads; fall back when stale, ambiguous, truncated, or source text is needed. |
| Review risk | Risk score, affected flows, blast radius, tests found via `tests_for`, changed public surfaces | Risk labels are prioritization, not proof of a bug. Behavioral findings require source or test evidence. |
| Architecture analysis | Degree, betweenness, community counts, cycle severity, instability, abstractness, distance from main sequence | Explain which principle or reason code makes the result interesting, and mention approximation limits. |
| Knowledge gaps | Isolated degree, community size, p95 production-candidate degree threshold, raw/returned counts | Treat gaps as review leads; exclude docs/tests from production test-risk claims. |
| Refactoring | Candidate count, callers, communities, edit count, dry-run diff size, truncation state | Verify public APIs, dynamic dispatch, generated code, framework entry points, and tests before editing. |
| Markdown/agent docs | Resolved directives, imports, section count, impact radius | Dependencies should be real signal. Do not invent directive comments for decoration. |

## Findings

### High: agent instructions lacked an explicit evidence standard

The previous generated MCP section strongly instructed agents to use dagayn
before Grep/Glob/Read. That was directionally correct, but it did not say how
to evaluate graph-derived claims. This could lead agents to treat structural
signals such as degree, centrality, or test gaps as automatic conclusions.

The generated instructions now state that graph insights are evidence-ranked
leads and tell agents to cite thresholds, counts, reason codes, and truncation
state when drawing conclusions.

### Medium: skills started with different entry points

Some skills started directly with `list_graph_stats`, `detect_changes`, or
`refactor_tool`. Those are useful tools, but they skip the shared orientation
step that reports freshness, risk, communities, and suggested next tools.

The exploration, debug, review, and refactor skills now start with
`get_minimal_context` before moving to specialized analysis.

### Medium: refactor skills needed stronger safety language

The refactor workflow previously focused on available tools: suggest, dead_code,
rename, apply. It now also names the evidence gates that matter before an edit:
public APIs, dynamic dispatch, generated code, test artifacts, and framework
entry points.

### Low: build skill had a stale language list risk

The build skill included a fixed supported-language list. Because dagayn's
parser registry changes, that list can become stale. The skill now points to
the README supported-language section as the authoritative source.

## Changes Made

- `AGENTS.md` now tells agents to start broad tasks with `get_minimal_context`,
  cite graph metrics, and fall back to source reads when graph output is stale,
  ambiguous, truncated, or text-insufficient.
- Generated `AGENTS.md` / `CLAUDE.md` instruction content now includes the same
  evidence standard and key analysis tools.
- Review/debug/explore/refactor skills now start with `get_minimal_context`.
- Review and refactor skills now distinguish graph prioritization from proof.
- Tests assert the generated instruction section includes `get_minimal_context`,
  analysis-judgment guidance, and truncation awareness.

## Remaining Judgment Calls

1. The skills still use compact token budgets. That is useful for default
   ergonomics, but complex reviews may legitimately need more calls.
2. The generated instructions intentionally do not list every MCP tool. They
   should teach decision flow, not become a duplicate command reference.
3. The best long-term improvement is for more MCP tools to expose their own
   metric policy metadata so skills can reference tool output rather than
   re-explaining every heuristic.

## Summary

<!-- derived-from #evaluation-criteria -->
<!-- derived-from #findings -->
<!-- derived-from #changes-made -->

The agent-facing interaction model is now stricter: use dagayn first for
structural context, judge each result by explicit quantitative or qualitative
evidence, and verify source behavior before turning graph insights into
findings or edits.
