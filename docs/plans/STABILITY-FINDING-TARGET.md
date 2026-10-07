# Stability as a finding

<!-- constrained-by ./ARCHITECTURE-TOOL-TARGET.md#target-contract -->
<!-- constrained-by ./REVIEW-TOOL-TARGET.md#target-contract -->

## Question

The SDP and SAP metrics (instability, abstractness, the distance from the
main sequence) were computed per declared unit but used only behind
`detail_level="verbose"`, in their own metric modes, and in the wiki. No
finding of the default workflow rested on them. Should they, and in what
form?

The earlier answer was no: "an instability or abstractness score crossing
a threshold is not a defect without a stated design intent"
([architecture](./ARCHITECTURE-TOOL-TARGET.md#finding-kinds)). This note
keeps that objection and answers it with a narrower claim.

Status: shipped ([decisions](#decisions-2026-10-08)).

## Target contract

<!-- supersedes ./ARCHITECTURE-TOOL-TARGET.md#finding-kinds -->

> `unstable_dependency` names a dependency of a declared unit on a less
> stable one: the place that makes it, both units' afferent and efferent
> counts and instability, and, where SAP applies to the unit depended on,
> its abstractness and distance. The architecture overview lists every one
> in the repository; a review lists the ones the change introduced.

Consequences:

- **A dependency, not a score.** The finding names the import that makes
  the dependency and an action (point it the other way: move what is
  needed into the stable unit, or have the unstable one implement an
  interface the stable one owns). A unit far from the main sequence is
  not a finding on its own.
- **Introduced by the change, in a review.** Every edge behind the
  dependency sits on a line the diff touches, and the base version of those
  files did not name the unit depended on. A change that edits a file which
  already had the dependency, or rewrites the import line, does not fire;
  the overview reports that dependency instead.
- **One computation.** The units, the edges (`strict_static`: imports,
  inheritance, `DEPENDS_ON`), and the 0.1 instability gap are those of
  `architecture_analysis_tool(mode="sdp_violations")`, so the finding and
  the mode agree. Test and fixture scopes are neither side.
- **SAP as evidence.** SAP does not apply to most Rust crates (no type
  counts as abstract), and on this repository the one applicable unit in
  its "zone of pain" is the Python package, which is no defect. The
  distance is evidence on the unit depended on, not a finding.

## Evidence

On this repository at `5f7ef086`: 17 declared units, 0 SDP violations
(the 8 the directory-based metric reported were directory nesting), 6 of 28
units with an applicable SAP position. Neither surface has a finding here,
which is the expected result for a workspace whose crate layering the
manifests enforce.

## Decisions (2026-10-08)

- SDP becomes the finding kind `unstable_dependency`, in both
  `review_tool(mode="changes")` (introduced by the change) and
  `architecture_analysis_tool(mode="overview")` (every one).
- SAP stays evidence on that finding.

## Evaluation

Cases in the existing harnesses, gated by the same floors (0.8):

- `tests/fixtures/review_eval`: `unstable_dependency_rust` (a Cargo
  workspace's `kernel` starts using `plugins`) and
  `unstable_dependency_python` (the same between import packages);
  negatives `neg_unstable_dependency_toward_stable` (the unstable `app`
  starts using `plugins`), `neg_unstable_dependency_existing` (the
  dependency predates the change, which edits another line), and
  `neg_unstable_dependency_import_rewritten` (the change rewrites the
  import line of a dependency that predates it).
- `tests/fixtures/architecture_eval`: `pos_unstable_dependency_rust`,
  `pos_unstable_dependency_python`, and `neg_layered_units`.

First run: precision and recall 1.00 in both harnesses, every negative
silent, and no other kind changed. A review at `minimal` on this
repository took 1.72 s before and after.

## Touch points

- Code: `crates/dagayn-tools/src/stability.rs` (the finding);
  `architecture.rs` (`Snapshot::dependency_edges`, the edges behind each
  scope dependency); `findings.rs` (the review kind); `arch_findings.rs`
  and `arch_tool.rs` (the overview kind).
- Eval: `eval/run_review_eval.py`, `eval/run_architecture_eval.py`, their
  threshold files, the cases above, and the `rust_workspace` and
  `py_layers` bases.
- Docs: `docs/COMMANDS.md`, `docs/LLM-OPTIMIZED-REFERENCE.md`, the
  architecture-analysis, review-changes, review-delta, and review-pr
  skills, and the tool descriptions.
