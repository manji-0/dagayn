# Pre-registration: G9, the Go agent experiment under the revised bar

<!-- derived-from ../g8_go/PREREG.md -->
<!-- constrained-by ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-04-after-g8 -->

This first version holds only the rule that picks the questions. It is
committed before the draw. The questions, keys, arms, procedure, and the
test follow in later commits, before any agent run.

## How the questions are picked

<!-- derived-from ../g8_go/RESULTS.md#where-dagayn-won-and-lost -->

### Question length

The owner's revised bar
([decisions](../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-04-after-g8))
tests 0.85x only on LONG questions, those of the kinds where the baseline
needs many steps. The kinds and their lengths are fixed here, before the
draw, from the baseline (no dagayn) runs of G7 and G8 alone. No G9 run
exists.

G8's four kinds do not separate by length on their own. Test reach (kind 1)
is long in every run. Kinds 2 to 4 hold both long and short questions:

| Kind | Baseline median turns, every registered run | Median |
|---|---|---|
| 1, which tests execute a function | 8.5, 8.5, 13, 17, 19.5, 22.5 | 15 |
| 2, callers on a receiver | 5, 5.5, 7, 9, 11.5, 13 | 8 |
| 3, callers through an interface or a function value | 5, 5.5, 5.5, 7.5, 8, 9 | 6.5 |
| 4, what a change left unedited | 4.5, 5.5, 6, 6, 11.5, 13 | 6 |

In kinds 2 to 4, what separates the long questions from the short ones is
how often the target's name is called in the repository. The baseline
answers with a grep for the name, and reads each hit. `name_calls` is the
number of lines in the repository's `.go` files (the files `go test ./...`
sees) where the name is followed by `(`, the lines `grep -E '\bName\('`
finds. `keys/length_basis.py` prints, for all sixteen G7 and G8 questions:

| Q | Kind | Target | Baseline median turns | `name_calls` | Length |
|---|---|---|---|---|---|
| T5 | 1 | `mock/mock.go::diffArguments` | 22.5 (G8) | 2 | LONG |
| C5 | 1 | `zip/reader.go::File.OpenRaw` | 13 (G8) | 3 | LONG |
| T1 | 1 | `assert/assertions.go::CollectT.Errorf` | 8.5 (G7), 8.5 (G8 rerun) | 124 | LONG |
| C1 | 1 | `flate/stateless.go::statelessEnc` | 17 (G7), 19.5 (G8 rerun) | 3 | LONG |
| T2 | 2 | `assert/assertion_forward.go::Assertions.Equal` | 13 (G7), 11.5 (G8 rerun) | 373 | LONG |
| C2 | 2 | `zstd/internal/xxhash/xxhash.go::Digest.Reset` | 7 (G7), 9 (G8 rerun) | 273 | LONG |
| C7 | 3 | `gzhttp/writer/gzkp/gzkp.go::NewWriter` | 9 (G8) | 270 | LONG |
| C4 | 4 | `flate/deflate.go::Writer.Reset` | 11.5 (G7), 13 (G8 rerun) | 273 | LONG |
| T6 | 2 | `assert/assertion_forward.go::Assertions.JSONEq` | 5 (G8) | 52 | SHORT |
| C6 | 2 | `gzip/gzip.go::Writer.init` | 5.5 (G8) | 127 | SHORT |
| T3 | 3 | `suite/suite.go::Suite.T` | 8 (G7), 7.5 (G8 rerun) | 23 | SHORT |
| T7 | 3 | `suite/suite.go::Suite.SetT` | 5 (G8) | 9 | SHORT |
| C3 | 3 | `flate/deflate.go::compressor.storeFast` | 5.5 (G7), 5.5 (G8 rerun) | 1 | SHORT |
| T4 | 4 | `assert/assertions.go::ObjectsAreEqual` | 6 (G7), 6 (G8 rerun) | 16 | SHORT |
| T8 | 4 | `assert/assertions.go::YAMLEq` | 5.5 (G8) | 54 | SHORT |
| C8 | 4 | `zstd/blockenc.go::blockHeader.setSize` | 4.5 (G8) | 12 | SHORT |

The rule:

- **LONG**: kind 1, or a kind 2 to 4 question whose target's `name_calls`
  is at least 200.
- **SHORT**: a kind 2 to 4 question whose target's `name_calls` is below
  200.

On the 24 baseline medians above, LONG has 13 (median 11.5 turns, 7 to
22.5) and SHORT has 11 (median 5.5, 4.5 to 8). The threshold 200 lies in
the gap between the largest short question (127, C6) and the smallest long
one (270, C7). The basis is thin: twelve kind 2 to 4 questions, one gap,
and the rule was found by looking at them. It was chosen from the baseline
alone. dagayn's results on these questions were known (G8's results), and
the rule does not use them.

### Pools

`keys/g9pools.py` takes G8's four kinds unchanged (`../g8_go/keys/pools.py`
and its rule in [G8's pre-registration](../g8_go/PREREG.md#how-the-questions-were-picked)),
removes G7's and G8's sixteen targets, and adds `name_calls` and the length
to every candidate. It runs on three repositories, each the tag's
`git archive`:

| Repository | Tag (commit) | Why |
|---|---|---|
| testify | v1.12.1 (959dbda) | G7's and G8's |
| compress | v1.20.1 (5d880f2) | G7's and G8's |
| kong | v1.15.0 (14c97a2) | added: third-party, with history, and none of the fixes since G8 was tuned on it |

kong is `github.com/alecthomas/kong`, cloned with its history. It is one
package of 347 production functions that builds and tests with Go 1.25.3
(269 top-level tests, 0 failures in both coverage runs). The key inputs
come from G8's `keys/build_inputs.sh`, unchanged. G8 used testify and
compress, and the fixes since G8 were tuned on their questions, so a
third repository is the only part of G9 those fixes have not seen.

| Pool (LONG / SHORT) | testify | compress | kong |
|---|---|---|---|
| Kind 1 | 57 / 0 | 220 / 0 | 41 / 0 |
| Kind 2 | 5 / 7 | 7 / 52 | 1 / 3 |
| Kind 3 | 0 / 2 | 3 / 7 | 0 / 3 |
| Kind 4 | 0 / 18 | 7 / 60 | 0 / 11 |

The testify and compress pools equal G8's `pools.json` less G8's eight
targets.

### Draw

`keys/g9draw.py` draws from `pools.json`. Its seed is the first 8 hex digits
of the SHA-256 of `pools.json`. There are three strata:

- **reach** (LONG): kind 1.
- **name** (LONG): kinds 2 to 4 with `name_calls` at least 200.
- **short** (SHORT): kinds 2 to 4 with `name_calls` below 200.

| Repository | reach | name | short |
|---|---|---|---|
| testify | 1 | 2 | 1 |
| compress | 1 | 2 | 1 |
| kong | 2 | 0 | 2 |

That is 8 LONG questions (4 reach, 4 name) and 4 SHORT, four per
repository. kong has one name candidate, so its LONG questions are test
reach. Each draw picks a kind uniformly among the stratum's kinds that
still have a candidate, then a target uniformly from it, sorted by name. A
repository draws no target twice and at most one kind 4 target, because
each repository gets one change commit. For a kind 4 target the draw then
takes 2 of its callers outside test files, which the change edits with the
target, as in G8.

8 and 4 follow the suggested sizes. The primary test needs the LONG
questions, and SHORT questions are only a guard on a point estimate, so
they get the smaller share.
