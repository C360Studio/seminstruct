# SemInstruct - OpenAI-Compatible Inference Proxy

**Status**: Alpha
**Package**: `seminstruct`
**Port**: `8083`

## Overview

SemInstruct is a lightweight proxy service that provides an OpenAI-compatible API by forwarding requests to [shimmy](https://github.com/michael-a-kuykendall/shimmy), a high-performance inference backend.

**Key Features**:

- OpenAI API compatible (`/v1/chat/completions`)
- Lightweight Rust proxy (~50MB container, ~256MB memory)
- Fast startup (no model loading)
- Retry logic with exponential backoff
- Health checks for shimmy backend
- Prometheus metrics

## Why a Proxy?

SemInstruct exists to decouple your SemStreams application from the inference backend:

1. **Backend Flexibility** - Swap shimmy for OpenAI, Ollama, or any OpenAI-compatible service without code changes
2. **Reliability** - Built-in retry logic with exponential backoff handles transient failures
3. **Observability** - Prometheus metrics for request rates, latencies, and error tracking
4. **Health Monitoring** - Aggregated health checks report backend availability
5. **Fast Startup** - No model loading means instant restarts and scaling

## Why Not MCP?

You might wonder why SemInstruct uses a traditional HTTP proxy instead of [Model Context Protocol (MCP)](https://modelcontextprotocol.io/).

**Use Case**: SemInstruct serves [SemStreams](https://github.com/c360studio/semstreams), a streaming data processing system. SemStreams graph nodes make inference requests during stream processing - they already have all the context they need from the data flowing through the graph.

**Why HTTP fits better**:

1. **Stateless by Design** - Each inference request is self-contained. SemStreams nodes pass complete context (document text, classification labels, etc.) in each request. No conversation history or tool discovery needed.

2. **No Tool Orchestration** - MCP excels when an LLM needs to discover and invoke tools dynamically. SemStreams nodes know exactly what they need - extract entities, classify text, summarize content - and include all inputs in the request.

3. **Streaming Architecture** - Requests arrive from NATS streams at high throughput. HTTP's request/response model maps cleanly to stream processing semantics.

4. **Backend Flexibility** - The OpenAI-compatible API lets you swap backends (shimmy → Ollama → OpenAI → vLLM) without code changes. MCP would tie you to a specific protocol.

**When MCP makes sense**: Interactive assistants, agentic workflows with tool use, or applications where the LLM needs to discover capabilities at runtime.

**When HTTP makes sense**: Batch processing, stream processing, or any workload where the caller knows exactly what inference it needs and provides complete context per request.

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
│            shimmy                   │
│  (Model loading & inference)       │  ~6-8GB memory
└─────────────────────────────────────┘
```

## Quick Start

```bash
# Start services (pulls pre-built images from GHCR)
docker compose up -d

# Wait for services to be ready (~30s)
docker compose logs -f shimmy

# Test with curl
curl http://localhost:8083/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen2.5-0.5b",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

No build required - images are pulled from `ghcr.io/c360studio/semshimmy`.

## API Reference

### POST /v1/chat/completions

OpenAI-compatible chat completions endpoint (proxied to shimmy).

**Request**:

```json
{
  "model": "mistral-7b-instruct",
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
  "model": "mistral-7b-instruct",
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

List available models (proxied from shimmy).

```bash
curl http://localhost:8083/v1/models
```

### GET /health

Health check endpoint. Returns shimmy backend status.

```bash
curl http://localhost:8083/health
```

**Response**:

```json
{
  "status": "healthy",
  "shimmy_url": "http://shimmy:8080",
  "shimmy_healthy": true
}
```

### GET /metrics

Prometheus metrics including:

- `seminstruct_requests_total` - Total requests
- `seminstruct_request_duration_seconds` - Request latency
- `seminstruct_errors_total` - Total errors
- `seminstruct_shimmy_errors_total` - Shimmy backend errors

## Client Examples

### Python (OpenAI SDK)

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://localhost:8083/v1",
    api_key="not-needed"
)

response = client.chat.completions.create(
    model="mistral-7b-instruct",
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
    "model": "mistral-7b-instruct",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `SEMINSTRUCT_SHIMMY_URL` | `http://localhost:8080` | Shimmy backend URL |
| `SEMINSTRUCT_PORT` | `8083` | HTTP port |
| `SEMINSTRUCT_TIMEOUT_SECONDS` | `120` | Request timeout |
| `SEMINSTRUCT_MAX_RETRIES` | `3` | Max retry attempts |
| `RUST_LOG` | `info` | Log level |

## Performance

| Metric | seminstruct | shimmy |
|--------|-------------|--------|
| Memory | ~256MB | ~1-2GB (Qwen2.5-0.5B) |
| Startup | <1s | ~30s (model already loaded) |
| Container Size | ~50MB | ~600MB |

## Docker Deployment

### Docker Compose (Recommended)

```bash
docker compose up -d
```

This pulls pre-built images from GHCR and starts both services.

### Manual

```bash
# Start shimmy first (pre-built with Qwen2.5-0.5B)
docker run -d \
  --name shimmy \
  -p 11435:11435 \
  ghcr.io/c360studio/semshimmy:latest

# Then start seminstruct
docker run -d \
  --name seminstruct \
  -p 8083:8083 \
  -e SEMINSTRUCT_SHIMMY_URL=http://shimmy:11435 \
  --link shimmy \
  ghcr.io/c360studio/seminstruct:latest
```

### Custom Models

To use a different model, build with Dockerfile.shimmy:

```bash
MODEL_REPO=TheBloke/Mistral-7B-Instruct-v0.2-GGUF \
MODEL_FILE=mistral-7b-instruct-v0.2.Q4_K_M.gguf \
docker build -f Dockerfile.shimmy -t shimmy:custom .
```

## Project Structure

```shell
seminstruct/
├── Cargo.toml              # Dependencies (axum, reqwest, etc.)
├── src/main.rs             # HTTP proxy + shimmy client
├── Dockerfile              # seminstruct build (2-stage)
├── Dockerfile.shimmy       # Custom model builds
├── docker-compose.yml      # Uses pre-built GHCR images
├── docker-compose.ci.yml   # CI override (builds from source)
└── README.md
```

**Stack**:

- **Axum**: Async web framework
- **Reqwest**: HTTP client for shimmy
- **Shimmy**: Inference backend (separate container)

## License

MIT

---

**Port**: `8083`
**Backend**: shimmy (Qwen2.5-0.5B default, configurable)
**API**: OpenAI-compatible `/v1/chat/completions`
