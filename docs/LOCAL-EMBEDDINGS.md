# Local Embeddings

<!-- constrained-by ./COMMANDS.md -->

dagayn can generate semantic-search embeddings locally during `build` and
`update` by starting `llama-server` as a subprocess and talking to its
OpenAI-compatible `/v1/embeddings` endpoint.

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
  --local-embedding-timeout 300
```

Leave a dagayn-started server running for reuse:

```bash
dagayn build --local-embedding high --keep-local-embedding-server
```

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
