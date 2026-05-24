# Dagayn usability scorecard

<!-- constrained-by ../COMMANDS.md#mcp-tools -->

This scorecard tracks the concrete work needed to move dagayn's agent
experience from useful graph output to workflow guidance that can carry code
exploration, code review, feature work, and refactoring to completion.

## Success criteria

<!-- derived-from #dagayn-usability-scorecard -->

| Case | 10/10 criterion | Implemented gate |
| --- | --- | --- |
| Code exploration | The first response identifies the workflow and the next graph action. | `get_minimal_context` returns `workflow`, `recommended_action`, `why`, and `confidence`. |
| Code review | The first review result separates facts, heuristics, and uncertain leads. | `detect_changes` returns `signal_quality` and ranked actionable test gaps. |
| Feature addition | A natural-language task routes to extension-point discovery before editing. | English and Japanese feature tasks route to semantic search, graph query, and change review. |
| Refactoring | Suggestions include an executable strategy, not only a candidate list. | `refactor_tool(mode="suggest")` returns `execution_plan` for every suggestion. |
| Guidance quality | Ranked guidance can be regression-tested. | `guidance_precision` evaluates precision@k for tests, docs, and refactor candidates. |

## Phase 1: coverage evidence

<!-- derived-from #success-criteria -->

Implemented a shared coverage inference layer that combines graph facts and
medium-confidence heuristics:

- Direct `TESTED_BY` edges remain high-confidence evidence.
- Test-like names such as `TestGetMinimalContext` can cover
  `get_minimal_context` when they clearly reference the target.
- Test source spans that mention the target symbol are medium-confidence leads.
- Markdown headings and test artifacts are separated from production test gaps.

Verification gates:

- `tests/test_tools.py::TestTools::test_query_graph_tests_for_uses_heuristic_test_names`
- `tests/test_changes.py::TestChanges::test_analyze_changes_uses_heuristic_test_coverage`

## Phase 2: review signal quality

<!-- derived-from #phase-1-coverage-evidence -->

`detect_changes` now exposes review evidence in three groups:

- `graph_facts`: direct graph-derived reasons such as blast radius and hotspots.
- `heuristics`: useful but non-final leads such as test gaps and docs candidates.
- `uncertain`: caveats that explain where the reviewer must still verify.

It also returns `test_gap_ranking` so production gaps are not mixed with
documentation or test-artifact noise.

Stable-component contracts now apply the Clean Architecture stability definition
from SDP/SAP metrics: components with low instability or high afferent coupling
are expected to have higher direct test density and explicit documentation
links. `detect_changes` exposes those checks in `stability_contracts`, while
recommended tests and documentation candidates include scores, evidence levels,
and stability context.

Verification gate:

- `tests/test_changes.py::TestChanges::test_detect_changes_tool_with_changes`
- `tests/test_changes.py::TestChanges::test_classify_test_gap_buckets_docs_and_tests`
- `tests/test_changes.py::TestChanges::test_detect_changes_scores_stable_component_tests_and_docs`

## Phase 3: workflow entry

<!-- derived-from #success-criteria -->

`get_minimal_context` keeps the compact `next_tool_suggestions` contract and
adds a structured workflow explanation:

- `workflow`
- `recommended_action`
- `why`
- `confidence`
- `graph_health`

English and Japanese tasks are covered for review, debugging, exploration,
feature addition, and refactoring. Refactor intent takes precedence over the
broad "add" keyword so "add a helper during refactoring" stays in the refactor
workflow.

Verification gates:

- `tests/test_tools.py::TestGetMinimalContext::test_task_routing_japanese_workflows`
- `tests/test_tools.py::TestGetMinimalContext::test_task_routing_returns_structured_workflow_guidance`

## Phase 4: refactor execution plans

<!-- derived-from #success-criteria -->

Each refactor suggestion now includes `execution_plan` with:

- why the work is worth doing now
- minimum safe steps
- safety checks
- required tests
- rollback guidance
- defer conditions

The plan is type-specific for split, move, remove, and document suggestions.
Each suggestion also includes `work_pack` with the owner scope, first commit,
verification commands, and success criteria.

Verification gate:

- `tests/test_refactor.py::TestSuggestRefactorings::test_suggestion_structure`

## Phase 5: end-to-end gate

<!-- derived-from #phase-1-coverage-evidence -->
<!-- derived-from #phase-2-review-signal-quality -->
<!-- derived-from #phase-3-workflow-entry -->
<!-- derived-from #phase-4-refactor-execution-plans -->

`guidance_precision` adds a precision@k gate for the ranked outputs that agents
act on most directly: recommended tests, documentation update candidates, and
refactor suggestions.

The usability work is complete when the focused test suite passes and
`detect_changes` shows the changed implementation files with low or explained
risk.

Required commands:

```bash
uv run pytest tests/test_tools.py::TestTools::test_query_graph_tests_for_uses_heuristic_test_names \
  tests/test_tools.py::TestGetMinimalContext \
  tests/test_changes.py::TestChanges::test_analyze_changes_uses_heuristic_test_coverage \
  tests/test_changes.py::TestChanges::test_detect_changes_tool_with_changes \
  tests/test_changes.py::TestChanges::test_detect_changes_scores_stable_component_tests_and_docs \
  tests/test_changes.py::TestChanges::test_classify_test_gap_buckets_docs_and_tests \
  tests/test_eval.py::test_precision_at_k \
  tests/test_refactor.py::TestSuggestRefactorings::test_suggestion_structure
uv run ruff check dagayn/coverage.py dagayn/changes.py dagayn/tools/query.py dagayn/tools/review.py dagayn/tools/context.py dagayn/refactor/suggestions.py dagayn/eval/scorer.py dagayn/eval/benchmarks/guidance_precision.py tests/test_tools.py tests/test_changes.py tests/test_refactor.py tests/test_eval.py
dagayn build
dagayn detect-changes --base HEAD
```
