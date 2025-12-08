# SemInstruct Quick Start

**No build required - pre-built images from GHCR!**

## Prerequisites

- Docker (with 2GB+ available memory)
- [Task](https://taskfile.dev/#/installation) (optional but recommended)

```bash
# Install Task
brew install go-task                  # macOS
snap install task --classic           # Ubuntu/Linux
go install github.com/go-task/task/v3/cmd/task@latest  # Go
```

## Quick Start

```bash
cd seminstruct

# 1. Start services (pulls pre-built images)
docker compose up -d

# 2. Wait for services to be ready (~30s)
docker compose logs -f shimmy

# 3. Test chat completions (OpenAI-compatible)
curl http://localhost:8083/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "test",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'

# 4. View logs
docker compose logs -f

# 5. Clean up
docker compose down
```

No build required - images are pulled from `ghcr.io/c360studio/semshimmy`.

## Custom Models

Default image uses Qwen2.5-0.5B (~491MB). For larger models, build with Dockerfile.shimmy:

| Resources | Model | Size | Notes |
|-----------|-------|------|-------|
| Edge / <1GB RAM | Qwen2.5-0.5B Q4_K_M | ~491MB | Default (pre-built) |
| ~4GB RAM | Mistral-7B Q4_K_M | ~4.1GB | Good balance |
| ~6GB+ RAM | Mistral-7B Q6_K | ~5.5GB | Higher quality |

```bash
# Build custom model image
MODEL_REPO=TheBloke/Mistral-7B-Instruct-v0.2-GGUF \
MODEL_FILE=mistral-7b-instruct-v0.2.Q4_K_M.gguf \
docker build -f Dockerfile.shimmy -t shimmy:custom .

# Then update docker-compose.yml to use shimmy:custom
```

## Architecture

```
┌─────────────────────────────────────┐
│         seminstruct:8083            │  Lightweight proxy (~256MB)
└──────────────┬──────────────────────┘
               │ HTTP
               ▼
┌─────────────────────────────────────┐
│          shimmy:11435               │  Inference backend (~1-26GB)
└─────────────────────────────────────┘
```

## Docker Compose Workflow

```bash
# Start both services (pulls pre-built images)
docker compose up -d

# Check shimmy status
docker compose logs -f shimmy

# Check seminstruct status
docker compose logs -f seminstruct

# Stop
docker compose down
```

## Troubleshooting

### Service won't start

```bash
# Check shimmy logs (model download/loading)
docker compose logs shimmy

# Check seminstruct logs (proxy errors)
docker compose logs seminstruct

# Restart everything
docker compose down
docker compose up -d
```

### Shimmy not healthy

```bash
# Check shimmy logs
docker compose logs -f shimmy

# Check health directly
curl http://localhost:11435/health
```

### Port already in use

```bash
# Find what's using ports
lsof -i :8083  # seminstruct
lsof -i :11435  # shimmy

# Change ports in docker-compose.yml if needed
```

## Quick Reference

| Service | Port | Purpose |
|---------|------|---------|
| seminstruct | 8083 | OpenAI-compatible proxy |
| shimmy | 11435 | Inference backend |

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/health` | GET | Health check (includes shimmy status) |
| `/v1/models` | GET | List available models |
| `/v1/chat/completions` | POST | OpenAI-compatible chat |
| `/metrics` | GET | Prometheus metrics |

**Service URL**: `http://localhost:8083`

**Backend**: shimmy (Qwen2.5-0.5B default, configurable)

**Expected Latency**: 300-500ms per response

**Memory Usage**:

- seminstruct: ~256MB
- shimmy: ~4-10GB (depends on quantization: Q2_K ~4GB, Q8_0 ~10GB)

## Before You Push

Always run integration tests locally before pushing:

```bash
task integration    # Full build + test + cleanup
```

## Next Steps

- Read [README.md](./README.md) for full documentation
- Integrate with SemStreams via HTTP client
