# dagayn

`dagayn`, `code-review-graph` का एक fork है। इस repository में official नाम `dagayn` माना जाता है।

## यह क्या करता है

- repository को local knowledge graph में बदलता है
- change impact analysis देता है
- MCP के ज़रिये AI tools को छोटा और relevant context देता है
- app code, Markdown और Terraform वाले monorepo को संभालता है

## Quick start

```bash
pip install dagayn
dagayn install
dagayn build
dagayn status
```

## इस fork की खास बातें

- Terraform first-class support
- Markdown structure और directive dependency extraction
- repo root relative paths पर आधारित graph registration
- `ruff` और `ty` आधारित CI

ज़्यादा जानकारी के लिए `README.md` और `docs/` देखें।
