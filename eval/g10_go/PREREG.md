# Pre-registration: G10, the Go agent experiment on new questions and an untuned repository

<!-- derived-from ../g9_go/PREREG.md -->
<!-- constrained-by ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-05-after-g9 -->
<!-- constrained-by ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-04-after-g8 -->

This file is in progress. The registering commit completes it.

## How the questions are picked

<!-- derived-from ../g9_go/PREREG.md#how-the-questions-are-picked -->

### Question length

G9's rule, unchanged (`../g9_go/keys/g9pools.py`, imported by
`keys/g10pools.py`):

- **LONG**: kind 1 (which tests execute a function), or a kind 2 to 4
  question whose target's `name_calls` is at least 200.
- **SHORT**: a kind 2 to 4 question whose target's `name_calls` is below
  200.

`name_calls` is the number of lines in the repository's `.go` files (the
files `go test ./...` sees) where the target's name is followed by `(`. The
rule's basis is in [G9's pre-registration](../g9_go/PREREG.md#question-length).

### Repositories

| Repository | Tag (commit) | Why |
|---|---|---|
| testify | v1.12.1 (959dbda) | G7's to G9's |
| compress | v1.20.1 (5d880f2) | G7's to G9's |
| kong | v1.15.0 (14c97a2) | G9's |
| chi | v5.5.0 (3a33ba5) | added: no fix since G7 was tuned on it |

chi is `github.com/go-chi/chi/v5`, cloned with its history (842 commits;
v5.5.0 is of 2026-10-14). It has two packages, `chi` and `middleware`, with
189 production functions and 136 top-level tests. It builds and tests with Go
1.25.3, and both coverage runs ended with 0 failures. Its `go.mod` says
`go 1.24`. Its two other `go.mod` files are under `_examples`, which the Go
tools, scip-go, and the prompts' scope all leave out.

How it was chosen. #292 asks for a Go repository with git history that no
fix since G7 was tuned on. The candidates were widely used Go modules of a
size near kong's. A candidate had to pass four checks, in this order:

1. Never used here. A search of the repository (code, tests, fixtures,
   `eval/`, `docs/plans/`, `CHANGELOG.md`) and of the session's scratch
   clones found no use of chi, logrus, urfave/cli, or zap. cobra was
   dropped: it appears in a fixture of `scip_overlay.rs`.
2. One module in the prompts' scope. scip-go and the coverage runs see the
   root module only, while the prompts count every `.go` file of the
   repository outside `_x`, `.x`, and `testdata`. A nested module there
   would put callers in the prompts' scope that the keys cannot see.
   **zap** v1.28.0 failed: `benchmarks`, `exp`, `assets`, and
   `zapgrpc/internal/test` are modules of their own.
3. A `go` directive the key toolchain (Go 1.25.3) builds. **urfave/cli**
   v3.14.0 failed: `go 1.26`.
4. Each stratum non-empty, so the new repository can carry LONG questions
   of both strata. **logrus** v1.10.2 passed checks 1 to 3, but its name
   stratum is empty (36 / 0 / 22 candidates in reach / name / short).
   **chi** passed with 41 / 1 / 25.

Only pool sizes were looked at for check 4, as G9 did when it gave kong no
name question. gorilla/mux was not checked.

### Pools

`keys/g10pools.py` takes G9's pools (`../g9_go/keys/g9pools.py`, imported
unchanged: G8's four kinds, less G7's and G8's targets, with `name_calls` and
the length), and also removes G9's twelve targets. It runs on the four tags'
`git archive`s. The testify, compress, and kong inputs are G9's files; the
script reproduces G9's `pools.json` from them exactly, and G9's
`keys/g9keys.py` reproduces G9's twelve keys from them. chi's inputs come from
G8's `keys/build_inputs.sh`, unchanged.

| Pool (LONG / SHORT) | testify | compress | kong | chi |
|---|---|---|---|---|
| Kind 1 | 56 / 0 | 218 / 0 | 39 / 0 | 41 / 0 |
| Kind 2 | 3 / 6 | 6 / 52 | 1 / 2 | 0 / 0 |
| Kind 3 | 0 / 2 | 2 / 6 | 0 / 2 | 1 / 21 |
| Kind 4 | 0 / 17 | 7 / 60 | 0 / 11 | 0 / 4 |

### Draw

`keys/g10draw.py` draws from `pools.json` with G9's draw
(`../g9_go/keys/g9draw.py`, imported unchanged). The seed is the first 8 hex
digits of the SHA-256 of `pools.json`, a new seed because the pools are new.
The strata are G9's: **reach** (LONG, kind 1), **name** (LONG, kinds 2 to 4
with `name_calls` at least 200), and **short** (SHORT, kinds 2 to 4 below
200). Only the allocation is G10's:

| Repository | reach | name | short | Questions |
|---|---|---|---|---|
| testify | 1 | 1 | 0 | 2 |
| compress | 1 | 1 | 0 | 2 |
| kong | 1 | 0 | 1 | 2 |
| chi | 2 | 1 | 3 | 6 |
| All | 5 | 3 | 4 | 12 |

That is 8 LONG questions and 4 SHORT, as in G9, and 240 runs.

Why chi carries half. The fixes since G7 were tuned on testify, compress,
and, since G9, kong, and G9's own questions have been used for tuning too.
chi is the only part of G10 the fixes have not seen, so it is the only
evidence of how dagayn does on Go it was not tuned on. Weighting it as one
repository in four would let three tuned repositories decide the result.
The tuned repositories stay in, two questions each, as a check that the
fixes did not break what they were tuned for.

- chi's name stratum has one candidate, so its name question is that
  candidate. Its other LONG questions are test reach.
- 3 of the 4 SHORT questions are chi's. G9's SHORT loss came from the one
  untuned repository (kong: K3 1.66x, K4 1.37x), so the guard should
  mostly be tested where nothing was tuned.
- testify and compress have no SHORT question and kong no name question.
  kong's name stratum has one candidate.
- `analyze.py` also reports chi's LONG and SHORT questions on their own, as
  a secondary result.

A repository draws no target twice and at most one kind 4 target. For a kind
4 target the draw takes 2 of its callers outside test files, which the
change commit edits with the target, as in G8.
