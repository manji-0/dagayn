# Local Embeddings

<!-- constrained-by ./COMMANDS.md -->

This document describes the **`--mode local`** install path: dagayn generates
semantic-search embeddings locally during `build` and `update` by starting
`llama-server` as a subprocess and talking to its OpenAI-compatible
`/v1/embeddings` endpoint.  The other two install modes are `--mode fts`
(no embeddings, fastest) and `--mode remote` (OpenAI-compatible / Google /
MiniMax cloud APIs) — see the README's "Choosing an install mode" section.

## Presets

| Preset | Model | Quantization | Dimension |
| --- | --- | --- | --- |
| `low` | `Qwen/Qwen3-Embedding-0.6B-GGUF` | `Q8_0` | 1024 |
| `high` | `Qwen/Qwen3-Embedding-4B-GGUF` | `Q4_K_M` | 2560 |

The Qwen3 Embedding series is published as `0.6B`, `4B`, and `8B` embedding
models. The `high` preset uses the official 4B GGUF 4-bit quantization, while
`low` uses the official 0.6B GGUF preset.

## Setup

Install `llama.cpp` so `llama-server` is available on `PATH`:

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

```bash
llama-server \
  -hf Qwen/Qwen3-Embedding-4B-GGUF:Q4_K_M \
  --embedding \
  --pooling last \
  -ub 8192 \
  --host 127.0.0.1 \
  --port 18080 \
  --alias qwen3-embedding-4b-gguf-q4_k_m
```

Then verify the OpenAI-compatible endpoint:

```bash
curl http://127.0.0.1:18080/v1/embeddings \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer dagayn-local' \
  -d '{"model":"qwen3-embedding-4b-gguf-q4_k_m","input":["dagayn local embedding test"]}'
```

## Build And Update

<!-- derived-from #setup -->

Run a full build with local embeddings:

```bash
dagayn build --local-embedding high
```

Run an incremental update and refresh missing or stale embeddings:

```bash
dagayn update --local-embedding low
```

If no compatible server is already listening on `127.0.0.1:18080`, dagayn
starts `llama-server` for the duration of the command. By default, dagayn stops
that subprocess when embedding finishes.

Useful options:

```bash
dagayn build \
  --local-embedding high \
  --local-embedding-port 18080 \
  --local-embedding-bin llama-server \
  --local-embedding-timeout 300 \
  --local-embedding-request-timeout 60 \
  --local-embedding-batch-size 1
```

`--local-embedding-timeout` only controls server readiness. If an individual
embedding batch stalls after the server is ready,
`--local-embedding-request-timeout` bounds that HTTP request. Successful
batches are saved as they complete, so rerunning the command resumes from the
remaining stale or missing embeddings.

The batch size is also pinned for local embeddings. `dagayn build
--local-embedding low` defaults to 1 text per request even if the shell has
`CRG_OPENAI_BATCH_SIZE` set for another provider. Raise it only after measuring;
larger batches can make llama-server's embedding endpoint stall on some hosts.

By default, embeddings are generated from graph metadata: symbol name,
qualified name, file path, display name, signature, params, return type, kind,
parent, and language. Set `DAGAYN_EMBEDDING_TEXT_MODE=body` to append each
non-file node's bounded source span to that metadata before embedding. The
source span is capped by `DAGAYN_EMBEDDING_SOURCE_CHARS` and defaults to 2048
characters. Body mode can improve conceptual searches where the query terms
appear only in implementation text, at the cost of larger embedding inputs and
more frequent re-embedding when function bodies change.

Leave a dagayn-started server running for reuse:

```bash
dagayn build --local-embedding high --keep-local-embedding-server
```

## Search quality

Measured on the dagayn codebase (4,597 nodes). Query set: 5 exact function names, 3 PascalCase class names, 4 conceptual natural-language queries.

### Aggregate

| Mode | mean MRR | Precision@1 | Precision@5 |
|---|---|---|---|
| FTS5 only | 0.71 | 0.67 | 0.75 |
| Qwen3-Embedding-0.6B `low` (hybrid) | **0.88** | **0.83** | **0.92** |
| Qwen3-Embedding-4B `high` (hybrid) | 0.82 | 0.75 | 0.92 |

### Per-query breakdown

| Query | Label | FTS rank | Qwen3-0.6B rank | Qwen3-4B rank |
|---|---|---|---|---|
| `hybrid_search` | exact_name | 1 | 1 | 1 |
| `rebuild_fts_index` | exact_name | 1 | 1 | 1 |
| `rrf_merge` | exact_name | 1 | 1 | 1 |
| `full_build` | exact_name | 2 | 2 | 2 |
| `detect_query_kind_boost` | exact_name | 1 | 1 | 1 |
| `GraphStore` | pascal_case_class | 1 | 1 | 1 |
| `EmbeddingProvider` | pascal_case_class | 1 | 1 | 1 |
| `LocalEmbeddingProvider` | pascal_case_class | 1 | 1 | 1 |
| "reciprocal rank fusion" | conceptual | — | **1** | **1** |
| "sentence transformers local model" | conceptual | — | **1** | **3** |
| "kind boost detection query" | conceptual | 1 | 1 | 1 |
| "incremental graph construction" | conceptual | — | — | — |

FTS5 is strong on exact and PascalCase queries. Both Qwen3 sizes close the gap on conceptual queries where FTS5 returns nothing. The 0.6B model matches or beats the 4B model on this 12-query suite (notably ranking "sentence transformers local model" → `LocalEmbeddingProvider` at 1 vs. 4B's 3), at ~1/7 the memory footprint — so `low` is the recommended default unless you have a specific reason to use `high`. The last query ("incremental graph construction" → `full_build`) is a miss for both because the function name shares no tokens with the query.

## Troubleshooting

<!-- derived-from #manual-check -->

- `Could not find 'llama-server'`: install `llama.cpp` or pass
  `--local-embedding-bin /path/to/llama-server`.
- Port already in use: either stop the process on that port or use
  `--local-embedding-port`.
- Timeout while starting: the first run may download the GGUF from Hugging
  Face; increase `--local-embedding-timeout` or run `llama-server` manually to
  watch progress.
- Incompatible endpoint: dagayn found something on the port, but it did not
  return an OpenAI-compatible embedding response from `/v1/embeddings`.

References:

- Qwen local llama.cpp guide:
  <https://qwen.readthedocs.io/en/latest/run_locally/llama.cpp.html>
- Qwen3-Embedding-4B-GGUF:
  <https://huggingface.co/Qwen/Qwen3-Embedding-4B-GGUF>
- Qwen3-Embedding-0.6B-GGUF:
  <https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF>
