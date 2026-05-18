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

The Qwen3 Embedding series is published as `0.6B`, `4B`, and `8B` embedding
models. The local preset uses the official 0.6B GGUF 8-bit quantization.

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
  -hf Qwen/Qwen3-Embedding-0.6B-GGUF:Q8_0 \
  --embedding \
  --pooling last \
  -ub 8192 \
  --host 127.0.0.1 \
  --port 18080 \
  --alias qwen3-embedding-0.6b-gguf-q8_0
```

Then verify the OpenAI-compatible endpoint:

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
starts `llama-server` for the duration of the command. By default, dagayn stops
that subprocess when embedding finishes.

Useful options:

```bash
dagayn build \
  --local-embedding low \
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

The local preset embeds graph metadata: symbol name, qualified name, file path,
display name, signature, params, return type, kind, parent, and language.

Set `DAGAYN_EMBEDDING_TEXT_MODE=metadata` or `body` to override that behavior
for any provider or preset. The source span is capped by
`DAGAYN_EMBEDDING_SOURCE_CHARS` and defaults to 2048 characters. Body mode can
improve conceptual searches where the query terms appear only in implementation
text, at the cost of larger embedding inputs and more frequent re-embedding
when function bodies change.

Leave a dagayn-started server running for reuse:

```bash
dagayn build --local-embedding low --keep-local-embedding-server
```

## Search quality

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
- Qwen3-Embedding-0.6B-GGUF:
  <https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF>
