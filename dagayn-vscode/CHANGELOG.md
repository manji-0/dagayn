# Changelog

## 0.2.2 — Unreleased

### Added

- **Saved custom queries**: new commands `Code Graph: Save Custom Query` and `Code Graph: Run Saved Query` persist `{label, pattern, target}` triples to `.dagayn/queries.json` and rerun them from the QuickPick. Includes an inline delete flow for stale queries.
- **Multi-root workspace support**: one graph reader per workspace folder, folder grouping in the tree and stats views, per-folder lifecycle commands, and active-editor-aware cursor commands.
- Folder picker for global commands (`Build Graph`, `Update Graph`, `Embed Graph`, `Watch Graph`, `Review Changes`) when multiple workspace folders are open.
- **Module dependency view**: new command `Code Graph: Show Module Dependencies` aggregates files by parent directory and renders directory-level dependency edges.
- **Saved custom queries**: new commands `Code Graph: Save Custom Query` and `Code Graph: Run Saved Query` persist `{label, pattern, target}` queries to `.dagayn/queries.json`.
- **Node documentation**: hover over tree symbols to see docstrings, or open a dedicated documentation panel with `Code Graph: Show Node Documentation`.
- Auto-update failure notification: consecutive failures now surface a warning after `dagayn.autoUpdateFailureThreshold` consecutive failures with "Open Settings" and "Disable Auto-Update" actions.
- New setting `dagayn.autoUpdateFailureThreshold` (default `3`) to control when the warning appears.

### Fixed

- `GraphWebview.openFileAtLine` no longer duplicates the workspace root when the stored file path is already absolute.
- Multi-root relative path resolution in `openFileAtLine` now correctly opens files relative to the longest-matching workspace folder and surfaces actionable diagnostics with a "Copy Path" action.
- Webview message handlers are wrapped in a single `try/catch` so malformed payloads cannot crash the extension host.
- Full-symbol graph loading is now bounded by `dagayn.graph.maxNodes` at the reader level, preventing large databases from freezing the extension host.

### Changed

- Symbol graph node ordering when truncated is now deterministic by `id`; previously it followed file enumeration order.

## 0.2.1 — 2026-04-08

### Fixed

- Compatible with Python backend schema v6 (no extension-side schema changes in this release)

## 0.2.0 — 2026-03-20

### Added

- **Query Graph** command with 8 query patterns (callers_of, callees_of, imports_of, etc.)
- **Find Callees** command to trace all functions called by a target
- **Find Large Functions** command to identify oversized functions/classes
- **Compute Embeddings** command to generate vector embeddings
- **Watch Mode** command for continuous graph updates
- Cursor-aware resolution for blast radius and navigation commands
- Fuzzy fallback search when exact node matches fail
- SCM decorations for git-aware file status

### Changed

- Updated README with complete command table (13 commands)
- All 13 commands now documented

## 0.1.1 — 2026-03-17

### Fixed

- CLI path setting scoped to `machine` level (security fix)
- Secure nonce generation using `crypto.randomBytes()`

## 0.1.0 — 2026-03-17

Initial release.

- Code Graph tree view with file, class, function, type, and test nodes
- Interactive D3.js graph visualisation in a webview panel
- Blast radius analysis from cursor position
- Find callers and find tests commands
- Search across all graph nodes
- Review changes with git-aware impact analysis
- Auto-update graph on file save
- CLI auto-detection and guided installation
- Getting Started walkthrough
