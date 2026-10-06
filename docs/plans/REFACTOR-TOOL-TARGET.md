# What refactor_tool is for

## Question

<!-- constrained-by ../COMMANDS.md#mcp-tools -->

`refactor_tool(mode="suggest")` is the refactor entry point the docs, the
`refactor-safely` skill, and the MCP instructions send agents to. Does it
name a refactor worth doing that an agent could not find by reading the
code, and what should it be?

Verdict: **today it is a list of threshold hits, most of them not
refactors.** One kind fires on 406 functions, the "high confidence" moves
are closures inside tests, and every remove candidate is a test fixture.
`dead_code` and `rename` in the same tool already answer a concrete
question; `suggest` does not. This note defines the target contract, the
evidence behind it, and the order of work.

Status: accepted ([decisions](#decisions-2026-10-07)); step 1 of the
[order of work](#order-of-work) is done.

## Target contract

<!-- supersedes ./ANALYSIS-TOOL-STRATEGY.md#refactor-opportunity-analysis -->
<!-- supersedes ../refactor-tool-suggest-spec.md#goals -->

> `refactor_tool(mode="suggest")` lists refactors worth doing in this
> repository now. Each finding names the symbol, the place it should go or
> the shape it should take, and the graph or history fact behind it. A
> repository with nothing worth doing gets an empty `findings` list.

Consequences of that sentence:

- **Silence is a result.** A kind fires only where its fact holds; no
  kind fires on hundreds of symbols by construction.
- **Findings, not rankings.** A size or comment-ratio threshold is a
  metric, not a finding. Metrics may feed a finding; they are not one.
- **Bounded output.** `minimal` fits in about 3K tokens: each kind keeps
  10 findings and counts the rest in `findings_omitted`. Per-type
  templates (`execution_plan`, `work_pack`) appear once per kind, not once
  per finding.
- **Works on the bootstrap graph.** No kind reads communities or flows, so
  the answer is the same after `ensure_graph_tool` (`postprocess="minimal"`)
  as after a full build.
- **Tests are inputs, not targets.** Test code, fixtures, examples, and
  generated code are never the subject of a finding; one path
  classification is shared with `review_tool`'s `untested_change`.
- **Precision before recall.** A finding kind that is wrong more often than
  right on the eval set is not shipped.

## Evidence: the current output

Measured on 2026-10-07 against this repository at `60864bcb` (875 files,
16,776 nodes, full postprocess), with `dagayn tool refactor_tool`. Sizes
are the response as compact JSON, as an MCP client receives it.

| Call | Output (chars) |
|---|---|
| `suggest`, `limit=10` | 51,513 |
| `suggest`, `limit=50` (the default) | 221,027 |
| `suggest`, every suggestion (655) | 2,511,559 |
| `dead_code` | 1,993 |
| `rename` `node_text` → `node_text2` | 91,264 |

| Type | Count | Confidence | What the hits are |
|---|---|---|---|
| `split` | 406 | 278 low, 128 medium | every function of 60+ lines with 12 branches, 22 calls, or a concern score of 0.65 |
| `document` | 159 | all medium | public or split-eligible symbols of 60+ lines with a comment ratio of at most 0.01 |
| `move` | 60 | 4 high, 3 medium, 53 low | see below |
| `remove` | 30 | 24 medium, 6 low | all 30 are samples under `tests/fixtures/` |

What the numbers mean:

- **No kind discriminates.** `split` fires on 406 of the repository's
  functions and classes, `document` on 159. Their thresholds
  (`split_metrics`, `crates/dagayn-tools/src/suggestions.rs`) mark a
  function as long, which the agent can see by opening it; they do not
  say that splitting it pays off.
- **Move points at communities, and almost never at a refactor.** The rule
  fires when every caller sits in one other community. All 60 hits,
  classified against the call edges:

  | Class | Count | Example |
  |---|---|---|
  | every caller is in the same file | 29 | `crates/dagayn-parser/src/kotlin.rs::kotlin_emit_function` |
  | nested function or closure | 15 | `crates/dagayn-tools/src/analysis.rs::find_knowledge_gaps.returned` |
  | test code | 10 | `crates/dagayn-parser/src/core_tests/javascript_calls.rs::resolves_typescript_member_calls.calls` |
  | has a caller in another file | 6 | `dagayn/tools/sync_status.py::sidecar_embed_payload` |

  The four `high` hits are all test helpers or closures inside tests.
  Of the six with a caller elsewhere, two are called only from tests, two
  from a benchmark script under `tools/`, and one from its own child
  module. Move suggestions skip test files nowhere: the test filter
  (`is_test_artifact`) covers `split`, `document`, and `remove`, not `move`.
- **Remove is dead_code with the evidence taken out.** `suggest` calls the
  verified `dead_code` pipeline, drops its `suppressed` counts and
  `verification`, and keeps the fixtures that `dead_code` itself sets
  aside as `test_fixture` (`8adec381`). On this repository `dead_code`
  reports 0 symbols; `suggest` reports the 30 fixtures as removal
  candidates. A fixture is the input of a test; deleting one breaks it.
- **The weight is in templates.** Each suggestion carries a
  `work_pack`, `execution_plan`, and `verification_steps` built from a
  static template per type, and function splits add a
  `concern_separation` profile: 3–4.6K characters per suggestion.
  `guidance` repeats one action string for the top three, and `_hints`
  copies it three times without deduplication. There is no
  `detail_level`: Python accepts one and ignores it.
- **The other modes already answer.** `dead_code` returns a verified list
  with suppression reasons and is silent here (0 found, 499 left out with
  reasons). `rename` answers "what does this rename touch" correctly, but
  returns all 585 edits across 36 files in one flat list.

## Finding kinds

<!-- derived-from #evidence-the-current-output -->

Proposed set; the eval decides which ship.

| Kind | Fires when | Evidence | Action |
|---|---|---|---|
| `unused_symbol` | the `dead_code` pipeline reports a symbol as unreferenced | the `dead_code` record, with its `suppressed` counts and `verification` status | delete it, or point to the dynamic use the graph missed |
| `complex_hotspot` | a production function passes today's split thresholds and at least the repository's p90 number of commits in the last 90 days changed lines inside it | split metrics plus the commits whose hunks touch the function | split it; the split pays off where the code keeps changing |
| `undocumented_surface` | a symbol in a unit's `surface` (the symbols other units use most) has no doc comment | the `surface` entry and its inbound edges from other units | document it |

Fates of today's types:

- `remove` becomes `unused_symbol`. Fixtures leave `suggest` the same way
  they left `dead_code`, which reverses the ranking choice in `8adec381`
  and `test_remove_suggestions_prioritize_executable_code`.
- `move` goes. No graph fact measured here separates a misplaced
  function from a well-placed one (see below).
- `split` stops being a finding on its own. Size feeds
  `complex_hotspot`, so a long function that nobody touches stays silent.
- `document` narrows to a unit's `surface`: at most three symbols per
  unit, the ones other code depends on.

`complex_hotspot` needs churn per function, not per file. Counted per
file, 98 of the 406 split candidates sit in a file at or above the p90 of
11 commits in the last 90 days (12 in `rust_lang/mod.rs` alone): this
repository was largely rewritten in that window, so file churn marks most
of it. The eval decides whether hunk-level churn is quiet enough.

Considered and not proposed:

- **Moves between declared units** (no caller in the function's own unit,
  every non-test caller in one other unit): measured on this repository it
  fires on 139 top-level functions, led by `dagayn-py` → `dagayn/` (55,
  the PyO3 bindings) and `dagayn-graph` → `dagayn-tools` (32). That is
  the declared layering, which `ARCHITECTURE-TOOL-TARGET.md` leaves to a
  rule file the repository authors.

- **File-level moves** (every caller in one other file of the same unit):
  helper modules with one consumer are a common, deliberate layout; the
  fact does not separate a misplaced function from a well-placed one.
- **Concern-separation scores as findings**: the weighted profile
  (0.45 responsibility, 0.25 side effects, 0.3 context) has no eval
  behind it. It can stay in a split finding's evidence.

## Decisions (2026-10-07)

- The finding kinds above go ahead; the eval still gates each one.
- Fixtures leave `suggest` as they left `dead_code`, which reverses the
  ranking choice in `8adec381`: all 30 remove candidates on this
  repository were fixture samples, and deleting one breaks its test.
- `move` is dropped rather than reworked; no measured fact separates a
  misplaced function from a well-placed one.

## Output shape

<!-- derived-from #target-contract -->

- `findings`: kind, symbol (`path::name`, line), evidence, action.
- `findings_omitted`: per-kind counts beyond 10.
- `plans`: one `execution_plan` and `verification_steps` per kind that
  fired, instead of one per finding.
- `detail_level`: `minimal` is the findings; `standard` adds each finding's
  edge list and metrics; `verbose` adds the deprecated `suggestions`,
  `work_packs`, and `counts_by_type` for one release.
- `rename` keeps its contract and adds a per-file summary: `files` with an
  edit count each, plus the first 20 edits. The full list stays under
  `refactor_id` for `apply_refactor_tool`, and `detail_level="full"`
  returns it inline.

## Evaluation

A harness beside `eval/run_architecture_eval.py`, run in CI on fixed
fixtures:

- **Negative cases**, each expecting zero findings: a repository whose
  only unreferenced symbols are fixtures; a long but unchanged function; a
  helper called only by tests; a closure called only by its parent; a
  function whose callers all sit in its own file.
- **Seeded positives**: an unreferenced private function; a long function changed in many
  commits of a scripted history; a surface symbol without a doc comment.
- **Reported per run:** precision and recall per kind, and `minimal`
  output size (p50/max). Each kind gates at 0.8, like the review and
  architecture evals.
- **This repository**, expected after the change: 0 `unused_symbol`
  (matching `dead_code`), and every `complex_hotspot` checked by hand.

## Order of work

1. **Done:** the CLI default. `dagayn tool refactor_tool` with no
   arguments ran `mode="rename"` (`refactor_func`,
   `dagayn/tools/refactor_tools.py`) and failed with "Input should be a
   valid string"; it now defaults to `suggest`, like the MCP wrapper and
   Rust.
2. Bound the output: per-kind templates once, `_hints` deduplicated,
   `detail_level` honoured, `rename` summarized by file.
3. Eval harness with the negative and positive cases above.
4. `unused_symbol` from the `dead_code` record, fixtures left out; drop
   the community-based move.
5. `complex_hotspot` and `undocumented_surface`; `split` and `document`
   behind `verbose` for one release, listed in `deprecated_fields`.
6. Docs, skills, and the MCP description describe `findings`.

## Touch points

- Code: `crates/dagayn-tools/src/suggestions.rs`, `refactor.rs`,
  `dead_code.rs`, `units.rs`, `findings.rs` (shared test-path
  classification); `dagayn/tools/refactor_tools.py`.
- Docs: `docs/refactor-tool-suggest-spec.md`, `docs/COMMANDS.md`,
  `docs/LLM-OPTIMIZED-REFERENCE.md`, the MCP tool description, and the
  `refactor-safely` skill.
- Tests: the suggest cases in `tests/test_refactor.py`,
  `crates/dagayn-tools/tests/tools.rs`, and the MCP parity snapshots.
