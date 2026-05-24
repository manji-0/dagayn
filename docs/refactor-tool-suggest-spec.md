# Refactor Tool Heuristic Specification

<!-- derived-from ./refactoring-priority-report-2026-05-04.md#p2-improve-refactor-tool-precision -->

This document defines the default heuristics for `refactor_tool`.

## Goals

Suggestions should be actionable refactoring leads, not deletion commands. The default first page should prefer low-risk internal code with strong graph evidence and should demote or hide candidates that static analysis is likely to misclassify.

The specification is language-neutral. Language-specific syntax is only an adapter for discovering common concepts such as public API, generated or test-only code, external entry points, unusually large units, and low explanation density.

## Suggestion Types

### Remove

`remove` suggestions are graph-backed dead-code candidates derived from `find_dead_code`.

Default remove suggestions must:

- require exact zero evidence for runtime and structural references:
  `caller_count == 0`, `test_ref_count == 0`, `importer_count == 0`,
  `reference_count == 0`, and for classes `subclass_count == 0`;
- exclude test-only code when source context shows it lives under a language test module;
- exclude documentation section nodes from default code suggestions;
- exclude data/value container type definitions identified by parser metadata;
- preserve the graph evidence used to make the decision;
- downgrade public API candidates instead of presenting them as ordinary internal dead code.

Public API candidates include exported declarations, public class or method declarations, and methods exported through bridge frameworks. These are not safe default deletion candidates because they may be consumed across package boundaries or by downstream users outside the current graph.

### Move

`move` suggestions identify a function assigned to one community but called only by another community.

Default move suggestions must require:

- at least two incoming call sites;
- every incoming call site has a known community;
- all caller communities agree on one target community;
- the target community differs from the function's current community;
- the function is not a public API candidate.

The two-call minimum is a low floor, not strong evidence. Confidence is
therefore derived from caller count:

| Incoming call sites | Confidence | Rationale |
| --- | --- | --- |
| 2-3 | low | Enough to avoid a one-off edge, but still weak. |
| 4-7 | medium | Repeated use by the same external community. |
| 8+ | high | Strong community ownership signal. |

Move suggestions are advisory because community assignment can shift as the graph changes.

### Split

`split` suggestions identify oversized code units whose length and interaction shape indicate that extraction or decomposition may reduce maintenance risk.

Default split suggestions must require:

- a function or class above its kind-specific size threshold;
- at least one additional complexity signal for functions, such as branch-heavy source or many outgoing call edges;
- either branch-heavy source or very high absolute size for classes, where class-level call edges are often sparse;
- non-test code.

The default thresholds are calibrated around the upper tail of the repository's
function/class distribution, not around a universal style preference:

| Kind | Size gate | Secondary gate | Rationale |
| --- | --- | --- | --- |
| Function | `line_count >= 60` | `branch_count >= 12` or `outgoing_call_count >= 22` | Roughly p95 for function length and p95-ish for branch/collaborator pressure in this repo. |
| Class | `line_count >= 120` | `branch_count >= 20` | Classes tend to have sparse class-level call edges, so branch density is the main secondary signal. |
| Class | `line_count >= 250` | none | Very large containers are worth reviewing even when branch extraction is low or parser edges sit on methods. |

Split evidence includes `line_count`, `branch_count`, `outgoing_call_count`,
threshold values, `reason_codes`, and `split_pressure`. `split_pressure` is a
dimensionless score built from threshold ratios; it is for ranking and review
triage, not a proof that code must be split.

Split suggestions are advisory and should not propose a target shape automatically.

### Document

`document` suggestions identify public or complex code units with low explanation density.

Default document suggestions must require:

- a public API candidate or a large complex unit;
- enough source lines to make missing explanation meaningful;
- a low ratio of comment/doc lines in the code unit.

The default threshold is `line_count >= 60` and `comment_ratio <= 0.01`.
In the current repository this selects the lower explanation-density tail among
public or complex units. A broader `0.03` threshold was rejected because it
captured roughly half of the candidate population, which is not selective enough
to mean "low explanation density."

Documentation suggestions should ask for intent, invariants, contracts, or edge-case notes. They should not ask for comments that merely restate code.

## Rename And Apply

`refactor_tool(mode="rename")` is a preview operation, not an autonomous
recommendation. Its quantitative evidence is the returned edit list and
confidence counts:

- high confidence: definition, exact call, or exact import edges;
- medium confidence: bare-name call edges where the graph cannot prove the
  fully-qualified target;
- ambiguity evidence: `candidate_count` and `ambiguous`.

`apply_refactor_tool` applies only a previously-previewed `refactor_id`. The
safety property is exact replacement of the previewed strings, optional
`dry_run`, path validation under the repo root, and preview expiry.

## Ranking

Default ranking is evidence-first:

1. internal executable code;
2. unknown executable-like code;
3. fixtures and tests;
4. public API candidates;
5. documentation nodes.

Within those buckets, higher priority, higher confidence, and lower estimated risk rank first. Sorting by symbol is only a final tie-breaker.

## Output Contract

Every suggestion should include:

- `type`
- `description`
- `symbols`
- `rationale`
- `priority`
- `confidence`
- `category`
- `estimated_risk`
- `affected_files`
- `verification_steps`

Remove, move, split, and document suggestions should include `reason_codes` and
`evidence` describing the graph, size, call, branch, or comment-density signals.
