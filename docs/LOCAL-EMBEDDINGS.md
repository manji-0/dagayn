# Local Embeddings

<!-- constrained-by ./COMMANDS.md -->

This document describes the **`--mode local`** install path: dagayn generates
semantic-search embeddings locally during `build` and `update` by starting
an OpenAI-compatible local model server as a subprocess and talking to its
`/v1/embeddings` endpoint.  The other two install modes are `--mode fts`
(no embeddings, fastest) and `--mode remote` (OpenAI-compatible / Google /
MiniMax cloud APIs) — see the README's "Choosing an install mode" section.

## Presets

| Preset | Runtime | Default platform | Model | Quantization | Dimension |
| --- | --- | --- | --- | --- | --- |
| `low` | `mlx-openai-server` | macOS Apple Silicon | `mlx-community/Qwen3-Embedding-0.6B-mxfp8` | MXFP8 | 1024 |
| `low` | `llama-server` | Linux, Windows, macOS Intel | `Qwen/Qwen3-Embedding-0.6B-GGUF` | `Q8_0` | 1024 |

The Qwen3 Embedding series is published as `0.6B`, `4B`, and `8B` embedding
models. The local preset uses the 0.6B 8-bit variant. On macOS Apple Silicon,
dagayn defaults to the MLX conversion; other platforms default to the GGUF
runtime. Set `DAGAYN_LOCAL_EMBEDDING_RUNTIME=llama` or
`DAGAYN_LOCAL_EMBEDDING_RUNTIME=mlx` to force a runtime.

## Setup

On macOS Apple Silicon, install `mlx-openai-server` so the
`mlx-openai-server` command is available on `PATH`:

```bash
uv tool install mlx-openai-server
mlx-openai-server --help
```

On other platforms, install `llama.cpp` so `llama-server` is available on
`PATH`:

```bash
brew install llama.cpp
llama-server --version
```

If the packaged version is too old for `-hf` or `--embedding`, build
`llama-server` from source:

```bash
git clone https://github.com/ggml-org/llama.cpp
cd llama.cpp
cmake -B build
cmake --build build -j --target llama-server
./build/bin/llama-server --version
```

## Manual Check

<!-- derived-from #presets -->

You can start the server yourself before running dagayn. dagayn will reuse a
compatible server already listening on the configured port.

For the macOS Apple Silicon MLX runtime:

```bash
mlx-openai-server launch \
  --model-type embeddings \
  --model-path mlx-community/Qwen3-Embedding-0.6B-mxfp8 \
  --host 127.0.0.1 \
  --port 18080 \
  --served-model-name mlx-community/Qwen3-Embedding-0.6B-mxfp8
```

Then verify the OpenAI-compatible endpoint:

```bash
curl http://127.0.0.1:18080/v1/embeddings \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer dagayn-local' \
  -d '{"model":"mlx-community/Qwen3-Embedding-0.6B-mxfp8","input":["dagayn local embedding test"]}'
```

For the llama.cpp GGUF runtime:

```bash
llama-server \
  -hf Qwen/Qwen3-Embedding-0.6B-GGUF:Q8_0 \
  --embedding \
  --pooling last \
  --flash-attn \
  --cache-type-k f16 \
  --cache-type-v f16 \
  -b 8192 \
  -ub 8192 \
  --host 127.0.0.1 \
  --port 18080 \
  --alias qwen3-embedding-0.6b-gguf-q8_0
```

Verify the GGUF endpoint:

```bash
curl http://127.0.0.1:18080/v1/embeddings \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer dagayn-local' \
  -d '{"model":"qwen3-embedding-0.6b-gguf-q8_0","input":["dagayn local embedding test"]}'
```

## Build And Update

<!-- derived-from #setup -->

Run a full build with local embeddings:

```bash
dagayn build --local-embedding low
```

Run an incremental update and refresh missing or stale embeddings:

```bash
dagayn update --local-embedding low
```

If no compatible server is already listening on `127.0.0.1:18080`, dagayn
starts the platform default local server for the duration of the command. By
default, dagayn stops that subprocess when embedding finishes.

Useful options:

```bash
dagayn build \
  --local-embedding low \
  --local-embedding-port 18080 \
  --local-embedding-bin auto \
  --local-embedding-timeout 300 \
  --local-embedding-request-timeout 60 \
  --local-embedding-batch-size 1
```

`--local-embedding-bin auto` resolves to `mlx-openai-server` on macOS Apple
Silicon and `llama-server` elsewhere. Pass an explicit executable name or path
to override it.

`--local-embedding-timeout` only controls server readiness. If an individual
embedding batch stalls after the server is ready,
`--local-embedding-request-timeout` bounds that HTTP request. Successful
batches are saved as they complete, so rerunning the command resumes from the
remaining stale or missing embeddings.

The batch size is also pinned for local embeddings. `dagayn build
--local-embedding low` defaults to 1 text per request even if the shell has
`CRG_OPENAI_BATCH_SIZE` set for another provider. Raise it only after measuring;
larger batches can make local embedding endpoints stall on some hosts.

For the MLX runtime, `mlx-openai-server` currently exposes most throughput and
KV-cache flags only for language or multimodal generation, not for `embeddings`.
dagayn therefore leaves the MLX server command minimal and tunes the embedding
request instead: the managed MLX preset sends `max_length=2048` to avoid the
server's 512-token embedding default truncating longer dagayn metadata or
documentation-body inputs. The max-length setting is part of the persisted
OpenAI-compatible provider identity, so changing it triggers a clean re-embed
rather than mixing vectors produced with different truncation limits.

The MLX preset uses the `mxfp8` conversion rather than the older generic
`8bit` MLX repository. `mxfp8` is an 8-bit MLX floating-point quantization mode,
which is the closer match to the llama.cpp `Q8_0` GGUF preset when we want the
two runtimes to have comparable precision. If you need to produce a local copy
yourself, use the `mlx-embeddings` conversion tool with `--quantize --q-mode
mxfp8`; dagayn's managed preset points at the published
`mlx-community/Qwen3-Embedding-0.6B-mxfp8` model directly.

For the llama.cpp runtime, dagayn starts `llama-server` with Flash Attention
enabled and keeps both KV cache tensors at `f16`. Flash Attention improves
prompt/embedding throughput on supported backends without changing the model
weights. The KV cache type is intentionally not quantized by the preset:
`q8_0`, `q4_0`, and similar cache types can reduce memory pressure, but they
also add another approximation layer to attention state. Since the local preset
already uses an 8-bit GGUF model, keeping K and V at `f16` is the conservative
quality default. The logical and physical llama.cpp batch limits are both set to
8192 (`-b 8192 -ub 8192`) so long embedding inputs are processed in larger
chunks when memory allows.

The local preset embeds graph metadata: symbol name, qualified name, file path,
display name, signature, params, return type, kind, parent, and language.
Markdown `DocSection`/`DocBody` nodes also include a bounded section body so
fuzzy documentation search can match prose that does not appear in headings.
Documentation bodies are repeated in the embedding input by default
(`DAGAYN_DOC_EMBEDDING_BODY_WEIGHT=2`) so prose has more influence than path
and heading metadata for fuzzy documentation queries.

Set `DAGAYN_EMBEDDING_TEXT_MODE=metadata` or `body` to override that behavior
for any provider or preset. The source span is capped by
`DAGAYN_EMBEDDING_SOURCE_CHARS` and defaults to 2048 characters. Body mode can
improve conceptual searches where the query terms appear only in implementation
text or Markdown section bodies, at the cost of larger embedding inputs and
more frequent re-embedding when function bodies or documentation sections
change.

Leave a dagayn-started server running for reuse:

```bash
dagayn build --local-embedding low --keep-local-embedding-server
```

## Search quality

The measurements below were taken with the llama.cpp GGUF runtime before the
macOS Apple Silicon default moved to MLX. They remain a baseline for the `low`
preset, but MLX-specific timings should be re-measured on Apple Silicon.

Measured on the dagayn codebase (6,197 graph nodes, 5,811 embedded non-file
nodes). Query set: 5 exact function names, 3 PascalCase class names, 4
conceptual natural-language queries. The local `low` preset embeds graph
metadata.

### Aggregate

| Mode | text embedded | build time | mean MRR | Precision@1 | Precision@5 | avg query latency |
|---|---|---:|---:|---:|---:|---:|
| FTS5 only | n/a | n/a | 0.7417 | 0.6667 | 0.9167 | 1.0 ms |
| Qwen3-Embedding-0.6B Q8 `low` (hybrid) | metadata | 133.2 s | **0.7222** | 0.5833 | 0.9167 | 413.2 ms |

### Per-query breakdown

| Query | Label | FTS rank | Qwen3-0.6B `low` rank |
|---|---|---|---|
| `hybrid_search` | exact_name | 5 | 2 |
| `rebuild_fts_index` | exact_name | 5 | 3 |
| `rrf_merge` | exact_name | 1 | 1 |
| `full_build` | exact_name | 2 | 3 |
| `detect_query_kind_boost` | exact_name | 1 | 1 |
| `GraphStore` | pascal_case_class | 1 | 2 |
| `EmbeddingProvider` | pascal_case_class | 1 | 1 |
| `LocalEmbeddingProvider` | pascal_case_class | 1 | 1 |
| "reciprocal rank fusion" | conceptual | **1** | **1** |
| "sentence transformers local model" | conceptual | **1** | **1** |
| "kind boost detection query" | conceptual | 1 | 1 |
| "incremental graph construction" | conceptual | — | — |

FTS5 is strong on exact and PascalCase queries. The previous 4B `high`
experiment did not beat `low` on this query set despite embedding source bodies,
and it was about 7x slower to embed. The local preset surface therefore keeps
only `low`. The last query ("incremental graph construction" → `full_build`) is
a miss for all modes because the target shares little lexical or semantic
surface with the query.

### Documentation fuzzy search

Measured on the dagayn documentation corpus (`README.md` plus `docs/`, excluding
audit/plan notes) after adding `DocBody` paragraph chunks. Query set: 19 complex
natural-language documentation questions whose full text does not appear in the
target body. Relevance is graded: each query has a primary target and optional
related sections.

| Mode | mean MRR | Precision@1 | Precision@5 | Precision@20 | nDCG@5 | nDCG@20 | avg query latency |
|---|---:|---:|---:|---:|---:|---:|---:|
| FTS5 only | 0.4146 | 0.3158 | 0.4737 | **0.8421** | 0.4361 | **0.7344** | 8.8 ms |
| Qwen3-Embedding-0.6B Q8 `low` | **0.5449** | **0.4737** | **0.5789** | **0.8421** | **0.4424** | 0.7220 | 501.2 ms |
| Qwen3-Embedding-0.6B Q8 `low` with documentation query prefix | 0.2545 | 0.1579 | 0.3684 | 0.6316 | 0.1551 | 0.2970 | 494.6 ms |

The raw local embedding query improves early ranking on this harder prose
retrieval set and ties FTS5 at Precision@20. FTS5 still has slightly higher
nDCG@20, so broad graded recall remains competitive. The generic
documentation-query prefix did not help; it added noise for this model and
corpus.

## Troubleshooting

<!-- derived-from #manual-check -->

- `Could not find 'mlx-openai-server'`: install `mlx-openai-server` or pass
  `--local-embedding-bin /path/to/mlx-openai-server`.
- `Could not find 'llama-server'`: install `llama.cpp` or pass
  `--local-embedding-bin /path/to/llama-server`.
- Port already in use: either stop the process on that port or use
  `--local-embedding-port`.
- Timeout while starting: the first run may download the MLX or GGUF model from
  Hugging Face; increase `--local-embedding-timeout` or run the local server
  manually to watch progress.
- Incompatible endpoint: dagayn found something on the port, but it did not
  return an OpenAI-compatible embedding response from `/v1/embeddings`.

References:

- Qwen local llama.cpp guide:
  <https://qwen.readthedocs.io/en/latest/run_locally/llama.cpp.html>
- Qwen3-Embedding-0.6B-GGUF:
  <https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF>
- MLX Qwen3-Embedding-0.6B-mxfp8:
  <https://huggingface.co/mlx-community/Qwen3-Embedding-0.6B-mxfp8>
- mlx-openai-server:
  <https://pypi.org/project/mlx-openai-server/>
