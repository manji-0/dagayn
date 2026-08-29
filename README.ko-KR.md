# dagayn

> **DAG is All You Need** — 지식 그래프를 중심으로 한 코드 리뷰와 영향 분석 접근.

`dagayn`은 `code-review-graph`의 포크입니다. 폴리글롯 저장소, 특히 인프라 비중이 높은 코드베이스를 대상으로 한 실용적 AI 지원 리뷰에 특화되어 있습니다.

상류의 그래프 중심 리뷰 모델을 계승하면서 독자 제품으로 문서와 유지보수를 합니다. 주요 차별점은 Terraform의 일급 지원, 포크 고유 파싱을 위한 커밋 고정 그래머 취득, 더 넓은 플랫폼 설치 흐름, 그리고 앱 코드·문서·인프라가 섞인 모노레포 대응 강화입니다.

## 무엇을 하는가

`dagayn`은 저장소를 로컬 SQLite 지식 그래프로 변환합니다. 파일, 심볼, 참조, 호출 엣지, 임포트, 테스트 링크, 커뮤니티, 실행 흐름을 기록합니다. AI 에이전트는 작업마다 저장소 전체를 다시 읽지 않고 이 그래프에 질의할 수 있습니다.

실질적 이점:

- 리뷰 컨텍스트 창 축소
- 변경 영향 범위의 빠른 분석
- 더 안전한 리팩터
- 대규모 저장소에서의 내비게이션 향상
- 코드·문서·노트북·Terraform을 한 워크플로로 다루는 것

## 포크로의 위치

`dagayn`은 명시적으로 `code-review-graph`의 포크입니다.

상류 문서를 정전으로 다루지 않습니다. 이 저장소의 가이드, 예제, 명령 설명은 모두 `dagayn` 자신을 위해 작성됩니다.

상류 귀속·원저자 정보는 [NOTICE](NOTICE)를 참조하세요.

## 주요 특징

- `.tf` 및 `.tfvars`의 일급 Terraform 파싱
- Markdown 구조와 의존 주석, 그리고 `dagayn:` 문서 링크 추출
- `.ipynb` 노트북 파싱
- 네이티브 일본어 FTS(Lindera IPADIC 형태소 + CJK bigram). 활용형 쿼리도 AND 매치
- 증분 그래프 갱신, 워치 모드, worktree sync, session prepare
- AI 코딩 도구용 MCP 서버
- 임팩트 반경·리뷰 컨텍스트·커뮤니티·플로·리팩터 그래프 쿼리
- 네이티브 Rust 그래프 스토어, 파서, FTS, 플로, 후처리(`dagayn._core`)
- 멀티 저장소 레지스트리와 데몬 워크플로
- GraphML / Mermaid C4 / SVG / Cypher / Obsidian 그래프 내보내기

## 지원 언어·파일 종류

주요 애플리케이션 언어에 더해 저장소 부속 형식을 다룹니다.

주요 항목:

- Python, JavaScript, TypeScript, TSX, Go, Rust, Java, C, C++, C#, Ruby, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, Objective-C, Bash, Elixir, Zig, PowerShell, Julia, Perl, R, GDScript, Vue, Svelte, Astro
- Markdown
- Jupyter 노트북과 Databricks 노트북 소스/내보내기를 그래프 입력으로 파싱
- Terraform

현재 커버리지 요약은 `docs/FEATURES.md`와 `docs/LLM-OPTIMIZED-REFERENCE.md`를 참조하세요.

## Terraform 지원

`dagayn`은 Terraform을 애플리케이션 코드와 동등한 일급 언어로 다룹니다. `.tf`와 `.tfvars` 모두 전용 Tree-sitter 그래머로 파싱합니다.

### 파싱 대상 블록

| 블록 | 한정명 패턴 | 그래프 종류 |
|---|---|---|
| `resource "type" "name"` | `resource.type.name` | Class |
| `data "type" "name"` | `data.type.name` | Class |
| `variable "name"` | `var.name` | Function |
| `locals { key = … }` | `local.key`(속성마다) | Function |
| `output "name"` | `output.name` | Function |
| `module "name"` | `module.name` | Class |
| `provider "name"` | `provider.name` | Class |
| `terraform {}` | `terraform` | Class |
| `check "name"` | `check.name` | Test |
| `ephemeral "type" "name"` | `ephemeral.type.name` | Class |
| `import {}` | 엣지만 | — |
| `moved {}` | 엣지만 | — |
| `removed {}` | 엣지만 | — |

### 생성되는 엣지 종류

- **REFERENCES** — 블록 본문의 `var.x`, `local.x`, `module.x`, `output.x`, `provider.x`, `data.type.name`, `resource_type.name` 식. 전용 정규식으로 추출하고 Terraform 내장 접두사(`count`, `each`, `path`, `self`, `terraform`)는 건너뜁니다.
- **CALLS** — `merge(…)`나 `length(…)` 같은 내장 함수 호출.
- **IMPORTS_FROM** — `module` 블록과 `terraform required_providers`의 `source` 속성, 그리고 `import` 블록의 대상.
- **CONTAINS** — 파일과 그 파일에서 정의된 각 블록의 포함 관계.
- **DEPENDS_ON** — `terraform` 블록의 `required_providers` 버전 제약.

### 크로스 모듈 분석

`module` 블록의 `source`가 로컬 경로를 참조하면, 호출 모듈에서 대상 디렉터리로 `IMPORTS_FROM` 엣지가 기록됩니다. 이로써 임팩트 반경 쿼리가 모듈 경계를 넘을 수 있습니다.

### `.tfvars` 파일

변수 값 파일(`.tfvars`)은 Terraform으로 파싱됩니다. 최상위 속성 대입은 `var.name` 노드가 되고 `.tf` 파일의 대응 `variable` 블록으로 REFERENCES 엣지로 연결됩니다. 변수 데이터 흐름의 완전한 그림이 그래프에 나타납니다.

## Markdown 지원

`dagayn`은 소스 코드와 함께 Markdown 문서에서 그래프 노드와 엣지를 추출합니다. 산문 아키텍처 결정과 그것이 설명하는 코드가 같은 그래프에 나타납니다.

### 파싱 대상 노드 종류

| 요소 | 한정명 패턴 | 그래프 종류 |
|---|---|---|
| 문서 | 파일 경로 | File |
| `# 제목` ～ `###### 제목` | `file::slug` | DocSection |
| Setext H1 / H2(밑줄 형식) | `file::slug` | DocSection |
| 제목 아래 문단/목록/표/코드 본문 | `file::slug--body-N` | DocBody |

제목 슬러그는 GitHub Markdown 규칙을 따릅니다: 소문자화, 공백과 하이픈을 `-`로 통일, 영숫자가 아닌 문자 제거. 같은 파일 안 중복 제목에는 숫자 접미사가 붙습니다(`slug-1`, `slug-2`, …).

### 생성되는 엣지 종류

- **CONTAINS** — 제목 계층. 1단계 제목 아래의 2단계 제목은 그 자식으로 기록됩니다.
- **REFERENCES** — 섹션 간 인라인 또는 참조 스타일 링크: `[text](./other.md#heading)` 또는 `[text](#local-heading)`. 소스는 포함하는 섹션, 대상은 `file::slug` 형식으로 해석됩니다.
- **IMPORTS_FROM** — 파일 간 링크. 링크나 디렉티브가 다른 Markdown 파일을 가리키면 현재 파일에서 대상으로 `IMPORTS_FROM` 엣지가 추가됩니다.
- **DEPENDS_ON** — 디렉티브 주석(아래 참조).

### 디렉티브 주석

디렉티브 주석은 문서 간 의존을 기계 가독 형식으로 표현하는 구조화 HTML 주석입니다:

```markdown
<!-- constrained-by ./decisions/adr-001.md#context -->
<!-- blocked-by ./specs/open-issue.md -->
<!-- supersedes ./old-api.md#endpoint-design -->
<!-- derived-from ./research/background.md#findings -->
```

지원 디렉티브 종류:

| 디렉티브 | 의미 |
|---|---|
| `constrained-by` | 이 섹션의 설계는 참조 문서/섹션에 제약됩니다 |
| `blocked-by` | 참조 항목이 해결될 때까지 구현이 막혀 있습니다 |
| `supersedes` | 이 문서가 참조 내용을 대체합니다 |
| `derived-from` | 이 섹션은 참조 소스에서 파생됩니다 |

각 디렉티브는 **DEPENDS_ON** 엣지가 됩니다. 엣지 속성 `markdown_directive_kind`에 구체적 디렉티브 종류가 기록됩니다.

### 문서 디렉티브(`dagayn:`)

<!-- derived-from ./docs/MARKDOWN-AUTHORING.md -->

`<!-- dagayn: implemented-by path::symbol -->` 형식의 HTML 주석은 Markdown 섹션에서 코드(또는 다른 산출물)로 `CROSS_ARTIFACT` 엣지를 만듭니다. 지원 종류에는 `implemented-by`, `discusses-artifact`, `raises-issue-for`가 있습니다. 코드 쪽에서는 `# dagayn: implements docs/spec.md#Section` 같은 줄 주석으로 반대 방향을 가리킬 수 있습니다.

전체 계약은 [`docs/MARKDOWN-AUTHORING.md`](docs/MARKDOWN-AUTHORING.md)를 참조하세요.

### 링크 해석

파서가 처리하는 링크 형식:

- `[text](./relative/path.md#section)` — 소스 파일 기준 상대 경로로 해석
- `[text](#local-section)` — 같은 파일의 섹션으로 해석
- `[ref]: path` — 참조 정의 형식
- 외부 URL(`http://`, `https://`, `mailto:`)은 무시

## 설치

```bash
pip install dagayn
```

지속적인 격리 CLI 환경에는 `uv tool install`도 쓸 수 있습니다:

```bash
uv tool install dagayn
```

일회성 격리 CLI에는 `uvx`가 편리합니다:

```bash
uvx --from dagayn dagayn --help
```

공개 휠에는 지원 대상용 컴파일된 확장이 포함되므로, 일반적인 PyPI 설치 경로에서는 Git 저장소에서 빌드할 필요가 없습니다.

격리 도구 설치를 선호한다면 `pipx`도 사용할 수 있습니다.

## 빠른 시작

```bash
dagayn install
dagayn build
dagayn status
```

`install`은 지원하는 AI 코딩 플랫폼을 자동 감지하여 적절한 위치에 MCP 설정을 작성합니다. 인자 없이 TTY에서 실행하면 임베딩 모드를 묻습니다(아래 참조). `-y` 또는 비 TTY stdin에서는 모드를 명시해야 합니다.

`build`는 초기 그래프를 생성합니다.

기존 그래프 데이터베이스를 지우고 처음부터 다시 만들려면 `dagayn build --force-full-build`(또는 `--force`)를 사용하세요.

`status`는 그래프 존재를 확인하고 기본 통계를 보고합니다.

### 설치 모드 선택

`dagayn install`은 다음 임베딩 전략을 일급 옵션으로 지원합니다:

```bash
# 1. FTS만 — 임베딩 없음, 가장 빠름, 모델 다운로드 없음.
dagayn install --mode fts-only

# 2. 로컬 — 관리되는 BGE-M3 llama.cpp GGUF 사이드카.
dagayn install --mode local-embedding

# 3. 관리되는 Qwen3 llama.cpp GGUF 사이드카.
dagayn install --mode local-embedding-llama --preset low    # Qwen3-Embedding-0.6B (~1 GB)

# 4. 원격 — OpenAI 호환 / Google / MiniMax 클라우드 임베딩.
dagayn install --mode remote-embedding --provider openai
dagayn install --mode remote-embedding --provider google
dagayn install --mode remote-embedding --provider minimax
```

`--mode remote-embedding`에서는 AI 코딩 도구를 띄우는 셸에 제공자의 환경 변수를 설정하세요(`openai`라면 `CRG_OPENAI_API_KEY`, `CRG_OPENAI_BASE_URL`, `CRG_OPENAI_MODEL`). MCP 서버는 시작 시 이를 상속하고, 생성되는 `dagayn serve --remote-embedding <provider>` 항목이 MCP 검색에 그 제공자를 쓰게 합니다. 정확한 환경 변수 목록은 설치 시 출력됩니다. 구 단축키(`--mode fts`, `--mode local`, `--mode local --preset low`, `--mode llama-qwen3`, `--mode remote`, `--local-embedding low`)는 새 명시적 모드 이름의 별칭으로 남아 있습니다.

### 네이티브 그래프 스토어

<!-- derived-from ./docs/USAGE.md#native-graph-store -->

그래프 스토어, 파서, FTS, 플로, 후처리는 네이티브 Rust 확장(`dagayn._core`)에서 돌아갑니다. 폴백할 Python 그래프 엔진은 없습니다. `DAGAYN_BACKEND=python`은 거부됩니다. 하이브리드 검색 랭킹과 manifest-bridge 추출은 Python에 남아 있습니다.

파서 대상은 Markdown, Terraform, Rust, Python/노트북, Bash, Go, Java, Ruby, C#, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, C / C 헤더 / Perl XS, C++, Objective-C, Elixir, GDScript, R, Julia, Perl, Vue, Svelte, Zig, PowerShell, 지원 스크립트 언어의 shebang 있는 확장자 없는 파일, 그리고 핵심 JavaScript / JSX / TypeScript / TSX / Astro입니다:

```bash
dagayn build
dagayn update
```

네이티브 확장이 없는 소스 체크아웃은 명확히 실패합니다.

## 자주 사용하는 CLI 흐름

```bash
dagayn build
dagayn update
dagayn watch
dagayn worktree sync
dagayn detect-changes --base HEAD~1
dagayn visualize --format graphml
dagayn serve
```

### MCP 도구 표면

<!-- derived-from ./docs/COMMANDS.md#mcp-tool-surface -->

`dagayn serve`는 컴팩트한 기본 워크플로 표면을 노출합니다. 주요 도구에 더해 `review_tool`, `flow_tool`, `architecture_analysis_tool` 같은 디스패처가 있어 일상 세션에서 이름 있는 서버 프로필이 필요 없습니다.

```bash
dagayn serve
dagayn serve --tools query_graph_tool,semantic_search_nodes_tool
```

`--tools`는 일부 공개 도구를 숨겨야 하는 배포를 위한 쉼표로 구분된 정확한 허용 목록입니다. 지속 서버 설정은 같은 제어에 `CRG_TOOLS`를 쓸 수 있습니다.

도구 응답은 보정된 guidance 계약을 사용합니다. 호환 필드(`status`, `summary`, `_hints`, `next_tool_suggestions`)는 남고, 리뷰·아키텍처·플로·리팩터·검색·쿼리 응답에는 `guidance`, `answerability`, `missingness`도 포함될 수 있습니다. guidance 항목은 `claim`, `evidence`, `confidence`, `missingness`, `action`, `reason_codes`, `counts`를 가져서 에이전트가 그래프 출력을 판결이 아니라 증거 순위 단서로 다루게 합니다. 상위 권장에는 `detail_level="minimal"`, 전체 뒷받침 섹션에는 `detail_level="standard"`를 쓰세요. `query_graph_tool`의 0건·미발견 응답에는 `zero_result_reason`, `next_action`, `result_count`, `results`, `answerability`, `missingness`가 포함됩니다. 부재는 소스나 테스트로 확인할 때까지 그래프 한정으로 다루세요. 문서 브리지 결과는 증거를 `authored`, `extracted`, `heuristic_reachable`로 라벨하여 Markdown 추적 가능성과 검증된 계약을 혼동하지 않게 합니다.

## 리포트와 내보내기

`dagayn visualize`는 정적 그래프 산출물을 내보냅니다.

- `--format`은 필수이며 `graphml`, `mermaid-c4`, `svg`, `cypher`, `obsidian`을 지원합니다
- `mermaid-c4`는 파일을 컴포넌트, 크로스파일 의존성을 관계로 집약한 Mermaid `C4Component` 코드를 출력합니다
- `svg`는 matplotlib를 사용하므로 필요하면 eval extra를 설치하세요: `pip install "dagayn[eval]"`
- Jupyter / Databricks 노트북은 리포트 출력 형식이 아니라 그래프 입력입니다

## AI 플랫폼 통합

`dagayn install`이 MCP를 설정할 수 있는 대상:

- Codex
- Claude / Claude Code
- Cursor
- Windsurf
- Zed
- Continue
- OpenCode
- Antigravity
- Qwen Code
- Kiro
- Qoder
- Pi
- Hermes Agent

`--platform <name>`으로 특정 플랫폼만 설치할 수 있습니다.
Codex의 경우 전역 `~/.codex/hooks.json`을 만들고 `~/.codex/config.toml`에서 hooks를 켜 세션 중 그래프를 갱신합니다. Claude hooks는 전역 `~/.claude/settings.json`에 기록됩니다. 설치된 git hooks는 커밋 전 검사에서 `dagayn update --skip-flows`를, 커밋 후 전체 `dagayn update`를 실행합니다. 로컬 임베딩 설치 모드를 고르면 생성되는 AI 도구 갱신 hooks도 같은 로컬 임베딩 사이드카 인자를 넘겨 편집 시 벡터를 유지합니다.

플랫폼별 지시 파일도 필요하면 설치됩니다:

- Claude는 `~/.claude/CLAUDE.md`
- Codex는 `~/.codex/AGENTS.md`
- OpenCode는 `~/.config/opencode/AGENTS.md`
- Qoder는 `QODER.md`
- `--platform qcoder`는 `qoder`의 별칭으로 받습니다

## 그래프 활용 방법

일반적인 리뷰 루프:

1. 그래프를 빌드하거나 업데이트
2. 최소 컨텍스트 또는 변경 리뷰 요청
3. 영향받은 파일과 심볼만 확인
4. 필요에 따라 커뮤니티, 흐름, 또는 파일 간 참조 추적
5. 편집 후 증분 새로 고침

그래프는 기본적으로 `.dagayn/` 하위에 로컬 저장됩니다. 외부 데이터베이스가 필요하지 않습니다.

## 의미 검색과 임베딩

<!-- derived-from ./docs/ARCHITECTURE.md#hybrid-search -->

`semantic_search_nodes`는 임베딩이 있으면 exact/name 검색과 임베딩 기반 퍼지 검색을 결합하고, 없으면 FTS만으로 폴백합니다. 어느 검색 경로가 기여했는지는 `search_mode`와 결과별 `source`로 보고합니다. 네이티브 FTS는 일본어를 Lindera IPADIC 형태소(사전 기본형 포함)와 겹치는 CJK bigram으로 나누므로 `検索する` 같은 활용형 쿼리도 `検索を行う`에 AND 매치합니다.

FTS 색인, RRF 병합, 리랭킹, 텍스트 모드, 제공자 설정 등 구현 세부사항은
[`docs/ARCHITECTURE.md#hybrid-search`](docs/ARCHITECTURE.md#hybrid-search)와
[`docs/LOCAL-EMBEDDINGS.md`](docs/LOCAL-EMBEDDINGS.md)를 참조하세요.

### 임베딩 모드와 제공자

| 모드/제공자 | 실행 위치 | 추가 설치 | 필요 환경 변수 |
|---|---|---|---|
| `--local-embedding` | 관리되는 localhost llama-server GGUF 사이드카 | — | — |
| `openai` | 클라우드 또는 셀프호스트 게이트웨이 | — | `CRG_OPENAI_API_KEY`, `CRG_OPENAI_BASE_URL`, `CRG_OPENAI_MODEL` |
| `google` | Google Cloud | `dagayn[google-embeddings]` | `GOOGLE_API_KEY` |
| `minimax` | MiniMax Cloud | — | `MINIMAX_API_KEY` |

`openai` 제공자는 표준 `/v1/embeddings` 스키마를 쓰므로 실제 OpenAI, Azure OpenAI, LiteLLM, vLLM, LocalAI, Ollama(OpenAI 모드) 등 유사 게이트웨이에서 동작합니다. `CRG_OPENAI_BASE_URL`이 localhost를 가리키면 클라우드 송신 경고는 자동으로 억제됩니다.

벡터 검색은 기본적으로 Rust 네이티브 코사인 유사도 백엔드를 사용합니다. 아키텍처별 SIMD(aarch64는 NEON, x86_64는 AVX와 SSE 폴백, 그 외는 스칼라)로 내적을 계산하므로 외부 BLAS나 Accelerate가 필요 없습니다. 네이티브 검색을 쓸 수 없을 때 Python 경로로 폴백하려면 `DAGAYN_EMBEDDING_SEARCH_BACKEND=auto`, A/B 테스트에는 `DAGAYN_EMBEDDING_SEARCH_BACKEND=python`을 설정하세요. Python 경로는 numpy가 있으면 선택적 BLAS matmul(`pip install "dagayn[numpy]"`), 없으면 순수 Python 코사인 루프입니다. numpy는 필수 하드 의존이 아닙니다.
`dagayn serve --local-embedding`은 관리되는 llama.cpp GGUF 사이드카로 BGE-M3를 돌려 가속을 Python 프로세스 밖에 둡니다. 옛 sentence-transformers/PyTorch `provider="local"` 모드는 삭제되었습니다. 로컬 임베딩은 이제 관리되는 llama-server 사이드카 또는 다른 localhost OpenAI 호환 엔드포인트를 뜻합니다.

### 임베딩 실행

MCP로 `embed_graph_tool`을 호출하거나, 에이전트가 `build_or_update_graph_tool` 뒤에 호출하게 하세요. 완전 로컬이면 `dagayn build --local-embedding`, `dagayn update --local-embedding`, `dagayn serve --local-embedding`을 우선하세요. 이들은 llama-server를 관리한 뒤 내부에서 OpenAI 호환 localhost 엔드포인트를 사용합니다. 이미 설정된 제공자를 쓸 때만 `provider`와 선택적으로 `model`을 넘깁니다.

```
dagayn build --local-embedding
embed_graph_tool(provider="openai")   # 환경의 CRG_OPENAI_* 를 읽음
embed_graph_tool(provider="google")   # 환경의 GOOGLE_API_KEY 를 읽음
embed_graph_tool(provider="minimax")  # 환경의 MINIMAX_API_KEY 를 읽음
```

임베딩은 `.dagayn/graph.db` 안의 `embeddings` 테이블에 저장됩니다. 제공자, 모델, 또는 `DAGAYN_EMBEDDING_TEXT_MODE`를 바꾸면 캐시가 분할되고 다음 호출에서 그 쌍의 재임베딩이 실행됩니다.

### 검색 품질

현재 검색 벤치마크는 20개 쿼리입니다. exact/name과 목적형 조회용 표준 쿼리 12개, 함수 행동에 대한 목적·프로세스 패턴 산문용 구조 쿼리 8개입니다.

| 검색 모드 | 쿼리 집합 | MRR | Hit@5 | Hit@20 |
|---|---|---:|---:|---:|
| `material` 텍스트 | 전체 (20) | 0.5528 | 14/20 | 18/20 |
| `narrative` 텍스트 | 전체 (20) | 0.6671 | 18/20 | 19/20 |
| intent-routed | 전체 (20) | **0.6725** | **18/20** | **19/20** |

8개의 구조 쿼리에서 `narrative`는 `material` 대비 MRR이 0.2881에서 0.5875, Hit@5가 3/8에서 7/8로 개선됩니다. 상세 벤치마크 표, 검색 모드 주석, 로컬 모델 비교는
[`docs/LOCAL-EMBEDDINGS.md#search-quality`](docs/LOCAL-EMBEDDINGS.md#search-quality)
를 참조하세요.

### 프라이버시와 클라우드 송신

클라우드 제공자에게 데이터를 보내기 전에 `dagayn`은 stderr로 경고를 출력하고 전송 내용(함수 이름, docstring, 파일 경로)을 나열합니다. 한 번 승인한 뒤 이후 경고를 끄려면:

```bash
export CRG_ACCEPT_CLOUD_EMBEDDINGS=1
```

완전 오프라인으로 유지하려면 `--local-embedding`을 써서 dagayn이 localhost llama-server 엔드포인트를 관리하게 하세요. Python ML 스택이나 PyTorch 의존은 필요 없습니다.

## 문서 지도

- `docs/USAGE.md` — 설치 및 일상 워크플로
- `docs/RECIPES.md` — watch, 레지스트리/데몬, 임베딩 복사-붙여넣기 레시피
- `docs/COMMANDS.md` — CLI, MCP 도구, 프롬프트, 내보내기 아티팩트
- `docs/FEATURES.md` — 포크의 중점 사항과 상위 프로젝트와의 차이점
- `docs/ARCHITECTURE.md` — 파서, 스토리지, 후처리 파이프라인
- `docs/SCHEMA.md` — 노드, 엣지, 메타데이터 모델
- `docs/MARKDOWN-AUTHORING.md` — 그래프 인식 Markdown 디렉티브와 `dagayn:` 링크
- `docs/SESSION-GRAPH-FRESHNESS.md` — session prepare, worktree, MCP 첫 도구 준비 상태
- `docs/EVALUATION-SEMANTICS.md` — 메트릭 역할, 프로필 요약, 게이트, 비용, 시맨틱 리포트 출력
- `docs/LOCAL-EMBEDDINGS.md` — 관리 사이드카와 로컬 임베딩 설정
- `docs/DAEMON-CONFIG.md` — 레지스트리와 워치 데몬 파일 형식
- `docs/TROUBLESHOOTING.md` — 실용적인 해결 방법
- `docs/LLM-OPTIMIZED-REFERENCE.md` — 기계 지향 참조 섹션

## 현재 개발 방향

이 포크가 현재 중점을 두는 사항:

- 인프라 인식 리뷰, 특히 Terraform
- 혼합 언어 모노레포
- 저장소 루트 기준의 안정적인 상대 경로 그래프 등록
- 터미널 및 에디터 에이전트를 위한 MCP 우선 워크플로
- 호스팅 서비스 없는 재현 가능한 로컬 분석

## 보안과 개인정보

`dagayn`은 로컬 그래프 저장을 중심으로 설계되었습니다. 일부 선택적 임베딩 제공자는 원격 API를 호출할 수 있지만, 이러한 흐름은 옵트인 방식이며 별도로 문서화됩니다.

자세한 내용은 `SECURITY.md`와 `docs/LEGAL.md`를 참조하세요.

## 기여

개발 설정, 검증 명령어, 기여 규칙은 `CONTRIBUTING.md`를 참조하세요.

## 라이선스

MIT. `LICENSE`를 참조하세요.
