# Parity fixtures

These directories are the canonical baseline for the Rust core migration (Phase 0).

Each subdirectory is a minimal fixture repository targeting a specific language or
cross-artifact scenario. The corresponding snapshots in `__snapshots__/` are
committed and must not diverge from what `dagayn build` + `tools/parity_export.py`
produces on the Python path.

**Do not edit fixture files casually.** Any change invalidates the committed
snapshot. To regenerate after an intentional change, run:

```bash
dagayn build --repo-dir tests/fixtures/parity/<name>
uv run python tools/parity_export.py tests/fixtures/parity/<name> \
  --out tests/fixtures/parity/__snapshots__/<name>.json
```

## Fixtures

| Directory | Purpose |
|---|---|
| `python_only/` | Python imports + calls + class hierarchy |
| `terraform_only/` | Terraform blocks, `var.*` references, outputs |
| `markdown_only/` | Markdown headings + `derived-from` directive edge |
| `notebook/` | Jupyter notebook with multiple code cells (cell attribution) |
| `mixed/` | Python + Terraform + Markdown in one repo (cross-artifact edges) |

## Acceptance criterion (Phase 0)

Running `parity_export.py --check-determinism` against any of these fixtures
must exit 0. The `test_parity_export.py` test suite verifies that two independent
builds of the same fixture produce byte-identical exports.
