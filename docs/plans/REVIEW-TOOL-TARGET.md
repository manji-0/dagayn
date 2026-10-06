# What review_tool is for

## Question

`review_tool(mode="changes")` is the change-analysis entry point the docs,
skills, and MCP instructions send every agent to. Does it tell a reviewer
something the diff does not, and what should it be?

Verdict: **today it mostly restates the diff and adds noise.** Every signal
fires on every change, so none of them separates a risky change from a safe
one, and the one thing a reviewer most needs from a graph (what still points
at code the change removed or reshaped) is missing. This note defines the
target contract, the evidence behind it, and the order of work.

## Target contract

<!-- supersedes ./ANALYSIS-TOOL-STRATEGY.md#change-analysis -->

> `review_tool(mode="changes")` lists what a reviewer must check before
> merging that the diff itself does not show. Each finding names a concrete
> place to look and the graph fact behind it. A change that needs nothing
> beyond the diff gets an empty `findings` list.

Consequences of that sentence:

- **Silence is a result.** A docs-only or CI-only change returns no code
  findings, not a medium risk level.
- **Findings, not scores.** Each finding is one checkable claim: kind,
  location, evidence edge(s), and the action ("open this caller", "run this
  test"). A summary score, if one is kept, is derived from findings and never
  shown on its own.
- **Bounded output.** `minimal` fits in about 2K tokens for any diff; larger
  sets are counted and paged, never dumped.
- **Precision before recall.** A finding kind that is wrong more often than
  right on the eval set below is not shipped.

## Evidence: the current output

Measured on 2026-10-06 against this repository. Each commit was checked out
in a scratch worktree, built with `dagayn build`, and reviewed with
`base="HEAD~1"` and `detail_level="minimal"`, so every row is that commit's
own diff on its own graph.

| Commit | Change | Files | Output (chars) | `risk_level` | reason codes | changed nodes | impacted nodes |
|---|---|---|---|---|---|---|---|
| `027bbc80` | CI yaml only | 1 | 5,037 | low | 1 | 0 | 0 |
| `a155e96f` | docs only | 1 | 4,993 | low | 1 | 34 | 91 |
| `1a452379` | plan doc | 1 | 11,422 | low | 2 | 108 | 24 |
| `6b90c812` | toolchain pin, 2 loops rewritten for clippy | 5 | 18,728 | medium | 10 | 118 | 475 |
| `529f7b16` | builtin-name list fix | 2 | 15,436 | low | 6 | 94 | 461 |
| `1def1b4f` | drop PowerShell parsing | 7 | 24,637 | medium | 10 | 245 | 448 |
| `797ba750` | grammar Cargo features | 21 | 22,295 | medium | 13 | 331 | 483 |
| `cf16d792` | Ruff parser only, −4,352 lines | 24 | 26,610 | medium | 13 | 559 | 437 |
| `23ec41ef` | add the Ruff extractor | 16 | 20,306 | high | 13 | 449 | 425 |
| `a30c7a4a` | query_graph to Rust | 12 | 17,572 | high | 13 | 330 | 444 |

`detail_level="standard"` on the last three commits together (39 files)
returned 548,671 characters: `changed_edges` alone was 388,545 (1,278 raw
edges) and `affected_flows` 132,724, mostly flow `members` id arrays. Neither
field is under `_truncation`.

What the numbers mean:

- **Docs-only and CI-only changes are already quiet** (1–2 reason codes,
  `low`), though still 5–11K characters.
- **No discrimination among code changes.** Any change that touches code
  fires 6–13 reason codes. The toolchain pin rewrote two loops for clippy (8
  lines) and still got `wide_blast_radius`, `critical_flow_affected`, and
  `architecture_violation_in_changed_scope`. `impacted_node_count` sits at
  425–483 whatever the change size, because flows are 512-member reachable
  sets, so any changed node is "in a critical flow".
- **Priority follows names, not risk.** `compute_change_risk_score` adds 0.20
  when an identifier token *starts with* a security keyword, and it checks
  the qualified name, which contains the file path. `python_call_signature`
  (3 lines) tops the review of `23ec41ef` because `signature` starts with `sign`;
  every function in `query.rs` or `session_cmd.rs` is "security sensitive".
  The rest of the score saturates: flow term ≤0.25, untested baseline 0.30.
- **Tests and build scripts are ranked as review targets.** Test functions
  top `review_priorities` for `60db881c` and `529f7b16`; `build.rs::main`, a
  `#[cfg(test)] mod tests`, and functions under `examples/` are reported as
  untested production code.
- **Deletions are invisible.** `change_entity_summary` has `added`,
  `existing`, and `unknown`; no `removed`. The Ruff commit deleted the
  `python_ts` module; the review lists its files as "unmapped" and says
  nothing about removed symbols or what still references them.
- **Doc candidates point at the change's own doc.** Reviewing the last three
  commits together, the only documentation candidate was
  `RUFF-PYTHON-PARSER.md`, written in that same range.
- **Supported files reported as unmapped.** `Cargo.toml` and CI YAML are
  "unmapped changed files", although TOML is an indexed language and
  manifest bridges exist.
- **The design already admits part of this.** `score_semantics` calls
  `risk_score` a "legacy alias".

## Finding kinds

Proposed set, each with the graph fact it rests on. Pending approval; the
eval decides which ship.

| Kind | Fires when | Evidence | Action |
|---|---|---|---|
| `dangling_reference` | a symbol present at `base` is gone or renamed and a node outside the diff still calls, imports, or links to it | base-side parse of the changed file + inbound edges | open each referencing site |
| `unchanged_caller` | a changed function's signature (params, return type, visibility) differs from `base` and a caller outside the diff was not edited | signature diff + `CALLS` edges | check the caller still fits |
| `tests_to_run` | changed production code has direct `TESTED_BY` edges | direct test edges only | a runnable command per language (`cargo test -p … name`, `pytest path::name`) |
| `untested_change` | changed production code (not tests, examples, build scripts, generated code) has no direct or transitive test | test edges + path classification | write or point to a test |
| `contract_doc_not_updated` | an authored contract doc (`implemented_by` / `implements_contract`) links to changed code and the doc is not in the diff | authored doc links | read the section, update or confirm |
| `bridge_touched` | the change edits one side of a HIGH/EXACT cross-artifact bridge (manifest, Terraform, FFI, build config) and not the other | `CROSS_ARTIFACT` edges | check the other side |

Moved out of `changes` into drill-down modes or dropped: hotspot proximity,
SDP/SAP/ADP density and "stable component contract gap" (architecture
questions, already in `architecture_analysis_tool`), flow criticality
rankings (stay in `affected_flows`), `answerability` counts (stay in
`get_minimal_context_tool`), raw `changed_edges`, and the duplicated
`change_file_sources` lists.

## Evaluation

A harness beside `eval/run_search_eval.py`, run in CI on fixed fixtures:

- **Negative cases** from real history: docs-only, CI-only, comment-only,
  formatting-only, and toolchain-pin commits expect zero code findings.
- **Seeded positives** in a fixture repo per major language (Python, Rust,
  TypeScript, Terraform): delete a function that still has a caller; change
  a signature with an unedited caller; edit a function covered by a contract
  doc; edit one side of a manifest bridge. Each must produce exactly its
  finding.
- **Reported per run:** precision and recall per kind, `minimal` output size
  (p50/max), and the share of commits with zero findings.

## Order of work

1. **Done:** explicit `changed_files` scopes the review. The base diff used
   to add every other file's changed nodes, so a CI-only file list reported
   Rust functions from the previous commit (`crates/dagayn-tools/src/review.rs`,
   regression case in `review_changes_scores_the_diff_against_base`).
2. Bound the output: put `changed_edges` and flow `members` under
   `_truncation`, drop the duplicated lists from `minimal`.
3. Eval harness with the negative cases and the seeded positives.
4. Base-side symbols: parse the `base` version of each changed file in Rust
   (the parsers already run in-process), giving `removed` and signature
   changes. This unlocks `dangling_reference` and `unchanged_caller`.
5. `findings` list with the six kinds; `tests_to_run` commands per language.
6. Retire the score-first fields: `risk_level`, `review_priorities`, and the
   always-on reason codes move behind `detail_level="verbose"` for one
   release, then go.
7. Update the touch points listed below and regenerate the MCP parity
   snapshots deliberately; the contract change breaks them by design.

## Touch points

- Code: `crates/dagayn-tools/src/review.rs`, `review_summary.rs`,
  `changes.rs`; `crates/dagayn-graph/src/impact_support.rs`
  (`compute_change_risk_score`).
- Docs: `docs/COMMANDS.md`, `docs/LLM-OPTIMIZED-REFERENCE.md`,
  `docs/USAGE.md`, `docs/plans/ANALYSIS-TOOL-STRATEGY.md`, the MCP tool
  description and server instructions, and the review / implement-feature
  skills.
- Tests: `crates/dagayn-tools/tests/tools.rs` review cases, Python
  `tests/test_changes.py`, `tests/test_review_flow_dispatchers.py`, and
  `tests/fixtures/parity/__mcp_snapshots__`.
