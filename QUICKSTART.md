# seminstruct Quick Start

`seminstruct` ships pre-built `llama-server` images with curated GGUF
models baked in. No build required for the published variants — pull and
run.

## Prerequisites

- Docker (with 2GB+ available memory for the default `:latest` image)
- [Task](https://taskfile.dev/#/installation) (optional)

## Pull and Run

```bash
cd seminstruct

# Start (pulls ghcr.io/c360studio/seminstruct:latest = Qwen3-0.6B hot tier)
docker compose up -d

# Wait for the model to load (~30s on first start)
docker compose logs -f seminstruct

# Test chat completions (OpenAI-compatible)
curl http://localhost:8083/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen3-0.6b",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'

# Stop
docker compose down
```

## Image Variants

CI publishes three variants tagged by the model inside:

| Tag | Model | Image | Memory | reasoning | Use |
|---|---|---|---|---|---|
| `:qwen3-0.6b` (= `:latest`) | Qwen3-0.6B Q4_K_M | ~600MB | ~1.0GB | `off` | Hot path: classify, intent, `defaults.model` |
| `:qwen3-8b` | Qwen3-8B Q4_K_M | ~5.3GB | ~6.0GB | `auto` | **Recommended summary tier**: community_summary, answer_synthesis (graph-persisting work) |
| `:qwen3-1.7b` | Qwen3-1.7B Q4_K_M | ~1.4GB | ~2.0GB | `auto` | Memory-constrained summary fallback (only if 8B won't fit; expect quality degradation) |

The 8B is the recommended summary tier because community summaries get
persisted into the graph and downstream queries compound their errors.
See README's "Image Variants" section for the rationale and the
GraphRAG context.

To run a different tier, edit `docker-compose.yml` to swap the image tag
and update `MODEL_ALIAS` / `MODEL_REASONING` env to match.

For other models build locally with build args:

```bash
docker build -t seminstruct:mistral \
  --build-arg MODEL_REPO=TheBloke/Mistral-7B-Instruct-v0.2-GGUF \
  --build-arg MODEL_FILE=mistral-7b-instruct-v0.2.Q4_K_M.gguf \
  --build-arg MODEL_ALIAS=mistral-7b \
  --build-arg MODEL_REASONING=off .
```

For a multi-tier deployment with capability-aware routing, see the
**Deployment Patterns** section in [README.md](./README.md).

## Architecture

```text
┌─────────────────────────────────────┐
│       seminstruct:8083              │  llama-server (llama.cpp)
│       ghcr.io/c360studio/           │  -np 4 -cb (4 parallel slots)
│       seminstruct:<variant>         │  GGUF baked in
└─────────────────────────────────────┘
```

That's the whole architecture — one container, one model, OpenAI API.

## Troubleshooting

### Service won't start / model never loads

```bash
docker compose logs seminstruct
docker compose down
docker compose up -d
```

The first start downloads no model — the GGUF is baked into the image —
but llama-server still needs ~10-30s to mmap the file and warm up. If
`/health` doesn't return 200 within ~60s the container is genuinely
broken; check logs.

### Health check failing

```bash
curl -v http://localhost:8083/health
```

Returns `200 OK` once the model is loaded. Anything else is a real
failure.

### Port already in use

```bash
lsof -i :8083
# change the host-side port in docker-compose.yml if needed
```

## Quick Reference

| Endpoint | Method | Purpose |
|---|---|---|
| `/health` | GET | Liveness/readiness — 200 once model loaded |
| `/v1/models` | GET | Lists the baked-in model under its alias |
| `/v1/chat/completions` | POST | OpenAI-compatible chat |
| `/metrics` | GET | llama-server's native Prometheus metrics |

**Service URL**: `http://localhost:8083`

**Default model**: `qwen3-0.6b` (Qwen3-0.6B Q4_K_M, `--reasoning off`)

**Expected first-token latency**: 200-400ms for `:qwen3-0.6b` on CPU;
500ms-1.5s for `:qwen3-1.7b`; 2-5s for `:qwen3-8b` on CPU (GPU
recommended for the 8B tier).

## Before You Push

```bash
task integration    # Full build + test + cleanup
```

## Next Steps

- [README.md](./README.md) for full documentation including deployment
  patterns and resource budgets
- [semembed](https://github.com/c360studio/semembed) for the embedding tier
- Integrate via SemStreams's `model_registry` (capability → endpoint URL)
