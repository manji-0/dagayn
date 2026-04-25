# dagayn

> **DAG is All You Need** — 지식 그래프를 중심으로 한 코드 리뷰 및 영향 분석 접근법.

`dagayn`은 `code-review-graph`의 포크로, 특히 인프라 비중이 높은 코드베이스를 포함한 다중 언어 저장소를 위한 실용적인 AI 지원 리뷰에 특화되어 있습니다.

상위 프로젝트의 그래프 중심 리뷰 모델을 계승하면서 독립적인 제품으로 문서화 및 유지관리됩니다. 주요 차별점은 Terraform 일급 언어 지원, 포크 고유 파싱을 위한 커밋 고정 문법 가져오기, 더 광범위한 플랫폼 설치 흐름, 그리고 애플리케이션 코드·문서·인프라가 혼합된 모노레포에 대한 강화된 지원입니다.

## 주요 기능

`dagayn`은 저장소를 로컬 SQLite 지식 그래프로 파싱합니다. 파일, 심볼, 참조, 호출 엣지, 임포트, 테스트 링크, 커뮤니티, 실행 흐름을 기록합니다. AI 에이전트는 매 작업마다 전체 저장소를 다시 읽는 대신 이 그래프에 쿼리할 수 있습니다.

실제 효과:

- 더 작은 리뷰 컨텍스트 윈도우
- 빠른 영향 범위 분석
- 더 안전한 리팩토링
- 대형 저장소에서의 향상된 탐색
- 코드, 문서, 노트북, Terraform을 위한 단일 워크플로

## 포크 상태

`dagayn`은 명시적으로 `code-review-graph`의 포크입니다.

상위 문서를 규범으로 취급하지 않습니다. 이 저장소의 모든 프로젝트 가이드, 예시, 커맨드 설명은 `dagayn` 자체를 위해 작성되었습니다.

상위 프로젝트 귀속 및 원작자 정보는 [NOTICE](NOTICE)를 참조하세요.

## 주요 특징

- `.tf` 및 `.tfvars`의 일급 Terraform 파싱
- 지시어 주석을 포함한 Markdown 구조 및 의존성 추출
- `.ipynb` 노트북 파싱
- 증분 그래프 업데이트 및 감시 모드
- AI 코딩 도구용 MCP 서버
- 영향 반경, 리뷰 컨텍스트, 커뮤니티, 흐름, 리팩토링을 위한 그래프 쿼리
- 다중 저장소 레지스트리 및 데몬 워크플로
- 대화형 시각화 및 GraphML / SVG / Cypher / Obsidian 내보내기

## 지원 언어 및 파일 유형

주류 애플리케이션 언어와 저장소 관련 형식을 포함합니다.

주요 항목:

- Python, JavaScript, TypeScript, TSX, Go, Rust, Java, C#, Ruby, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, Objective-C, Bash, Elixir, Zig, PowerShell, Julia, GDScript, Vue, Svelte, Astro, ReScript
- Markdown
- Jupyter 노트북 및 Databricks 스타일 노트북 내보내기
- Terraform

현재 커버리지 요약은 `docs/FEATURES.md`와 `docs/LLM-OPTIMIZED-REFERENCE.md`를 참조하세요.

## Terraform 지원

`dagayn`은 Terraform을 애플리케이션 코드와 동등한 일급 언어로 취급합니다. 전용 Tree-sitter 문법으로 `.tf`와 `.tfvars` 파일을 파싱합니다.

### 파싱되는 블록 유형

| 블록 | 한정 이름 패턴 | 그래프 종류 |
|---|---|---|
| `resource "type" "name"` | `resource.type.name` | Class |
| `data "type" "name"` | `data.type.name` | Class |
| `variable "name"` | `var.name` | Function |
| `locals { key = … }` | `local.key`（속성별） | Function |
| `output "name"` | `output.name` | Function |
| `module "name"` | `module.name` | Class |
| `provider "name"` | `provider.name` | Class |
| `terraform {}` | `terraform` | Class |
| `check "name"` | `check.name` | Test |
| `ephemeral "type" "name"` | `ephemeral.type.name` | Class |
| `import {}` | 엣지만 생성 | — |
| `moved {}` | 엣지만 생성 | — |
| `removed {}` | 엣지만 생성 | — |

### 생성되는 엣지 유형

- **REFERENCES** — 블록 본문 내의 `var.x`, `local.x`, `module.x`, `output.x`, `provider.x`, `data.type.name`, `resource_type.name` 표현식. 전용 정규식으로 추출하며 Terraform 내장 접두사(`count`, `each`, `path`, `self`, `terraform`)는 건너뜁니다.
- **CALLS** — `merge(…)`나 `length(…)` 등의 내장 함수 호출.
- **IMPORTS_FROM** — `module` 블록과 `terraform required_providers`의 `source` 속성, 그리고 `import` 블록의 대상.
- **CONTAINS** — 파일과 그 안에 정의된 각 블록 간의 포함 관계.
- **DEPENDS_ON** — `terraform` 블록의 `required_providers` 버전 제약.

### 크로스 모듈 분석

`module` 블록의 `source`가 로컬 경로를 참조하는 경우, `dagayn`은 호출 모듈에서 대상 디렉토리까지 `IMPORTS_FROM` 엣지를 기록합니다. 이를 통해 영향 반경 쿼리가 모듈 경계를 넘을 수 있습니다.

### `.tfvars` 파일

변수 값 파일(`.tfvars`)은 Terraform으로 파싱됩니다. 최상위 속성 할당은 `var.name` 노드가 되며, REFERENCES 엣지를 통해 `.tf` 파일의 대응하는 `variable` 블록에 연결됩니다. 이를 통해 변수 데이터 흐름의 완전한 그림이 그래프에 나타납니다.

## Markdown 지원

`dagayn`은 소스 코드와 함께 Markdown 문서에서 그래프 노드와 엣지를 추출합니다. 산문 형태의 아키텍처 결정과 그것이 설명하는 코드가 동일한 그래프에 나타납니다.

### 파싱되는 노드 유형

| 요소 | 한정 이름 패턴 | 그래프 종류 |
|---|---|---|
| 문서 | 파일 경로 | File |
| `# 제목` ～ `###### 제목` | `file::slug` | Class |
| Setext H1 / H2（밑줄 형식） | `file::slug` | Class |

제목 슬러그는 GitHub Markdown 관례를 따릅니다: 소문자화, 공백과 하이픈을 `-`로 통일, 비영숫자 문자 제거. 동일 파일 내 중복 제목은 숫자 접미사를 붙입니다(`slug-1`, `slug-2`, …).

### 생성되는 엣지 유형

- **CONTAINS** — 제목 계층 구조. 1단계 제목 아래에 나타나는 2단계 제목은 그 자식으로 기록됩니다.
- **REFERENCES** — 섹션 간의 인라인 또는 참조 스타일 링크: `[text](./other.md#heading)` 또는 `[text](#local-heading)`. 소스는 포함 섹션이며 대상은 `file::slug` 형식으로 해석됩니다.
- **IMPORTS_FROM** — 파일 간 링크. 링크나 지시어가 다른 Markdown 파일을 가리킬 때, 현재 파일에서 대상으로 `IMPORTS_FROM` 엣지가 추가됩니다.
- **DEPENDS_ON** — 지시어 주석（아래 참조）.

### 지시어 주석

지시어 주석은 문서 간 의존 관계를 기계가 읽을 수 있는 형식으로 표현하는 구조화된 HTML 주석입니다:

```markdown
<!-- constrained-by ./decisions/adr-001.md#context -->
<!-- blocked-by ./specs/open-issue.md -->
<!-- supersedes ./old-api.md#endpoint-design -->
<!-- derived-from ./research/background.md#findings -->
```

지원하는 지시어 종류:

| 지시어 | 의미 |
|---|---|
| `constrained-by` | 이 섹션의 설계는 참조된 문서/섹션에 의해 제약됨 |
| `blocked-by` | 참조된 항목이 해결될 때까지 구현이 차단됨 |
| `supersedes` | 이 문서는 참조된 내용을 대체함 |
| `derived-from` | 이 섹션은 참조된 출처에서 파생됨 |

각 지시어는 **DEPENDS_ON** 엣지가 됩니다. 엣지 속성 `markdown_directive_kind`에 구체적인 지시어 종류가 기록됩니다.

### 링크 해석

파서가 처리하는 링크 형식:

- `[text](./relative/path.md#section)` — 소스 파일 기준 상대 경로로 해석
- `[text](#local-section)` — 동일 파일의 섹션으로 해석
- `[ref]: path` — 참조 정의 형식
- 외부 URL(`http://`, `https://`, `mailto:`)은 무시

## 설치

```bash
pip install git+https://github.com/manji-0/dagayn.git
```

격리된 도구 설치를 선호한다면 `pipx`도 사용 가능합니다.

## 빠른 시작

```bash
dagayn install
dagayn build
dagayn status
```

`install`은 지원하는 AI 코딩 플랫폼을 자동 감지하여 적절한 위치에 MCP 설정을 작성합니다.

`build`는 초기 그래프를 생성합니다.

`status`는 그래프 존재를 확인하고 기본 통계를 보고합니다.

## 자주 사용하는 CLI 흐름

```bash
dagayn build
dagayn update
dagayn watch
dagayn detect-changes --base HEAD~1
dagayn visualize --serve
dagayn serve
```

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

`--platform <name>`으로 특정 플랫폼만 설치할 수 있습니다.

## 그래프 활용 방법

일반적인 리뷰 루프:

1. 그래프를 빌드하거나 업데이트
2. 최소 컨텍스트 또는 변경 리뷰 요청
3. 영향받은 파일과 심볼만 확인
4. 필요에 따라 커뮤니티, 흐름, 또는 파일 간 참조 추적
5. 편집 후 증분 새로 고침

그래프는 기본적으로 `.dagayn/` 하위에 로컬 저장됩니다. 외부 데이터베이스가 필요하지 않습니다.

## 문서 지도

- `docs/USAGE.md` — 설치 및 일상 워크플로
- `docs/COMMANDS.md` — CLI, MCP 도구, 프롬프트, 내보내기 아티팩트
- `docs/FEATURES.md` — 포크의 중점 사항과 상위 프로젝트와의 차이점
- `docs/architecture.md` — 파서, 스토리지, 후처리 파이프라인
- `docs/schema.md` — 노드, 엣지, 메타데이터 모델
- `docs/TROUBLESHOOTING.md` — 실용적인 해결 방법
- `docs/LLM-OPTIMIZED-REFERENCE.md` — 기계 지향 참조 섹션

## 현재 개발 방향

이 포크가 현재 중점을 두는 사항:

- 인프라 인식 리뷰, 특히 Terraform
- 혼합 언어 모노레포
- 저장소 루트 기준의 안정적인 상대 경로 그래프 등록
- 터미널 및 에디터 에이전트를 위한 MCP 우선 워크플로
- 호스팅 서비스 없는 재현 가능한 로컬 분석

## 보안 및 개인정보

`dagayn`은 로컬 그래프 저장을 중심으로 설계되었습니다. 일부 선택적 임베딩 제공자는 원격 API를 호출할 수 있지만, 이러한 흐름은 옵트인 방식이며 별도로 문서화됩니다.

자세한 내용은 `SECURITY.md`와 `docs/LEGAL.md`를 참조하세요.

## 기여

개발 설정, 검증 명령어, 기여 규칙은 `CONTRIBUTING.md`를 참조하세요.

## 라이선스

MIT. `LICENSE`를 참조하세요.
