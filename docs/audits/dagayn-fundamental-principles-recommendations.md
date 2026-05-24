# dagayn Fundamental Principles and Recommendations

<!-- derived-from ../../README.md#what-dagayn-does -->
<!-- derived-from ../ARCHITECTURE.md#pipeline-overview -->
<!-- derived-from ../ROADMAP.md#roadmap -->
<!-- derived-from ./dagayn-usability-scorecard.md#success-criteria -->

## Purpose

この文書は、dagayn の基本思想をもう一段掘り下げて再評価し、今後の
機能判断・設計判断・評価判断の基準をまとめるものである。

目的は、機能の種類や対応言語を広げることではない。既存の review、
refactor、architecture、debug、documentation contract の各ユースケースを
より高い精度と較正で実行できるようにするため、何を中核原則として守るべきかを
明文化する。

分析時点の graph は、8148 nodes、48473 edges、389 files、graph health
0.85、unresolved edges 1099、knowledge-gap signals 2644 だった。
architecture overview は status ok、369 communities、表示対象の coupled
community pairs 5、ADP violations 2、SDP violations 1、SAP violation
candidates は artifact scope と filtering により 5 から 7 件だった。

これらの数値は、dagayn の成功や失敗を直接示すものではない。むしろ、
dagayn が「一定の構造的証拠を持っているが、断言できない部分も多い」ことを
示す較正情報として扱うべきである。

## Core Thesis

dagayn は、AI coding agent のための evidence-calibrated context compiler
として設計されるべきである。

これは「repository knowledge graph」という説明より狭く、しかし実用上は強い。
graph は基盤であり、製品価値そのものではない。製品価値は、repository の情報を
task-specific で、根拠つきで、不確実性を明示した開発判断へ圧縮することにある。

主な価値は「repo についてより多く知ること」ではない。主な価値は、
agent や開発者が変更時に、少ない読解量で、過信を抑えながら、次の安全な行動を
選べるようにすることである。

## What Dagayn Is Not

dagayn は、汎用 static analysis platform になるべきではない。対応言語や
artifact type の拡大は、既存の review、refactor、architecture、debug、
documentation contract の各 workflow の判断品質を上げる場合にだけ価値がある。

dagayn は、graph metric を verdict として提示するべきではない。hub score、
bridge score、SAP distance、SDP direction、flow criticality、knowledge-gap
category は review lead であり、設計の良し悪しを直接証明するものではない。

たとえば stable concrete package が SAP pressure を示すことは、
必ずしも悪い設計を意味しない。その package が多くの code に依存される安定領域
であるなら、むしろ当然の pressure かもしれない。高 degree の function も、
test helper、CLI coordinator、実際の risk のいずれにもなり得る。

dagayn は missingness を隠すべきではない。unresolved edge、stale graph、
missing embedding、absent test edge、truncated result、low-confidence
documentation link は、補助情報ではなく一級の出力である。

## Product Identity

dagayn の最も強い identity は、change-time reasoning engine である。

静的な repository map は有用だが、dagayn の leverage が最大になるのは、
developer や agent が「今から何かを変える」瞬間である。その時に重要なのは、
repository 全体の百科事典的説明ではなく、変更に関する判断である。

変更時に問うべきことは次の通りである。

- この変更は、どの stable component に触れるのか。
- どの execution flow や community に波及し得るのか。
- stability、coupling、contract role から見て、どの test が期待されるのか。
- どの document が、この挙動の仕様・背景・運用上の制約を定義しているのか。
- どの refactor は安全そうで、どれは人間の確認を要するのか。
- 現在の graph では、どの問いに十分答えられないのか。

したがって dagayn の本質は、repository cataloging ではなく、change
understanding にある。cataloging は必要条件だが、最終目的ではない。

## Decision Model

dagayn の高価値な response は、実際の API 名が異なっていても、次の五つの要素を
持つべきである。

| Field | Meaning |
| --- | --- |
| Claim | dagayn が何を主張または推奨しているか |
| Evidence | その主張を支える edge、metric、threshold、flow、community、document role |
| Confidence | その主張をどの程度 action に反映すべきか |
| Missingness | 欠けている graph state、parser coverage、evidence、または truncation |
| Action | 次に読む file、実行する test/command、または呼ぶべき graph tool |

この model は、review guidance、architecture signals、refactor suggestions、
semantic search、flow analysis、documentation links、graph health summary
のすべてに適用できる。

この形を徹底すると、graph output が「それっぽい説明」へ崩れる危険が減る。
dagayn は、agent の推論を増幅するのではなく、agent の認識を較正するためにある。

## Evidence Taxonomy

dagayn は、少なくとも四種類の evidence を区別するべきである。

Extracted evidence は parser が直接抽出した証拠である。calls、imports、
contains edges、Terraform references、Markdown links、test edges などが
該当する。

Authored evidence は、人間が文書や code comment に書いた意図の証拠である。
Markdown dependency directives、documentation-to-code relationship roles、
runbook や issue note への明示 link が該当する。これは単なる類似度より強い。

Computed evidence は graph から導出された指標である。communities、flows、
centrality、stability、abstraction distance、risk score が該当する。
これは ranking や prioritization に有用だが、threshold と reason code による
説明可能性を伴うべきである。

Evaluated evidence は、dagayn の guidance が実際に有用だったかを測る証拠である。
precision、accepted recommendation、false-positive rate、regression test が
該当する。product quality を上げる上では、この tier が最も重要である。

今後は、computed metric を増やすことより、extracted evidence と authored
evidence を evaluated guidance へ接続することを優先するべきである。

## Stability As A Quality Spine

Clean Architecture の stability は、dagayn の品質判断の背骨として自然に使える。

stable または should-be-stable な component には、より高い品質期待を置くべきで
ある。

- test density が高いこと
- written contract が明文化されていること
- undocumented behavior change への警戒度が高いこと
- 変更時の review warning が強いこと
- refactor recommendation が保守的であること

重要なのは、stability を architecture description に留めないことである。
stability は品質期待を変える policy signal である。afferent coupling が高い
component は、単に重要なのではない。他の code がその挙動に依存しているため、
test、documentation、contract traceability の要求水準も高くなる。

この発想により、architecture analysis は diagramming feature ではなく、
review policy engine になる。

## Function-Level Concern Separation

<!-- derived-from #decision-model -->
<!-- derived-from #evidence-taxonomy -->
<!-- derived-from #scope-discipline -->

関数レベルの関心の分離は、既存 workflow の品質を上げる metric として妥当である。
ただし、それは「単一責務違反を証明する」metric ではなく、split、review、
documentation guidance の優先度を上げるための profile として扱うべきである。

この profile は三つの観点を分けて返すのがよい。第一に、単一責務の圧力である。
callee community の広がり、callee scope の広がり、branch count、outgoing call
count は、一つの関数が複数の変更理由を抱えている可能性を示す。第二に、純粋性の
低さである。filesystem、database、network、environment、time/random、logging
のような side effect evidence は、decision logic と IO が混在している可能性を示す。
第三に、context clarity である。parameter count、boolean flag、return contract
の欠落、曖昧な function name は、呼び出し側から意味を読み取りにくい可能性を示す。

実現性は高いが、較正が必要である。静的解析だけでは、関数が本当に単一責務か、
本当に pure か、domain context が十分かを確定できない。とくに CLI handler、
adapter、framework entry point、test helper は、IO や orchestration を持つことが
自然である。したがって出力には role、score、confidence、reason codes、
evidence、missingness、action を含め、role-aware な lead として提示する必要が
ある。

この指標の最初の適用先は、refactor suggestion の split evidence がよい。
大きい関数に対して、branch-heavy や many-collaborators だけでなく、concern
pressure が高い場合にも候補にできる。これにより、長さは同程度でも「純粋な変換」
として読める関数と、「IO、判断、暗黙 context が混じる」関数を区別できる。
将来的には review finding や documentation expectation にも同じ profile を渡せる。
その場合も、verdict ではなく、次に読むべき範囲や抽出すべき最初の責務を示す
actionable evidence として扱うべきである。

## Scope Discipline

新しい work item は、roadmap に入る前に少なくとも一つの条件を満たすべきである。

- 既存 workflow の precision を上げる。
- confidence calibration または missingness の可視性を上げる。
- agent が action の前に読むべき source 量を減らす。
- 既存 graph evidence を concrete next action に接続する。
- stable または should-be-stable component の保護を強める。
- dagayn 自身の recommendation を評価しやすくする。

逆に、単に artifact type、visualization mode、metric、language surface を増やす
だけで、上記の outcome に接続しない work は defer するべきである。

## Implementation Recommendations

第一に、answerability を shared concept として明示する。graph health signal は
すでに存在する。次は、claim を返す tool response に answerability を伝播させる。
graph が fresh か、必要な edge kind が存在するか、結果が truncated か、
cross-artifact edge が low-confidence か、changed file の parser coverage が弱い
かを、response が明示するべきである。

第二に、guidance を decision model に揃える。review、refactor、architecture、
flow の各 tool は、claim、evidence、confidence、missingness、action を持つべき
である。これは新しい user-facing tool を増やす話ではない。既存の focused
surface の response contract を強くする話である。

第三に、stability-derived quality expectation を first-class にする。test と
documentation の recommendation は、observed stability、should-be-stable
pressure、public contract role、flow criticality、change proximity によって
重みづけされるべきである。目標は recommendation を増やすことではなく、
少数の強い recommendation を visible rationale つきで返すことである。

第四に、guidance precision の evaluation に投資する。precision at k、採用された
test recommendation、false-positive refactor suggestion、review finding の
usefulness は、raw graph size より重要である。top recommendation が実際に役立った
かを測る小さな benchmark は、未検証の metric を増やすことより価値が高い。

第五に、MCP surface は compact に保つ。minimal context から始め、mode で drill
down する current dispatcher style は product identity に合っている。新しい tool
は、本当に新しい workflow を表す場合だけ追加するべきである。多くの改善は surface
expansion ではなく response quality improvement として実装すべきである。

第六に、docs を passive corpus ではなく contract layer として扱う。Markdown
dependencies、documented contracts、runbooks、issue notes は、component がなぜ
stable なのか、どの behavior を変えてはいけないのか、どの test が必要なのかを
説明するために使う。documentation quality は stable / should-be-stable component
ほど厳しく評価するべきである。

## Evaluation Recommendations

最も重要な評価対象は graph completeness ではない。calibrated action quality で
ある。

有用な metrics は次の通りである。

- recommended tests、docs、refactor work packs の precision at k
- explicit evidence と confidence を持つ recommendation の割合
- architecture warning の human review 後 false-positive rate
- stable component の test / contract documentation 充足率
- workflow ごとの answerability coverage
- common agent task における latency と context-size reduction

graph-size metrics は operational health metrics として残すべきである。ただし、
downstream decision outcome に結びつかない限り、product quality metrics として
扱うべきではない。

## Operating Principles

自信のある広い答えより、較正された部分的な答えを優先する。

大きな ranked list より、証拠の強い少数の recommendation を優先する。

新しい surface area より、既存 workflow の品質向上を優先する。

behavior、contract、intent に関する問いでは、inferred relationship より authored
relationship を優先する。

一律の test / documentation rule より、stability-aware quality expectation を
優先する。

graph richness だけを測る metric より、developer usefulness を測る evaluation
loop を優先する。

## Recommended Roadmap Shape

次の roadmap は、artifact type ではなく capability で整理するべきである。

1. Calibrated review guidance: すべての finding が evidence、confidence、
   missingness、action を持つ。
2. Stability-backed quality policy: stable component に明示的な test と
   documentation expectation を与える。
3. Contract-aware documentation graph: authored docs を review / refactor の
   constraint として扱う。
4. Refactor work packs: suggestion を小さく検証可能な単位へ束ね、blast radius と
   test guidance を添える。
5. Guidance evaluation: precision と usefulness を継続的に測る。
6. Answerability reporting: 各 workflow が、安全に答えられない場合を明示する。

この roadmap は、既存ユースケースを維持しながら、その実行品質の上限を上げる。

## Summary

<!-- derived-from #core-thesis -->
<!-- derived-from #product-identity -->
<!-- derived-from #decision-model -->
<!-- derived-from #stability-as-a-quality-spine -->
<!-- derived-from #function-level-concern-separation -->
<!-- derived-from #implementation-recommendations -->

dagayn は evidence-calibrated change-time reasoning を最適化するべきである。
graph は必要だが、製品そのものではない。製品は、graph が可能にする guidance で
ある。何が変わったのか。何が重要なのか。何が risky なのか。その claim を支える
evidence は何か。何が missing なのか。agent は次に何をするべきなのか。

短期的に最も強い投資先は、scope の拡大ではない。precision の向上、confidence の
明示、missingness の出力、stability-aware quality expectation、そして dagayn の
recommendation が実際の開発判断を良くしたかを測る evaluation である。

## Conclusion

<!-- derived-from #summary -->

最終提言は、dagayn を strict evidence contract を持つ agent context compiler と
して位置づけることである。今後の work は、既存 workflow の decision quality または
calibration を改善する場合にだけ採用する。この制約により、scope を無闇に広げずに、
AI-assisted development をより安全で、速く、過信しにくいものにするという本来の
野心を保てる。
