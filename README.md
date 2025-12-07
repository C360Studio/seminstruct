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

SemInstruct exists to decouple your application from the inference backend:

1. **Backend Flexibility** - Swap shimmy for OpenAI, Ollama, or any OpenAI-compatible service without code changes
2. **Reliability** - Built-in retry logic with exponential backoff handles transient failures
3. **Observability** - Prometheus metrics for request rates, latencies, and error tracking
4. **Health Monitoring** - Aggregated health checks report backend availability
5. **Fast Startup** - No model loading means instant restarts and scaling

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
# Build and run (starts both seminstruct and shimmy)
docker compose up -d

# Wait for shimmy to download model (~4GB, first time only)
docker compose logs -f shimmy

# Test with curl
curl http://localhost:8083/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "mistral-7b-instruct",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

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
| Memory | ~256MB | ~6-8GB |
| Startup | <1s | 30-300s (model load) |
| Container Size | ~50MB | ~4GB |

## Docker Deployment

### Docker Compose (Recommended)

```bash
docker compose up -d
```

This starts both seminstruct and shimmy with proper health checks.

### Manual

```bash
# Start shimmy first
docker run -d \
  --name shimmy \
  -p 8080:8080 \
  -v shimmy-cache:/root/.cache/huggingface \
  ghcr.io/michael-a-kuykendall/shimmy:latest

# Then start seminstruct
docker build -t seminstruct:latest .

docker run -d \
  --name seminstruct \
  -p 8083:8083 \
  -e SEMINSTRUCT_SHIMMY_URL=http://shimmy:8080 \
  --link shimmy \
  seminstruct:latest
```

## Project Structure

```shell
seminstruct/
├── Cargo.toml              # Dependencies (axum, reqwest, etc.)
├── src/main.rs             # HTTP proxy + shimmy client
├── Dockerfile              # 2-stage build
├── docker-compose.yml      # seminstruct + shimmy stack
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
**Backend**: shimmy (Mistral-7B-Instruct)
**API**: OpenAI-compatible `/v1/chat/completions`
