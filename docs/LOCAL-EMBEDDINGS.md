# Local Embeddings

<!-- constrained-by ./COMMANDS.md -->

This document describes dagayn's local embedding paths. A bare
`--local-embedding` on `build`, `update`, or `serve` uses the measured default:
in-process sentence-transformers with `BAAI/bge-m3` and the `material` text
mode. The `--mode local-embedding` install path writes MCP configs that serve
BGE-M3 with the same in-process path. Use `--mode local-embedding-llama` or the
legacy `--local-embedding low` request for the managed Qwen3 llama.cpp sidecar.
The other install modes are `--mode fts-only` (no embeddings, fastest) and
`--mode remote-embedding` (OpenAI-compatible / Google / MiniMax cloud APIs) —
see the README's "Choosing an install mode" section.

## Local modes

| Request | Runtime | Model | Dimension |
| --- | --- | --- | --- |
| `--local-embedding` | in-process sentence-transformers | `BAAI/bge-m3` | 1024 |
| `--local-embedding --mode llama-qwen3` | `llama-server` sidecar | `Qwen/Qwen3-Embedding-0.6B-GGUF:Q8_0` | 1024 |
| `--local-embedding low` | `llama-server` sidecar | `Qwen/Qwen3-Embedding-0.6B-GGUF:Q8_0` | 1024 |

## Qwen Sidecar Presets

| Preset | Runtime | Default platform | Model | Quantization | Dimension |
| --- | --- | --- | --- | --- | --- |
| `low` | `llama-server` | all platforms | `Qwen/Qwen3-Embedding-0.6B-GGUF` | `Q8_0` | 1024 |

The Qwen3 Embedding series is published as `0.6B`, `4B`, and `8B` embedding
models. The local preset uses the 0.6B 8-bit GGUF variant through
`llama-server`. `DAGAYN_LOCAL_EMBEDDING_RUNTIME=llama` is accepted for
explicit configuration; other runtime values are rejected.

## Qwen Sidecar Setup

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

## Qwen Manual Check

<!-- derived-from #qwen-sidecar-presets -->

You can start the server yourself before running dagayn. dagayn will reuse a
compatible server already listening on the configured port.

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

Run a full build with the default in-process BGE-M3 local embeddings:

```bash
dagayn build --local-embedding
```

Run an incremental update and refresh missing or stale BGE-M3 embeddings:

```bash
dagayn update --local-embedding
```

Use the managed Qwen3 llama.cpp sidecar explicitly when you want the legacy
OpenAI-compatible server behavior:

```bash
dagayn build --local-embedding --mode llama-qwen3
dagayn update --local-embedding low
```

Do not use embedding-enabled full rebuilds as routine parser, flow,
documentation-edge, or review verification. For those checks, run the graph
refresh without local embeddings, for example `dagayn update --local-embedding
none` or the equivalent `build_or_update_graph_tool(local_embedding="none")`.
Reserve `dagayn build --force-full-build --local-embedding` for explicit
embedding-quality or end-to-end maintenance work after stating why the embedding
refresh itself is required.

For Qwen sidecar mode, if no compatible server is already listening on `127.0.0.1:18080`, dagayn
starts `llama-server` for the duration of the command. By default, dagayn stops
that subprocess when embedding finishes. Managed starts are serialized per port
with a lock under `~/.dagayn/`, so concurrent dagayn processes do not launch
multiple `llama-server` subprocesses for the same localhost port.

Useful Qwen sidecar options:

```bash
dagayn build \
  --local-embedding --mode llama-qwen3 \
  --local-embedding-port 18080 \
  --local-embedding-bin auto \
  --local-embedding-timeout 300 \
  --local-embedding-request-timeout 60 \
  --local-embedding-batch-size 1
```

`--local-embedding-bin auto` resolves to `llama-server`. Pass an explicit
executable name or path to override it.

`--local-embedding-timeout` only controls Qwen sidecar server readiness. If an individual
embedding batch stalls after the server is ready,
`--local-embedding-request-timeout` bounds that HTTP request. Successful
batches are saved as they complete, so rerunning the command resumes from the
remaining stale or missing embeddings.

The Qwen sidecar batch size is also pinned. `dagayn build
--local-embedding --mode llama-qwen3` defaults to 1 text per request even if the shell has
`CRG_OPENAI_BATCH_SIZE` set for another provider. Raise it only after measuring;
larger batches can make local embedding endpoints stall on some hosts.

Dagayn starts `llama-server` with Flash Attention
enabled and keeps both KV cache tensors at `f16`. Flash Attention improves
prompt/embedding throughput on supported backends without changing the model
weights. The KV cache type is intentionally not quantized by the preset:
`q8_0`, `q4_0`, and similar cache types can reduce memory pressure, but they
also add another approximation layer to attention state. Since the local preset
already uses an 8-bit GGUF model, keeping K and V at `f16` is the conservative
quality default. The logical and physical llama.cpp batch limits are both set to
8192 (`-b 8192 -ub 8192`) so long embedding inputs are processed in larger
chunks when memory allows.

The default embedding material is `material`, chosen from local measurements on
mixed code and documentation queries. Markdown `DocSection`/`DocBody` nodes
embed the bounded section or paragraph body. Code classes/functions/methods
embed symbol name, qualified name, file path, parent, language, and adjacent or
owned comment sentences; signatures and implementation bodies are left out by
default because the measured material benchmark favored symbol-name material
plus comments over signature-heavy or body-heavy inputs.

Set `DAGAYN_EMBEDDING_TEXT_MODE=metadata`, `material`, `body`, `structured`,
or `narrative` to override that behavior for any provider or preset. The
`structured` mode is an experimental labeled code-reference representation that
combines node metadata with the bounded source span. The `narrative` mode is a
static, deterministic natural-language rendering of code-reference facts such
as calls, assignments, returns, branches, loops, IO, search, embedding
operations, and graph relationships such as `CALLS`, `IMPORTS_FROM`,
`REFERENCES`, `TESTED_BY`, and callers. These modes make it easier to compare
AST-derived, graph-derived, or source-derived explanations against the default
material. The source span is capped by
`DAGAYN_EMBEDDING_SOURCE_CHARS` and defaults to 2048 characters. Body mode can
improve conceptual searches where the query terms appear only in implementation
text or Markdown section bodies, at the cost of larger embedding inputs and
more frequent re-embedding when function bodies or documentation sections
change.

Hybrid search routes prose query intent across these materials. Purpose-like
queries use the `material` text because names and adjacent comments usually
carry intent. Process-pattern queries use `narrative` text because static
source and graph facts expose operations such as calls, reads, writes, returns,
loops, merges, searches, and rebuilds. Persisted vectors are partitioned by
provider plus text mode, so running `material` and `narrative` embeddings for
the same provider keeps both rows available for routing.

Leave a dagayn-started server running for reuse:

```bash
dagayn build --local-embedding --mode llama-qwen3 --keep-local-embedding-server
```

## Search quality

The measurements below use the best measured material strategy
(`doc=section|code=name|comment=sentence|join=combined`) on the dagayn codebase:
11,741 embedded materials, 8,236 graph references, 31 positive queries, and 5
unrelated negative calibration queries. `negative top score` is lower-is-better.

| Local model | positive MRR | Precision@5 | negative top score | embedding throughput |
|---|---:|---:|---:|---:|
| `BAAI/bge-m3` | **0.5639** | **0.6774** | **0.4157** | 87.8 nodes/s |
| `intfloat/multilingual-e5-base` | 0.5317 | 0.6452 | 0.8127 | **302.1 nodes/s** |
| `nomic-ai/nomic-embed-text-v1.5` | 0.5215 | 0.5806 | 0.5202 | 136.6 nodes/s |
| `mixedbread-ai/mxbai-embed-large-v1` | 0.4604 | 0.4516 | 0.4700 | 85.4 nodes/s |
| Qwen3-Embedding-0.6B Q8 `low` | 0.4301 | 0.5484 | 0.5261 | 55.4 nodes/s |
| `jinaai/jina-embeddings-v2-base-code` | 0.3604 | 0.4516 | 0.4753 | 171.8 nodes/s |

`BAAI/bge-m3` is the default for `embed_graph_tool(provider="local")`, the
sentence-transformers local provider, and bare `--local-embedding` on
`build`, `update`, and `serve`. The managed `low` preset remains Qwen3 GGUF
because it provides a no-Python-model-server path through `llama-server`.

## Troubleshooting

<!-- derived-from #manual-check -->

- `Could not find 'llama-server'`: install `llama.cpp` or pass
  `--local-embedding-bin /path/to/llama-server`.
- Port already in use: either stop the process on that port or use
  `--local-embedding-port`.
- Timeout while starting: the first run may download the GGUF model from
  Hugging Face; increase `--local-embedding-timeout` or run the local server
  manually to watch progress.
- Incompatible endpoint: dagayn found something on the port, but it did not
  return an OpenAI-compatible embedding response from `/v1/embeddings`.

References:

- Qwen local llama.cpp guide:
  <https://qwen.readthedocs.io/en/latest/run_locally/llama.cpp.html>
- Qwen3-Embedding-0.6B-GGUF:
  <https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF>
