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
│  llama-server (llama.cpp)           │  ~1-2GB memory (0.5B Q4)
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
    "model": "qwen2.5-0.5b",
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
  "model": "qwen2.5-0.5b",
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
  "model": "qwen2.5-0.5b",
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
    model="qwen2.5-0.5b",
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
    "model": "qwen2.5-0.5b",
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

## Performance

| Metric | seminstruct | semserve |
|--------|-------------|----------|
| Memory | ~256MB | ~1-2GB (Qwen2.5-0.5B Q4_K_M) |
| Startup | <1s | ~30s (model already loaded) |
| Container Size | ~50MB | ~600MB |
| Concurrency | per-connection (Tokio) | 4 parallel slots (`-np 4 -cb`) |

## Docker Deployment

### Docker Compose (Recommended)

```bash
docker compose up -d
```

This pulls pre-built images from GHCR and starts both services.

### Manual

```bash
# Start semserve first (pre-built with Qwen2.5-0.5B)
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

### Custom Models

To use a different GGUF model, build with `Dockerfile.semserve`:

```bash
MODEL_REPO=TheBloke/Mistral-7B-Instruct-v0.2-GGUF \
MODEL_FILE=mistral-7b-instruct-v0.2.Q4_K_M.gguf \
docker build -f Dockerfile.semserve -t semserve:custom .
```

Then update `docker-compose.yml` to use `semserve:custom`.

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
**Backend**: semserve / llama-server (Qwen2.5-0.5B default, configurable)
**API**: OpenAI-compatible `/v1/chat/completions`
