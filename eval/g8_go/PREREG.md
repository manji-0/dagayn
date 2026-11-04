# Pre-registration: G8, the Go agent experiment rerun

<!-- derived-from ../g7_go/PREREG.md -->
<!-- derived-from ../../docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md#decisions-2026-11-04 -->

Draft. This commit holds only how the questions are picked. The rest of the
pre-registration follows in later commits, before any agent run.

## How the questions are picked

G7's eight questions were chosen after dagayn's answers to them were seen,
and four defects were then fixed against them. G8's primary questions are
eight new ones, picked from the answer keys' data by a fixed rule and a
seeded draw, before any dagayn answer to them exists.

1. `keys/build_inputs.sh` builds, for each repository, the tag's archive
   (the harness's target with no change commit): the scip-go v0.2.7 index
   (`scipdump`, `sciprels`), every `func` declaration (`gofuncs`), and two
   per-test coverage runs of every package without `-short`
   (`per_test_cover.py`). None of it uses dagayn.
2. `keys/pools.py` lists the candidates of each question kind from those
   files. It leaves out functions in `_test.go` files and the eight G7
   targets. A caller is a `func` declaration that holds a scip-go reference
   to the target followed by `(`; any other reference is a value.
   - Kind 1, which tests execute a function: 2 to 15 top-level `Test`
     functions executed it in both coverage runs, and fewer than half of
     those tests contain its name.
   - Kind 2, callers of a method on a receiver whose type the call site does
     not name: a method with 3 to 20 callers whose name 3 or more `func`
     declarations of the repository share.
   - Kind 3, callers through an interface or a function value: the only
     method of the repository that implements a method of an interface the
     repository declares, with at least one call through that interface; or
     a function or method whose every value reference is stored in a field or
     package variable, each of which is called.
   - Kind 4, what a change left unedited: a function or method with 6 to 15
     callers, in 2 or more files, 2 or more of them outside test files.
   - Kinds 2 and 4 also need the key to be the direct callers: no value
     reference, no call through an interface the method implements, and no
     call outside a function.
3. `keys/draw.py` draws from `pools.json`, which this commit registers. Its
   seed is the first 8 hex digits of the SHA-256 of `pools.json`. For each
   repository, testify then compress, and each kind, 1 to 4, it takes one
   target uniformly from the pool sorted by name, drawing again on a target
   the repository already has. For kind 4 it then draws 2 of the target's
   callers outside test files.
4. The kind 4 change commit edits the target's body, so that its behaviour
   changes and its signature does not, and the two drawn callers, and
   nothing else. It adds or removes no reference to any of the repository's
   four targets. It is written after the draw, by hand.

The rule was written by someone who had read G7's results, and it was
adjusted while looking at the pools' sizes (not at any draw): kind 2 first
required 3 types to declare the name, which left 2 testify candidates. The
pools are committed so that a reader can rerun the draw.
