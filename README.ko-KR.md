# dagayn

`dagayn`은 `code-review-graph`의 포크입니다. 호환용 명령은 남아 있지만, 이 저장소의 문서는 포크 자체인 `dagayn`을 기준으로 작성됩니다.

## 핵심 기능

- 저장소를 로컬 지식 그래프로 구축
- 변경 영향 범위 분석
- MCP를 통해 AI 도구에 더 작은 문맥 제공
- 애플리케이션 코드, Markdown, Terraform이 섞인 모노레포 분석

## 빠른 시작

```bash
pip install dagayn
dagayn install
dagayn build
dagayn status
```

## 이 포크의 초점

- Terraform 1급 지원
- Markdown 구조 및 지시형 의존성 추출
- 저장소 루트 상대 경로 기반 그래프 등록
- `ruff` / `ty` 중심 CI

자세한 내용은 `README.md` 와 `docs/`를 참고하세요.
