# SemInstruct - OpenAI-Compatible Inference Proxy

**Status**: Alpha
**Package**: `seminstruct`
**Port**: `8083`

## Overview

SemInstruct is a lightweight proxy service that provides an OpenAI-compatible API
by forwarding requests to **semserve**, a small inference backend built on
[llama.cpp](https://github.com/ggml-org/llama.cpp)'s `llama-server` with a GGUF
model baked in.

**Key Features**:

- OpenAI API compatible (`/v1/chat/completions`)
- Lightweight Rust proxy (~50MB container, ~256MB memory)
- Concurrent inference: semserve runs `-np 4 -cb` (four parallel slots,
  continuous batching) — fixes request stacking under graph-busy bursts
- Fast startup (no model loading in the proxy)
- Retry logic with exponential backoff
- Health and readiness checks for the backend
- Prometheus metrics

## Why a Proxy?

SemInstruct exists to decouple your SemStreams application from the inference
backend:

1. **Backend Flexibility** - Swap semserve for OpenAI, Ollama, or any
   OpenAI-compatible service without code changes
2. **Reliability** - Built-in retry logic with exponential backoff handles
   transient failures
3. **Observability** - Prometheus metrics for request rates, latencies, and
   error tracking
4. **Health Monitoring** - Aggregated health checks report backend availability
5. **Fast Startup** - No model loading means instant restarts and scaling

## Why Not MCP?

You might wonder why SemInstruct uses a traditional HTTP proxy instead of
[Model Context Protocol (MCP)](https://modelcontextprotocol.io/).

**Use Case**: SemInstruct serves
[SemStreams](https://github.com/c360studio/semstreams), a streaming data
processing system. SemStreams graph nodes make inference requests during stream
processing - they already have all the context they need from the data flowing
through the graph.

**Why HTTP fits better**:

1. **Stateless by Design** - Each inference request is self-contained.
   SemStreams nodes pass complete context (document text, classification labels,
   etc.) in each request. No conversation history or tool discovery needed.

2. **No Tool Orchestration** - MCP excels when an LLM needs to discover and
   invoke tools dynamically. SemStreams nodes know exactly what they need -
   extract entities, classify text, summarize content - and include all inputs
   in the request.

3. **Streaming Architecture** - Requests arrive from NATS streams at high
   throughput. HTTP's request/response model maps cleanly to stream processing
   semantics.

4. **Backend Flexibility** - The OpenAI-compatible API lets you swap backends
   (semserve → Ollama → OpenAI → vLLM) without code changes. MCP would tie you
   to a specific protocol.

**When MCP makes sense**: Interactive assistants, agentic workflows with tool
use, or applications where the LLM needs to discover capabilities at runtime.

**When HTTP makes sense**: Batch processing, stream processing, or any workload
where the caller knows exactly what inference it needs and provides complete
context per request.

## Architecture

```text
┌─────────────────────────────────────┐
│           seminstruct               │
│  ┌─────────────────────────────┐    │
│  │ Axum HTTP Proxy             │    │  ~256MB memory
│  │ Retry logic, metrics        │    │  Fast startup
│  └─────────────────────────────┘    │
└──────────────┬──────────────────────┘
               │ HTTP proxy
               ▼
┌─────────────────────────────────────┐
│            semserve                 │
│  llama-server (llama.cpp)           │  ~1.5GB memory (0.6B Q4, np=4)
│  -np 4 -cb (concurrent inference)   │
└─────────────────────────────────────┘
```

## Quick Start

```bash
# Start services (pulls pre-built images from GHCR)
docker compose up -d

# Wait for services to be ready (~30s)
docker compose logs -f semserve

# Test with curl
curl http://localhost:8083/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen3-0.6b",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

No build required - images are pulled from `ghcr.io/c360studio/semserve`.

## API Reference

### POST /v1/chat/completions

OpenAI-compatible chat completions endpoint (proxied to semserve).

**Request**:

```json
{
  "model": "qwen3-0.6b",
  "messages": [
    {"role": "system", "content": "You are a helpful assistant."},
    {"role": "user", "content": "Summarize this article..."}
  ],
  "max_tokens": 256,
  "temperature": 0.7
}
```

**Response**:

```json
{
  "id": "chatcmpl-abc123def456ghi789",
  "object": "chat.completion",
  "created": 1699000000,
  "model": "qwen3-0.6b",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Here is a summary..."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 50,
    "completion_tokens": 100,
    "total_tokens": 150
  }
}
```

### GET /v1/models

List available models (proxied from semserve).

```bash
curl http://localhost:8083/v1/models
```

### GET /health

Health check endpoint. Returns the inference backend's status.

```bash
curl http://localhost:8083/health
```

**Response**:

```json
{
  "status": "healthy",
  "backend_url": "http://semserve:11435",
  "backend_healthy": true
}
```

### GET /ready

Readiness probe. Returns 200 only when the backend can complete inference
(verifies the model is actually loaded, not just that the process is up). Use
for Kubernetes readiness probes.

### GET /metrics

Prometheus metrics including:

- `seminstruct_requests_total` - Total requests
- `seminstruct_request_duration_seconds` - Request latency
- `seminstruct_errors_total` - Total errors
- `seminstruct_backend_errors_total` - Inference backend errors

## Client Examples

### Python (OpenAI SDK)

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://localhost:8083/v1",
    api_key="not-needed"
)

response = client.chat.completions.create(
    model="qwen3-0.6b",
    messages=[
        {"role": "system", "content": "You are a helpful assistant."},
        {"role": "user", "content": "Hello!"}
    ],
    max_tokens=100
)

print(response.choices[0].message.content)
```

### curl

```bash
curl http://localhost:8083/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen3-0.6b",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `SEMINSTRUCT_BACKEND_URL` | `http://localhost:11435` | Inference backend URL |
| `SEMINSTRUCT_PORT` | `8083` | HTTP port |
| `SEMINSTRUCT_TIMEOUT_SECONDS` | `120` | Request timeout |
| `SEMINSTRUCT_MAX_RETRIES` | `3` | Max retry attempts |
| `RUST_LOG` | `info` | Log level |

## Image Variants

CI publishes two pre-baked semserve images, tagged by **the model inside**:

| Tag | Model | Image size | Memory (np=4, ctx=2048) | Best for |
|---|---|---|---|---|
| `:qwen3-0.6b` (also `:latest`) | Qwen3-0.6B Q4_K_M | ~600MB | ~1.2GB | Hot path: intent + classify, `defaults.model`, every-message latency |
| `:qwen3-1.7b` | Qwen3-1.7B Q4_K_M | ~1.4GB | ~2GB | Quality tier: community summaries, answer synthesis, anomaly review |

Tagging by model name (not deployment role) is intentional — `:qwen3-0.6b`
tells you what's *inside* the image; "hot" or "quality" is deployment intent
that lives in your compose / registry config, not the image tag. `:latest`
points at the 0.6B variant because that's the most common single-endpoint
deployment.

**Qwen3 thinking**: Qwen3 is a hybrid model that emits chain-of-thought
inside `<think>...</think>` tags (separated into `message.reasoning_content`
on the response). The hot image bakes `--reasoning off` because intent
classification doesn't benefit from 100+ tokens of thinking prefix; the
quality image bakes `--reasoning auto` so it can think on harder summary
work. Override per-deployment with `-e SEMSERVE_REASONING=on|off|auto`.

For other models (Mistral, etc.) build locally with `Dockerfile.semserve`:

```bash
MODEL_REPO=TheBloke/Mistral-7B-Instruct-v0.2-GGUF \
MODEL_FILE=mistral-7b-instruct-v0.2.Q4_K_M.gguf \
SEMSERVE_ALIAS=mistral-7b \
docker build -f Dockerfile.semserve -t semserve:mistral \
  --build-arg MODEL_REPO --build-arg MODEL_FILE --build-arg SEMSERVE_ALIAS .
```

## Docker Deployment

### Docker Compose (Recommended)

```bash
docker compose up -d
```

This pulls pre-built images from GHCR and starts both services.

### Manual

```bash
# Start semserve first (pre-built with Qwen3-0.6B)
docker run -d \
  --name semserve \
  -p 11435:11435 \
  ghcr.io/c360studio/semserve:latest

# Then start seminstruct
docker run -d \
  --name seminstruct \
  -p 8083:8083 \
  -e SEMINSTRUCT_BACKEND_URL=http://semserve:11435 \
  --link semserve \
  ghcr.io/c360studio/seminstruct:latest
```

## Deployment Patterns

> **One seminstruct fronts one semserve.** The proxy reads a single
> `SEMINSTRUCT_BACKEND_URL` and routes everything there — there is no
> model-field-based dispatch inside seminstruct today. To run multiple
> tiers, you deploy multiple `seminstruct + semserve` pairs and let the
> caller (typically SemStreams's `model_registry`) pick which proxy URL
> to hit per capability. This split of concerns is intentional: the
> registry routes, seminstruct hardens (retry / circuit-break /
> metrics), semserve serves.

For the simplest case, run one `seminstruct` + one `semserve:latest` and
point everything at it. That works fine until two workload classes start
sharing the inference queue and the cheap one (e.g. intent classification)
starves behind the expensive one (e.g. graph community summaries). The
fix is to deploy a second pair on a different port and route by capability.

### Three-endpoint reference deployment

The shape we recommend for SemStreams workloads with the new model
registry (capabilities → endpoints). Each row is an independent
`seminstruct + semserve` pair plus the standalone `semembed` service:

```text
                  ┌── seminstruct-hot     :8083 ── semserve:qwen3-0.6b   :11435
                  │   (retry, metrics)            (--reasoning off, np=4 -cb)
semstreams ──────┤
(model_registry)  ├── seminstruct-quality :8084 ── semserve:qwen3-1.7b   :11435
                  │   (retry, metrics)            (--reasoning auto, np=4 -cb)
                  │
                  └── semembed                :8081 (own service, fastembed-rs)
```

semserve listens on 11435 inside its own container regardless of variant;
the host-side port mapping is what differs between the hot and quality
deployments. Each `seminstruct-*` proxy has its own
`SEMINSTRUCT_BACKEND_URL` pointing at exactly one semserve.

| Capability route | Proxy URL the registry hits | Backing semserve image | model id (`--alias`) | Capabilities to send here |
|---|---|---|---|---|
| Hot | `http://seminstruct-hot:8083` | `ghcr.io/c360studio/semserve:qwen3-0.6b` | `qwen3-0.6b` | `query_classification`, intent classification (hidden), `defaults.model` |
| Quality | `http://seminstruct-quality:8084` | `ghcr.io/c360studio/semserve:qwen3-1.7b` | `qwen3-1.7b` | `community_summary`, `answer_synthesis` fallback, anomaly review (piggyback) |
| Embedding | `http://semembed:8081` | `ghcr.io/c360studio/semembed:latest` | n/a (embeddings) | `embedding` |

### When you'd want multi-backend routing inside seminstruct instead

Adding model-field-based routing inside seminstruct (one URL, many
backends, dispatch by `model` field) is possible but not currently
implemented. It would duplicate the work the SemStreams `model_registry`
already does on the caller side, so for SemStreams workloads the
two-pairs deployment is the right shape. It's worth revisiting only if
a non-SemStreams consumer needs a single OpenAI-compatible URL covering
multiple models.

### Two operator notes that bite if missed

1. **Point `defaults.model` at the hot endpoint, not quality.** Several
   SemStreams call sites (intent classification on every user message,
   onboarding layer normalization) currently fall through to
   `defaults.model` — they have no capability constant yet. Putting
   `defaults.model` on the quality endpoint means every user message
   queues behind background summary work, which is the failure mode this
   project's concurrency story exists to prevent.

2. **Anomaly relationship review piggybacks on `community_summary`.**
   It's not its own capability — it shares the LLMClient injected into
   graph-clustering. Whatever endpoint owns `community_summary` will
   also carry anomaly review load; size accordingly and don't be
   surprised when `seminstruct_requests_total` for the quality endpoint
   is higher than `community_summary` alone would predict.

## Resource Use

### Per-process budget

For a single `semserve` process running with `-np 4 -cb -c 2048`:

| Component | Qwen3-0.6B Q4_K_M | Qwen3-1.7B Q4_K_M |
|---|---|---|
| Model weights | ~430MB | ~1.1GB |
| KV cache (4 slots × 2048 ctx) | ~110MB | ~250MB |
| Compute / activations / overhead | ~400MB | ~600MB |
| **Process total** | **~1.0GB** | **~2.0GB** |
| Container image on disk | ~600MB | ~1.4GB |

`seminstruct` (the proxy) is independent: ~256MB process memory, ~50MB
image, regardless of which backend it talks to.

### CPU under concurrency

`-np 4` means four logical inference slots, **not** four parallel forward
passes. With `-cb` (continuous batching) llama-server batches active slot
requests into a single forward pass per step — throughput goes up,
per-token latency stays roughly constant for the best slot, and the
worst-case slot only pays a small batching overhead. So:

- 4 cores is a reasonable floor for the hot tier; 8 cores comfortable.
- 8 cores is a reasonable floor for the quality tier on CPU.
- More slots than that costs little memory but produces diminishing
  returns once compute saturates — the bottleneck moves from queue
  depth to raw forward-pass throughput.

If you raise `-np`, also raise `-c` only if you need longer per-request
context — KV cache scales linearly with `slots × ctx`.

### Three-endpoint reference total

Co-locating the reference deployment on a single host:

| Component | Memory | Image |
|---|---|---|
| seminstruct (×1) | ~256MB | ~50MB |
| semserve hot (qwen3-0.6b) | ~1.0GB | ~600MB |
| semserve quality (qwen3-1.7b) | ~2.0GB | ~1.4GB |
| semembed | ~512MB | ~1GB |
| **Total** | **~3.8GB** | **~3GB on disk** |

Comfortable on a 4-core / 8GB host; comfortable headroom on 8-core / 16GB.

## Migration from `semshimmy`

This project's backend was previously called `semshimmy` and wrapped
[shimmy](https://github.com/michael-a-kuykendall/shimmy). It's been replaced
with `semserve` running `llama-server` directly. Consumer impact:

- **HTTP API consumers** (anything calling `seminstruct:8083`): no changes —
  OpenAI-compatible contract is unchanged.
- **Compose / deployment**: replace any `ghcr.io/c360studio/semshimmy:latest`
  reference with `ghcr.io/c360studio/semserve:latest`. If you reference the
  service by name in your own compose file, rename `shimmy` → `semserve`.
- **Env var**: the proxy's backend URL env var was renamed
  `SEMINSTRUCT_SHIMMY_URL` → `SEMINSTRUCT_BACKEND_URL`. The internal config is
  not consumer-facing in the typical deployment but flagged here for completeness.

## Project Structure

```shell
seminstruct/
├── Cargo.toml              # Dependencies (axum, reqwest, etc.)
├── src/main.rs             # HTTP proxy + backend client
├── Dockerfile              # seminstruct build (2-stage)
├── Dockerfile.semserve     # llama-server + GGUF baked-in image
├── docker-compose.yml      # Uses pre-built GHCR images
├── docker-compose.ci.yml   # CI override (builds semserve from source)
└── README.md
```

**Stack**:

- **Axum**: Async web framework
- **Reqwest**: HTTP client for the backend
- **semserve**: Inference backend (separate container) — `llama-server` with
  GGUF model baked in. See `Dockerfile.semserve`.

## License

MIT

---

**Port**: `8083`
**Backend**: semserve / llama-server (Qwen3-0.6B default `:latest`, Qwen3-1.7B `:qwen3-1.7b`, custom configurable)
**API**: OpenAI-compatible `/v1/chat/completions`
