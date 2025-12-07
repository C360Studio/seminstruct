# SemInstruct Quick Start

**No Rust installation needed - everything runs in Docker!**

## Prerequisites

- Docker (with 2GB+ available memory for default model)
- [Task](https://taskfile.dev/#/installation) (optional but recommended)

```bash
# Install Task
brew install go-task                  # macOS
snap install task --classic           # Ubuntu/Linux
go install github.com/go-task/task/v3/cmd/task@latest  # Go
```

## Quick Start

> **First run takes longer** (~10-15 min) to build shimmy from source and download model.
> Subsequent runs are fast with cached builds.

```bash
cd seminstruct

# 1. Build and run (builds shimmy from source + starts services)
docker compose up -d

# 2. Wait for shimmy to download model
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

## Model Sizes

Default is TinyLlama-1.1B (~660MB) for CI. Override for production:

| Tier | Model | Size | Usage |
|------|-------|------|-------|
| CI (default) | TinyLlama-1.1B Q4_K_M | ~660MB | Default, fits GitHub Actions |
| Dev | Mistral-7B Q4_K_M | ~4.1GB | Good balance for local dev |
| Prod | Mistral-7B Q6_K | ~5.5GB | Production quality |

```bash
# Default (TinyLlama for CI)
docker compose build

# Production with Mistral-7B
MODEL_REPO=TheBloke/Mistral-7B-Instruct-v0.2-GGUF \
MODEL_FILE=mistral-7b-instruct-v0.2.Q4_K_M.gguf \
docker compose build
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
# Start both services
docker compose up -d

# Check shimmy status (model loading)
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
# Wait for model download (~4GB, can take several minutes)
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

**Backend**: shimmy (Mistral-7B-Instruct-v0.2 GGUF)

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
