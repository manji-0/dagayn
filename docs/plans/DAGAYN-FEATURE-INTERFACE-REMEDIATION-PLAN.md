# Dagayn feature/interface remediation plan

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#implementation-recommendations -->
<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#recommended-roadmap-shape -->
<!-- constrained-by ./ANALYSIS-TOOL-STRATEGY.md#mcp-tool-surface-plan -->
<!-- constrained-by ../COMMANDS.md#mcp-tools -->
<!-- constrained-by ../ARCHITECTURE.md#query-surfaces -->

## Goal

この計画は、dagayn を evidence-calibrated change-time reasoning engine として
直していくための実装順序を定義する。

対象は新しい機能面と user-facing interface の両方である。ただし、新しい top-level
tool を増やすことは主目的にしない。既存の compact MCP surface と dispatcher style を
保ち、各 tool response が claim、evidence、confidence、missingness、action を
安定して返せる状態へ寄せる。

## Current Baseline

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#decision-model -->

分析時点の graph は 8170 nodes、47716 edges、389 files、graph health score 1.0 だった。
`get_minimal_context_tool` は現在の worktree に対して risk 0.55 の medium と判定し、answerability を
`[364 flows, 370 communities, 6988 test edges, 598 reportable cross-artifact edges, 0.0 unresolved ratio]`
として返した。

architecture overview は 370 communities、5 coupled pairs shown、0 warnings だった。
health signal は hub nodes 5、bridge nodes 5、knowledge gaps 2642、surprising
connections 5、ADP violations 2、SDP violations 1、SAP violations 5 を返した。
overview output は truncated=true で、communities は 1/370、cross-community coupling
は 1/5 だけを保持した。

flow list は 10 flows を返し、top criticality は `activate` 0.665、
`embed` 0.61、`embed_query` 0.61 だった。これは flow evidence が既に存在する一方で、
workflow ごとの coverage と confidence を response contract に明示する余地があることを
示す。

この plan の元になった監査文書は graph 上で 72 nodes を持ち、impact analysis は
49 impacted nodes、18 affected files、truncated=false を返した。文書変更だけでも
docs、commands、roadmap、README に波及するため、計画変更は docs と interface contract を
同じ単位で扱う必要がある。

## Interface Direction

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#product-identity -->
<!-- derived-from ./ANALYSIS-TOOL-STRATEGY.md#tool-tiers -->

守るべき interface 方針は次の通り。

1. first call は `get_minimal_context_tool` に寄せる。
2. user-facing surface は `review_tool`、`flow_tool`、`architecture_analysis_tool`、
   `refactor_tool`、`query_graph_tool`、`semantic_search_nodes_tool` を中心に保つ。
3. 新しい分析は、まず既存 dispatcher の mode または response section として実装する。
4. response は verdict ではなく calibrated guidance を返す。
5. large list ではなく、少数の強い recommendation と next action を返す。
6. graph が安全に答えられない場合は、空欄や曖昧な summary ではなく missingness として返す。

## Step 1: Shared Guidance Contract

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#decision-model -->
<!-- dagayn: discusses-artifact ../../dagayn/tools/_common.py::make_response -->
<!-- dagayn: discusses-artifact ../../dagayn/tools/_common.py::compact_response -->

最初に、すべての workflow tool が共有できる guidance contract を定義する。

現在の `make_response` と `compact_response` は `status`、`summary`、`_hints`、
`next_tool_suggestions` を揃える土台として有効である。ただし、decision model の中核である
claim、evidence、confidence、missingness、action はまだ共通 envelope ではない。

実装方針:

1. `dagayn/tools/_common.py` に guidance item builder を追加する。
2. guidance item の必須 field を `claim`、`evidence`、`confidence`、`missingness`、
   `action`、`reason_codes`、`counts` にする。
3. confidence は `high`、`medium`、`low`、`unknown` に限定する。
4. evidence は `extracted`、`authored`、`computed`、`evaluated` の type を持てるようにする。
5. `_hints.next_steps` は guidance item の action から生成できる形に寄せる。
6. CLI の summary format でも、最低限 claim と action が残るようにする。

Done criteria:

1. `review_tool(mode="changes")`、`architecture_analysis_tool(mode="overview")`、
   `refactor_tool(mode="suggest")` の top recommendations が同じ guidance item shape を持つ。
2. 既存の `status`、`summary`、`_hints`、`next_tool_suggestions` の互換性を壊さない。
3. tests に contract snapshot を追加し、field 欠落を regression として扱う。

## Step 2: Answerability Propagation

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#implementation-recommendations -->
<!-- dagayn: discusses-artifact ../../dagayn/tools/context.py::_graph_answerability -->
<!-- dagayn: discusses-artifact ../../dagayn/tools/context.py::get_minimal_context -->

次に、answerability を `get_minimal_context_tool` だけの metadata から、すべての claim を
較正する shared signal へ昇格する。

現在は `_graph_answerability` が graph health、flow count、community count、test edge count、
reportable cross-artifact count、unresolved ratio を計算している。この情報は有用だが、
review、architecture、flow、refactor の個別 response には必ずしも伝播しない。

実装方針:

1. answerability summary を `_common.py` から呼べる shared helper に移す。
2. 各 dispatcher response に `answerability` または `missingness` block を追加する。
3. stale graph、missing flows、missing communities、missing test edges、low-confidence docs、
   missing embeddings、truncated output を明示的な reason code にする。
4. `semantic_search_nodes_tool` の `search_mode` と embedding health を missingness model に接続する。
5. 0 件 response は「存在しない」ではなく「現在の graph では見つからない」と表現できるようにする。

Done criteria:

1. graph が stale または partial な fixture で、各 tool が missingness を返す。
2. healthy graph では missingness が空または low-severity になり、summary が過度に noisy にならない。
3. docs に answerability field の読み方を追加する。

## Step 3: Calibrated Review Guidance

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#recommended-roadmap-shape -->
<!-- dagayn: discusses-artifact ../../dagayn/tools/review.py::detect_changes_func -->
<!-- dagayn: discusses-artifact ../../dagayn/tools/review.py::_change_analysis_summary -->

`review_tool(mode="changes")` を最初の本格適用先にする。

現在の review surface は既に `analysis_summary`、`reason_codes`、recommended tests、
affected-flow rankings、documentation candidates、`signal_quality`、
`stability_contracts` を返す。これは方向として正しい。次の改修では、これらを並列の raw
sections ではなく、actionable guidance item の集合として扱う。

実装方針:

1. `analysis_summary` に `guidance` list を追加する。
2. test recommendation、documentation candidate、stable contract warning、architecture delta、
   hotspot proximity を guidance item へ変換する。
3. 各 item に evidence type、source edge/metric、threshold、count、confidence、missingness、
   next action を入れる。
4. `signal_quality` は top-level caveat ではなく、各 item の confidence と missingness にも反映する。
5. `detail_level="minimal"` は top 3 guidance items を返し、standard は full sections と guidance を返す。
6. nested list を安全に trim できるように output budget 処理を直す。

Done criteria:

1. `review_tool(mode="changes", detail_level="minimal")` だけで、reviewer が次に実行すべき
   command/test/doc read を判断できる。
2. test gap と documentation update candidate が graph fact、heuristic、uncertain lead のどれかを
   item ごとに明示する。
3. budget truncation が nested field にも効き、`truncated` と `_truncation` が正確に出る。

## Step 4: Stability-Backed Quality Policy

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#stability-as-a-quality-spine -->
<!-- dagayn: discusses-artifact ../../dagayn/tools/review.py::_component_stability_profiles -->
<!-- dagayn: discusses-artifact ../../dagayn/tools/review.py::_stability_contracts -->

stable または should-be-stable component に対する品質期待を、review 専用 helper から
dagayn 全体の policy signal にする。

現在は review path が package-level SDP/SAP から stability profile を作り、
expected test density と expected doc density を計算している。この発想はよいが、
architecture output、refactor suggestion、documentation contract check でも同じ policy が
必要になる。

実装方針:

1. stability profile 計算を shared analysis module に移す。
2. threshold を output に必ず含める。特に instability max、afferent coupling threshold、
   expected test/doc density を隠さない。
3. `architecture_analysis_tool(mode="overview")` に stable component policy summary を追加する。
4. `refactor_tool(mode="suggest")` で stable component への remove/move/split suggestion を保守的にする。
5. documentation candidates は stable component ほど authored contract edge を強く要求する。

Done criteria:

1. 同じ component に対して review、architecture、refactor が矛盾しない stability status を返す。
2. stable component warning は threshold と reason code を持つ。
3. fixture で stable concrete pressure と high afferent coupling の両方を検証する。

## Step 5: Contract-Aware Documentation Graph

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#evidence-taxonomy -->
<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#implementation-recommendations -->

docs を passive corpus ではなく contract layer として扱う。

Markdown dependency directive、documentation directive、runbook、issue note は authored evidence であり、
単なる semantic similarity より強い。behavior、contract、intent に関する問いでは、inferred
relationship より authored relationship を優先する。

実装方針:

1. `docs_for` と `implementations_of` の結果を review/refactor guidance に統合する。
2. documentation candidate の evidence type を `authored`、`extracted`、`heuristic_reachable` に分ける。
3. low-confidence Markdown code-span edge は missingness として扱い、contract evidence と混同しない。
4. stable component に contract docs がない場合、test gap とは別の documentation obligation として返す。
5. docs update guidance は「どの section を読むか」「どの directive を追加/修正するか」まで示す。

Done criteria:

1. code change が authored contract section に接続している場合、review guidance がその section を top action に出す。
2. Markdown-only change では implementation impact と downstream docs impact を分けて返す。
3. unresolved or ambiguous cross-artifact edge は confidence low として表示される。

## Step 6: Refactor Work Packs

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#recommended-roadmap-shape -->
<!-- dagayn: discusses-artifact ../../dagayn/refactor/suggestions.py::_execution_plan_for_suggestion -->
<!-- dagayn: discusses-artifact ../../dagayn/refactor/suggestions.py::_work_pack_for_suggestion -->

`refactor_tool(mode="suggest")` は既に `execution_plan` と `work_pack` を返すため、ここでは
work pack を review/refactor contract に合わせて強くする。

実装方針:

1. work pack に blast radius、required tests、documentation obligations、safe first commit、
   rollback path、defer conditions を必須 field として揃える。
2. suggestion ranking は evidence type と confidence を明示する。
3. remove/move/split/document の各 suggestion に stability policy を反映する。
4. `apply_refactor_tool` は advanced/maintenance surface のままにし、通常 flow では dry run と
   verification command を先に示す。
5. dynamic dispatch、generated code、plugin registration、public API は missingness または defer condition として扱う。

Done criteria:

1. top refactor suggestion だけで、安全な first commit と verification command が分かる。
2. stable component に対する destructive suggestion は confidence が下がるか defer condition を持つ。
3. precision benchmark が refactor suggestion の accepted/relevant target を測れる。

## Step 7: Architecture And Flow Calibration

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#what-dagayn-is-not -->
<!-- dagayn: discusses-artifact ../../dagayn/tools/community_tools.py::_architecture_health_summary -->

architecture と flow は verdict ではなく lead として返す。

現在の architecture overview は `architecture_health` に counts、reason_codes、top_examples、
drill_downs を持つ。次は、metric が何を意味し、何を意味しないかを output に含める。

実装方針:

1. architecture health の top examples を guidance item 化する。
2. ADP/SDP/SAP は formula、threshold、artifact_scope、truncation state を必ず返す。
3. `artifact_scope` default が code であることを response に明示する。
4. flow criticality は ranking signal であり coverage guarantee ではないことを missingness に入れる。
5. `flow_tool(mode="get")` は source inclusion の有無、flow extraction coverage、truncation を明示する。

Done criteria:

1. architecture warning が human review lead であることを response shape が示す。
2. flow output が「この flow だけを見れば十分」と誤解されない。
3. architecture/flow docs の examples が new contract に更新される。

## Step 8: Search And Exploration Calibration

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#scope-discipline -->

exploration tools は、検索結果そのものより「次にどの exact graph query を呼ぶか」を強くする。

実装方針:

1. `semantic_search_nodes_tool` の result に exactness、source arm、embedding availability、
   ambiguity をまとめる。
2. exact identifier match が 0 件または複数件の場合、`query_graph_tool` へ進む前に確認すべき
   action を返す。
3. `query_graph_tool` は pattern ごとに result_count、confidence/truncation、zero-result reason を揃える。
4. docs/code mixed results は evidence type を明示し、Markdown body hit を code symbol hit と混同しない。

Done criteria:

1. agent が search hit を過信せず、次に読む file/symbol を選べる。
2. ambiguous symbol search の fixture が action と missingness を検証する。
3. docs search benchmark と code search benchmark の regression gate が残る。

## Step 9: Guidance Evaluation Loop

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#evaluation-recommendations -->
<!-- dagayn: discusses-artifact ../../dagayn/eval/benchmarks/guidance_precision.py::run -->

最後に、guidance が実際に役立つかを測る loop を強化する。

現在の `guidance_precision` は recommended tests、documentation update candidates、
refactor suggestions の precision@k を測れる。次は、answerability と calibrated guidance contract を
評価対象に入れる。

実装方針:

1. guidance item の precision@k を測る case kind を追加する。
2. stable contract warnings、architecture leads、answerability warnings の expected targets を設定できるようにする。
3. false-positive architecture warning と false-positive refactor suggestion を記録する。
4. common agent task ごとに context-size reduction、latency、accepted recommendation を測る。
5. CI では focused fixture、local audit では `dagayn eval` を使う二段構えにする。

Done criteria:

1. guidance contract の field coverage と precision が benchmark で見える。
2. raw graph size は health metric、guidance usefulness は product metric として分けて報告される。
3. new feature は evaluation case なしでは roadmap に入れない。

## Step 10: Documentation And Migration

<!-- derived-from ../audits/dagayn-fundamental-principles-recommendations.md#operating-principles -->

interface を変えるたびに、docs と migration examples を同じ change に含める。

実装方針:

1. `docs/COMMANDS.md` に guidance contract、answerability、minimal/standard の違いを追加する。
2. `docs/USAGE.md` に common workflows の before/after examples を追加する。
3. README の MCP tool surface 説明を新しい response contract に合わせる。
4. audit/plan docs には real dependency directive を追加し、graph が docs evolution を追えるようにする。
5. old field を消す場合は、少なくとも one release window は alias または migration note を残す。

Done criteria:

1. code、tests、docs が同じ interface contract を説明している。
2. `dagayn tool --list` と MCP default surface の docs が一致する。
3. `dagayn build` 後、plan/doc dependencies が graph で確認できる。

## Summary

<!-- derived-from #step-1-shared-guidance-contract -->
<!-- derived-from #step-2-answerability-propagation -->
<!-- derived-from #step-3-calibrated-review-guidance -->
<!-- derived-from #step-4-stability-backed-quality-policy -->
<!-- derived-from #step-9-guidance-evaluation-loop -->

dagayn の次の改善は、tool surface の拡大ではなく、既存 surface の response quality と
calibration を上げることに集中する。

実装順序は、shared guidance contract、answerability propagation、review guidance、
stability policy、documentation contract、refactor work packs、architecture/flow calibration、
search calibration、evaluation、docs migration の順に進める。この順序なら、各 step が前の
step の contract を再利用でき、agent が過信せずに次の安全な行動を選ぶという product identity に
近づける。
