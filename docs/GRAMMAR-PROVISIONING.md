# Grammar provisioning

<!-- constrained-by ./ARCHITECTURE.md -->

## Purpose

dagayn uses fork-specific Tree-sitter grammars for language support that is more opinionated than the generic upstream language-pack path.

The current provisioned grammars are:

- Markdown
- Terraform
- Rust
- Python
- JavaScript
- TypeScript
- TSX
- Bash
- Go
- Java
- Ruby
- C#
- PHP
- Kotlin
- Scala
- Solidity
- Dart
- Lua
- Luau
- C
- C++
- Objective-C
- Elixir
- GDScript
- R
- Julia
- Perl
- Vue
- Svelte
- Zig
- PowerShell
- Swift
- ReScript

## Provisioning model

Grammar source trees are **not** stored as tracked vendor directories in this repository.

Instead, dagayn:

1. pins exact upstream commits for the forked grammar repositories
2. downloads the grammar source archive on demand
3. stores the fetched source under a local cache directory
4. builds the parser binding from that cached source when needed

This applies to:

- local runtime parser initialization
- test runs
- package builds
- CI

For maturin builds, the Rust grammar build script also stages the required
grammar files under `dagayn/_vendor_grammars/` before wheel/sdist assembly.
Those generated staging files are ignored by git, but they are included in
published artifacts so Python and Rust parser paths use the same pinned grammar
sources after installation.

## Cache behavior

The default grammar cache lives under the user cache directory for the current platform.

An explicit override is supported with:

```bash
DAGAYN_GRAMMAR_CACHE_DIR=/custom/cache/path
```

The cache key includes the pinned commit, so changing the pin yields a separate cached tree.

## Pinned source contract

Each grammar pin must identify:

- repository owner and name
- exact commit SHA
- required source files
- any fork-local assets that must be injected before binding compilation

The provisioner injects a small Python binding shim where the pinned source
tree does not provide the exact binding layout dagayn expects.

The Rust backend currently routes Markdown, Terraform, Rust, Python/notebooks,
JavaScript/JSX, TypeScript/TSX, Astro, Bash, Go, Java, Ruby, C#, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, C, C++, Objective-C, Elixir, GDScript, R, Julia, Perl, Vue, Svelte, Zig, PowerShell, and ReScript through these pinned grammar sources.

## Operational expectations

- builds should remain reproducible because the grammar revision is pinned
- CI should be able to prefetch grammars explicitly
- parser initialization may trigger fetch/build the first time a pinned grammar is needed

## Related design concerns

- cache invalidation must be commit-based, not mutable-branch-based
- docs should describe dagayn behavior, not assume upstream code-review-graph vendor layout
- user-facing behavior should continue to work with repo-root-relative graph paths after grammar loading
