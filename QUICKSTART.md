# SemInstruct Quick Start

**No Rust installation needed - everything runs in Docker!**

## Prerequisites

- Docker (with 8GB+ available memory for shimmy model)
- [Task](https://taskfile.dev/#/installation) (optional but recommended)

```bash
# Install Task
brew install go-task                  # macOS
snap install task --classic           # Ubuntu/Linux
go install github.com/go-task/task/v3/cmd/task@latest  # Go
```

## 5-Minute Quick Start

```bash
cd seminstruct

# 1. Build and run service (starts both seminstruct and shimmy)
docker compose up -d

# 2. Wait for shimmy to download model (~4GB first time)
docker compose logs -f shimmy

# 3. Test chat completions (OpenAI-compatible)
curl http://localhost:8083/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "mistral-7b-instruct",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'

# 4. View logs
docker compose logs -f

# 5. Clean up
docker compose down
```

## Architecture

```
┌─────────────────────────────────────┐
│         seminstruct:8083            │  Lightweight proxy (~256MB)
└──────────────┬──────────────────────┘
               │ HTTP
               ▼
┌─────────────────────────────────────┐
│           shimmy:8080               │  Inference backend (~6-8GB)
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
curl http://localhost:8080/health
```

### Port already in use

```bash
# Find what's using ports
lsof -i :8083  # seminstruct
lsof -i :8080  # shimmy

# Change ports in docker-compose.yml if needed
```

## Quick Reference

| Service | Port | Purpose |
|---------|------|---------|
| seminstruct | 8083 | OpenAI-compatible proxy |
| shimmy | 8080 | Inference backend |

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/health` | GET | Health check (includes shimmy status) |
| `/v1/models` | GET | List available models |
| `/v1/chat/completions` | POST | OpenAI-compatible chat |
| `/metrics` | GET | Prometheus metrics |

**Service URL**: `http://localhost:8083`

**Backend**: shimmy (Mistral-7B-Instruct)

**Expected Latency**: 300-500ms per response

**Memory Usage**:

- seminstruct: ~256MB
- shimmy: ~6-8GB (includes model)

## Next Steps

- Read [README.md](./README.md) for full documentation
- Integrate with SemStreams via HTTP client
