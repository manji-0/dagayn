# Open measurements

<!-- derived-from ../POSITIONING.md#open-measurements -->

[POSITIONING](../POSITIONING.md#open-measurements) keeps six features only if
a measurement shows they help an agent. This note records one measurement of
each on real repositories, with a verdict: keep, cut, or fix-then-keep.

Measured on 2026-10-25 at `fix/rust-test-reach` (1624781e) with the branch's
own `dagayn` build. Corpora are clones of four of the maintainer's
repositories: dagayn itself (Python and Rust, 15,649 nodes), cfwdon (Rust and
TypeScript, 9,392), purecrate-ts (Rust and TypeScript), and tstodon
(TypeScript, 11,808). The flow, review, refactor, community, and cross-repo
judgments were made by hand from source and carry a file:line reason; the
search judgments carry a graded relevance, pooled from both arms' top 5. Each
judgment file sits next to the script that reruns it.

## Verdicts

| Feature | Verdict | Deciding number |
| --- | --- | --- |
| Embeddings (hybrid search) | keep | Japanese MRR 0.04 → 0.55; English MRR 0.48 → 0.68; 21 wins, 1 loss in 38 queries |
| `flow_tool(mode="entry_points")` | fix-then-keep | recall 0.71 at the default limit, 0.85 at limit 50; the default hides every real entry of a widely called helper |
| `review_tool` `impact` | cut | 11 actionable facts beyond `changes` in 44 commits, all from one commit and all returned by `callers_of` at a sixth of the characters |
| `review_tool` `context` | cut | 1 actionable fact in 44 commits; keeps a median of 1 of 4,754 radius nodes, unranked |
| `refactor_tool(mode="suggest")` | fix-then-keep | precision 17/50; `unused_symbol` 9/11 once one fixture file is excluded, `undocumented_surface` 3/18 |
| communities modes | cut | 3 of 35 hand-checked communities useful; 200 of 330 tstodon names duplicated; the tool cannot list its own communities |
| Registry + `cross_repo_search_tool` | fix-then-keep | same hits as per-repo search on 16 of 16 queries; 2 calls → 1, but 11% more characters and per-repo exactness lost |
| Watch daemon | keep | an edit is searchable 0.61 to 0.69 s later (n=6); without the daemon it is not indexed |

## Search embeddings

<!-- derived-from ../LOCAL-EMBEDDINGS.md#search-quality -->

Rerun: `eval/run_search_eval.py --repo dagayn=PATH --repo tstodon=PATH`
with BGE-M3 `material` embeddings built by `dagayn update --local-embedding`
(14,861 of 14,861 non-file nodes in dagayn, 11,010 of 11,010 in tstodon).

The eval runs one `hybrid_search` per query twice: as shipped, and with the
embedding arm replaced by an empty list, so FTS sub-queries, keyword fallback,
and reranking are shared. 38 queries (16 dagayn, 22 tstodon): 8 exact-name,
5 keyword, 13 English prose, 12 Japanese. Judgments were pooled from the top 5
of both arms in two rounds; a seeded typo in a judged target makes the eval
refuse to run.

| Slice | n | MRR@10 FTS → hybrid | nDCG@10 | P@5 | success@5 |
| --- | ---: | --- | --- | --- | --- |
| all | 38 | 0.339 → **0.636** | 0.306 → 0.614 | 0.132 → 0.274 | 0.45 → 0.76 |
| exact name | 8 | 0.891 → 0.938 | 0.890 → 0.943 | 0.225 → 0.275 | 0.88 → 1.00 |
| keyword | 5 | 0.450 → 0.567 | 0.418 → 0.631 | 0.200 → 0.320 | 0.80 → 0.80 |
| English prose | 13 | 0.231 → 0.559 | 0.158 → 0.466 | 0.154 → 0.262 | 0.38 → 0.69 |
| Japanese | 12 | 0.042 → **0.547** | 0.029 → 0.547 | 0.017 → 0.267 | 0.08 → 0.67 |

Per query, hybrid wins 21, loses 1 (`ts-kw-trending-statuses`, rank 2 → 3),
and ties 16. On English queries alone it wins 11 and loses 1. FTS returns
nothing at all for 6 of the 12 Japanese queries, because the code is named in
English. Every hybrid reply reported `hybrid` or `embedding_only` with the
full vector count, so no row silently fell back to FTS. Before the second
judgment round the gap was smaller (all-query MRR 0.334 → 0.573); the second
round added nodes only the hybrid arm surfaced, which is what pooling is for
but does favor the arm that found them.

Latency, median of 5 alternating passes per query, query-embedding cache
cleared before every pass: FTS 4.96 ms (p90 6.98), hybrid 24.35 ms (p90
28.22). The limiter is the sidecar call: one `/v1/embeddings` POST to the
local BGE-M3 `llama-server` takes 16.7 to 17.2 ms (median of 20), and the
vector scan itself about 4 ms. With the cache left on (an earlier run) the
same comparison read 4.7 vs 9.1 ms, which measured the cache, not the work.
Four other measurement jobs shared the 18-core machine (load average about
4); arms were interleaved per pass, and the p10 to p90 spread of each arm
stayed within 9 ms.
The other costs are outside the query: embedding the graph took 325 s for
tstodon and 328 s for dagayn (batch 8), and the sidecar stays resident.

Verdict: **keep.** The deciding fact is Japanese: FTS answers 1 of 12
Japanese queries in its top 5, hybrid answers 8. On English prose it doubles
MRR; on exact names it changes nothing an agent would notice. The 20 ms per
query is small next to the reply an agent reads.

## Flow entry points

Rerun: `eval/run_flow_entry_measure.py`; judgments in
`eval/flow_entry_judgments.yaml`.

17 targets (9 Rust, 8 TypeScript) with 52 hand-established ways in (Worker
event handlers, route components, exported API, CLI `main`). Each was asked
of `flow_tool(mode="entry_points")` and of `query_graph_tool(callers_of,
depth=6)`.

| Measure | entry_points (limit 50) | entry_points (default 10) | callers_of depth 6 |
| --- | --- | --- | --- |
| precision | 0.789 | | 0.979 |
| micro recall | 0.846 | 0.712 | 0.885 (upper bound) |
| macro recall | 0.804 | 0.727 | 0.706 |
| median characters | 1,868 | | 4,376 |

13 of 17 targets get the same answer from both. callers_of truncated on 3
targets, and its reply carries no edges, so the roots above were recovered by
looking up callers in `graph.db`: an agent reading the reply alone cannot do
that, which makes its column an upper bound. Every returned chain hop is a
graph edge.

Where entry_points wins: `crates/check/src/types.rs::show` (purecrate-ts)
reaches `main` at 7 hops in 1.5K characters, where callers_of returns 39.6K
characters, 107 of 214 rows, and no root.

Where it fails: entries are sorted by chain length, and any `handle_*` name
counts as an entry. For `crates/cfwdon-worker/src/id_utils.rs::generate_entity_id`
the default 10 are all inbox `handle_*` helpers; the real entries `scheduled`
(4 hops) and `handle_fetch` (5 hops) appear only at limit 50. All 12
imprecise entries are `handle_*` name matches. Route components called only
by their own tests (`web-ui/src/ui/pages/ListsPage.tsx::ListsPage`) are
dropped because test callers count as callers (`entry_points.rs:45`).

Verdict: **fix-then-keep.** The deciding fact is the default limit hiding the
real entries of the widely called helpers, which are exactly the targets
where it beats callers_of. Fixes: rank boundary kinds (`main`, framework
handler, uncalled) before `named_entry`, stop treating a `handle_*` name with
production callers as an entry, and ignore test callers in `has_callers`.
This is answer correctness against hand ground truth, not an agent
experiment; that gap in POSITIONING stays open.

## Review impact and context

Rerun: `DAGAYN_MEASURE_DIR=DIR eval/run_review_modes_measure.py run`, then
`summarize`, at b0250d3f; both modes are removed after it, and the script with
them. Judgments in `eval/review_modes_judgments.yaml`.

44 commits, 11 per repository, picked for touching code symbols (release,
lockfile, and docs-only commits skipped; the reason for each is in the
script). Each ran in a private clone with the graph brought to the commit
(one commit checked against a full rebuild: identical reference sets in all
three modes), then `review_tool` with `base=C~1` in modes `changes`,
`impact`, and `context` at their defaults. 0 errors.

The references each reply names were diffed against `changes`, classified
mechanically (unchanged sibling in a changed file, callee, container file,
unlinked doc, second-hop), and every remaining caller, test, or doc
candidate was judged by hand: 26 for impact, 1 for context.

| Repository | Median characters: changes / impact / context |
| --- | --- |
| dagayn | 5,236 / 25,658 / 34,108 |
| cfwdon | 6,142 / 32,936 / 33,941 |
| purecrate-ts | 3,782 / 36,081 / 34,633 |
| tstodon | 5,635 / 30,534 / 33,839 |
| all | 5,000 / 30,572 / 34,267 |

| Facts beyond `changes` | impact | context |
| --- | --- | --- |
| actionable, all 44 commits | 11 (all in one commit) | 1 |
| judged noise | 15 | 0 |
| mechanical noise | 2,809 | 1,321 |
| unchanged siblings, share of added | 76% | 83% |

The one commit is cfwdon `5e96c75c`, where `upsert_remote_status`'s `env`
argument started deciding whether a status streams. impact lists 11 of its
callers; one, `status_interactions.rs::upsert_embedded_announce_target`, was
a real bug fixed in the next commit (`3b5ad59d`). `query_graph_tool(callers_of)`
returns all 12 callers in 5,405 characters against impact's 11 in 32,557.
`changes` named all 14 direct tests in the radius and neither mode added
one.

"context keeps 4 of 954", in these numbers: every node of a changed file
seeds the radius, so the 2-hop radius has a median of 4,754 nodes. context
caps its own count at 500 and keeps a median of 1. Of 1,591 caller, test,
and doc candidates across all commits, context kept 1 and impact 29. The
survivors are not ranked: nodes are ordered by depth then qualified name
before the limit (`impact_radius.rs:75-78`), and the budget drops
`impacted_nodes` before `changed_nodes`. In tstodon `4c66e184` the real
caller `routes/statuses.ts` is in the radius, and both modes drop it while
keeping 61 siblings. 43% of context's characters are source of files the
diff already shows.

Verdicts: impact **cut**, context **cut**. The deciding fact is 11 actionable
facts in 44 commits for impact, all reproducible with one `callers_of` call at
a sixth of the characters, and 1 in 44 for context. The tstodon result is
limited by the graph (route handlers are anonymous arrows whose calls attach
to the file node), not by the modes.

## Refactor suggestions

Rerun: `eval/run_refactor_suggest_measure.py`; judgments in
`eval/refactor_suggest_judgments.yaml`.

50 findings judged across the 4 repositories as actionable, false, or noise.

| Kind | Actionable | Note |
| --- | --- | --- |
| `unused_symbol` | 9/20 | 10 of the 11 false are `corpus/patterns.rs::p0*` in purecrate-ts, survey input (`patterns.rs:1-2`); 9/11 with that file counted once |
| `complex_hotspot` | 5/12 | the rest are long dispatch matches or linear pipelines; churn half of the claim unchecked |
| `undocumented_surface` | 3/18 | self-describing structs, or types whose variants are already documented (`crates/dagayn-build/src/lock.rs::LockMode`) |
| all | 17/50 | |

Replies are 6.4K to 14.0K characters at standard detail, and verbose shows
the same 10 findings per kind as standard although the summary counts more
(71 hotspots in dagayn, 10 shown).

Verdict: **fix-then-keep.** The deciding fact is that `unused_symbol` is
right on hand-written code (`crates/dagayn-graph/src/flows.rs::GraphStore.get_node_kind_by_id`,
`web-ui/src/domain/navigation/route.ts::AppRoute.toSearchParams`) but one
fixture or generated file fills all 10 slots it shows. Fixes: leave out
fixture, corpus, and generated files, or cap findings per file; drop
`undocumented_surface`.

## Communities

Rerun: `eval/run_communities_measure.py`; judgments in
`eval/communities_judgments.yaml`.

Purity against declared units and their top-level modules, from the graph's
community assignment (members of the 3 largest communities per repository
matched the tool's reply exactly):

| Repository | Unit purity | Module purity | Nodes in communities spanning units |
| --- | --- | --- | --- |
| cfwdon (code) | 0.949 (baseline 0.699) | 0.508 (baseline 0.10) | 0.108 |
| tstodon (no generated Prisma) | 0.999 | 0.568 | |
| dagayn | 0.857 | 0.656 | 0.428 |

72% of tstodon's community nodes are generated Prisma code. cfwdon-worker is
split into 182 communities, 39 of them needed to cover 80% of its nodes; 164
of cfwdon's 326 communities have fewer than 10 nodes.

Hand check of 35 communities (cfwdon 13, tstodon 12, dagayn 10): 3 useful, 20
restate a unit, directory, or file, 12 incoherent. The useful three are all in
cfwdon (account provisioning across the models and domain crates, quote state,
inbound federation), and each carries a name (`src-status-sub6`) that does not
say what it holds.

The tool also cannot list itself: `top_n=100000` at minimal detail returned
163 of 326 communities, standard detail returned 1. 200 of 330 tstodon names
and 97 of 326 cfwdon names are duplicates, and `community_name="models-account"`
silently returned 1 of the 4 communities with that name. One `mode=community`
reply is 21K to 155K characters.

Verdict: **cut.** The deciding fact is 3 useful communities out of 35, against
units that already state the module boundaries in `mode=overview`.

## Multi-repo registry, daemon, and cross-repo search

Rerun: `eval/run_cross_repo_measure.py`; judgments in
`eval/cross_repo_judgments.yaml`. Everything ran with `HOME` pointed at a
scratch directory; the Rust `list_repos` reads `$HOME` too
(`crates/dagayn-tools/src/repos.rs:39`).

End to end on cfwdon and tstodon clones, 30 of 30 steps passed: register,
duplicate register, `repos`, `list_repos_tool`, an unbuilt repository
reported in `repos_skipped`, `daemon add/start/status/stop` with no watcher
left behind.

Daemon latency over 6 edits (appends and new files, Rust and TypeScript): the
node row appears 0.43 to 0.44 s after the write, 0.3 s of it the debounce,
and `cross_repo_search_tool` finds it 0.61 to 0.69 s after. With the daemon
stopped, the same edit was not indexed after 5 s.

Search over 16 queries: per-repo top-10 sets equal on 16 of 16, order equal on
32 of 32 lists, which follows from the implementation (it runs the same
`hybrid_search` per repo and sorts by score, `dagayn/tools/registry_tools.py`).
It takes 16 calls instead of 32, but 198,746 characters against 179,312 for
per-repo search at standard detail (85,924 at minimal); 43,076 of them are a
full `repo_path` on every hit and repeated `embedding_health` blocks. Scores
are per-repo RRF sums, so the merged order mostly alternates repositories:
in 6 merged lists judged by hand, 4 were sensible and 2 misleading, such as
cfwdon's `D1Database` definition tied with a tstodon function that only uses
the type. The merged reply drops the per-repo `exactness` that says which
repository defines the name.

Verdicts: registry **keep** (the only source of repository paths); daemon
**keep** (the only thing that indexes an edit between tool calls);
`cross_repo_search_tool` **fix-then-keep**. The deciding fact is that it
answers nothing per-repo search cannot and costs more characters; it earns
its call only by saving one call per extra repository. Fixes: carry
`exactness` and `zero_result_reason` per repository, accept `detail_level`,
and send `repo_path` once per repository.

## Caveats

- One person wrote all four repositories, so names follow one style, which
  helps FTS on exact names and may not hold elsewhere.
- Sample sizes are small: 38 search queries, 17 flow targets, 44 commits, 50
  refactor findings, 35 communities, 16 cross-repo queries. A 1-query swing moves search MRR by about 0.03.
- One judge. Search judgments were pooled from both arms; the flow, review,
  refactor, and community judgments were made by delegates, and a few per
  feature were re-checked against source rather than double-judged.
- The search eval used `material` embeddings only; no query in the set
  routed to `narrative`. The cross-repo measurement ran on FTS-only graphs.
- `eval/flow_entry_judgments.yaml` is the ground-truth artifact; the
  one-off script that wrote it was not committed.
- None of this is an agent experiment: it measures whether the answer is
  right and what it costs, not whether an agent reaches a better outcome.
