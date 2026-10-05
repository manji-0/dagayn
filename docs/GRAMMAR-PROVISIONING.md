# Grammar provisioning

<!-- constrained-by ./ARCHITECTURE.md -->

## Purpose

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->

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
- Dart
- Lua
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
- Swift

## Provisioning model

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->

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

## Building a subset of grammars

<!-- derived-from #provisioning-model -->

Each grammar is a Cargo feature, as in ast-grep's language crate:
`dagayn-grammars` has one `lang-<language>` feature per grammar it compiles
(`lang-markdown`, `lang-terraform`, `lang-rust`, `lang-python`,
`lang-javascript`, `lang-typescript`, `lang-tsx`, `lang-bash`, `lang-go`,
`lang-java`, `lang-ruby`, `lang-csharp`, `lang-php`, `lang-kotlin`,
`lang-scala`, `lang-dart`, `lang-lua`, `lang-c`, `lang-cpp`, `lang-objc`,
`lang-elixir`, `lang-gdscript`, `lang-r`, `lang-julia`, `lang-perl`,
`lang-vue`, `lang-svelte`, `lang-zig`, `lang-swift`). `all-languages` turns
on every one and is the default, so a plain `cargo build`, `uv sync`, and
every published wheel keep all languages and produce the same graph as
before. Markdown and Terraform are features too, but stay in the default.

`dagayn-parser`, `dagayn-build`, `dagayn-cli`, and `dagayn-py` forward the
same `lang-*` names; `dagayn-graph`, `dagayn-postproc`, and `dagayn-tools`
forward only `all-languages`. In `dagayn-parser`, `lang-vue` and
`lang-svelte` also enable `lang-javascript` and `lang-typescript`, which parse
their script blocks. To build or test a subset, turn the default off:

```bash
cargo check -p dagayn-parser --no-default-features
cargo test -p dagayn-parser --no-default-features --features lang-python,lang-rust
cargo build -p dagayn-cli --no-default-features --features lang-go,lang-markdown
```

The build script compiles only the enabled grammars (it reads the
`CARGO_FEATURE_LANG_*` variables). Bash's scanner includes headers staged
with the JavaScript grammar, so `lang-bash` alone still stages, but does not
compile, the JavaScript source. Tests for a disabled language are compiled
out; the examples need `all-languages`.

A file whose language is disabled is still Rust-owned: it gets its File node
(as a PowerShell file always does) and no symbols, rather than a parse error,
a missing file, or the Python fallback parser. This goes by the file's kind,
so a `.tf.json` file is File-only without `lang-terraform` although its JSON
needs no grammar. A file whose own grammar is on but which embeds a disabled
one keeps what its grammar finds: a marimo Markdown file without
`lang-python` gets its sections but no Python symbols. A graph built that way keeps
those File-only entries until a full build re-parses them
(`dagayn build --force-full-build`), so use subsets for development and CI,
not for a graph you keep.

On an 18-core Apple Silicon machine, a clean `cargo build --release -p
dagayn-parser` took 20 s wall / 74 s CPU with every grammar, and 16 s wall /
31 s CPU with none (17 s / 34 s with `lang-python,lang-rust`). The 29
compiled grammar archives total about 51 MB; Python and Rust account for
1.6 MB of them.

## Cache behavior

<!-- derived-from ./ARCHITECTURE.md#storage-model -->

The default grammar cache lives under the user cache directory for the current platform.

An explicit override is supported with:

```bash
DAGAYN_GRAMMAR_CACHE_DIR=/custom/cache/path
```

The cache key includes the pinned commit, so changing the pin yields a separate cached tree.

## Pinned source contract

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->

Each grammar pin must identify:

- repository owner and name
- exact commit SHA
- required source files
- any fork-local assets that must be injected before binding compilation

The provisioner injects a small Python binding shim where the pinned source
tree does not provide the exact binding layout dagayn expects.

The Rust backend currently routes Markdown, Terraform, Rust, Python/notebooks,
JavaScript/JSX, TypeScript/TSX, Astro, Bash, Go, Java, Ruby, C#, PHP, Kotlin, Swift, Scala, Dart, Lua, C, C++, Objective-C, Elixir, GDScript, R, Julia, Perl, Vue, Svelte, Zig, and PowerShell through these pinned grammar sources.

## Operational expectations

<!-- derived-from ./ARCHITECTURE.md#storage-model -->

- builds should remain reproducible because the grammar revision is pinned
- CI should be able to prefetch grammars explicitly
- parser initialization may trigger fetch/build the first time a pinned grammar is needed

## Related design concerns

<!-- derived-from ./ARCHITECTURE.md#storage-model -->

- cache invalidation must be commit-based, not mutable-branch-based
- docs should describe dagayn behavior, not assume upstream code-review-graph vendor layout
- user-facing behavior should continue to work with repo-root-relative graph paths after grammar loading
