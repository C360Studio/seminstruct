# SemSummarize - Lightweight Text Summarization Service

**Status**: Alpha
**Package**: `semsummarize`
**Port**: `8083`

**📚 Ecosystem Documentation**: For SemStreams architecture, integration guides, and deployment strategies, see [semdocs](https://github.com/c360/semdocs). This README covers semsummarize implementation details.

## Overview

SemSummarize is a lightweight, CPU-optimized text summarization service built with Rust and Candle. It provides HTTP-based access to encoder-decoder models (T5, BART) for generating concise summaries of community descriptions, entity clusters, and other textual content.

**Key Features**:

- Pure Rust implementation (no Python runtime)
- CPU-optimized inference with Candle
- Lightweight Docker image (~200-500MB)
- Simple REST API
- Prometheus metrics
- Graceful degradation (fallback to statistical summarization)

## Containerized Development

**No local Rust required!** All development uses Docker to avoid toolchain setup.

**Quick Start**:

```bash
# Build and run service
task dev

# View logs
task logs

# Test summarization
task test:summarize

# Clean up
task clean
```

See [Taskfile.yml](./Taskfile.yml) for all available tasks.

## Architecture

```shell
semsummarize/
├── Cargo.toml              # Dependencies (Candle, Axum, HuggingFace Hub)
├── src/
│   └── main.rs             # HTTP server + model inference
├── Dockerfile              # Multi-stage build
├── docker-compose.yml      # Standalone development
├── Taskfile.yml            # Development automation
├── .github/workflows/ci.yml # CI/CD pipeline
└── README.md               # This file
```

**Technology Stack**:

- **Candle**: Hugging Face's Rust ML framework for inference
- **Axum**: Fast, ergonomic web framework
- **HuggingFace Hub**: Automatic model downloading
- **Tokenizers**: Fast tokenization library

## API Reference

### POST /summarize

Generate a summary from input text.

**Request**:

```json
{
  "input": "Community with 15 drones in SF Bay Area, average battery 78.5%, operating in delivery routes",
  "max_length": 100,
  "min_length": 10,
  "temperature": 0.7
}
```

**Response**:

```json
{
  "summary": "Southwest delivery drone cluster (15 active units)",
  "model": "google/flan-t5-small",
  "input_length": 20,
  "output_length": 7
}
```

**Error Response**:

```json
{
  "error": {
    "message": "Failed to generate summary: ...",
    "type": "internal_error"
  }
}
```

### GET /health

Health check endpoint.

**Response**:

```json
{
  "status": "healthy",
  "model": "google/flan-t5-small"
}
```

### GET /models

List loaded models.

**Response**:

```json
{
  "models": ["google/flan-t5-small"]
}
```

### GET /metrics

Prometheus metrics endpoint.

**Metrics**:

- `semsummarize_requests_total`: Total summarization requests
- `semsummarize_request_duration_seconds`: Request latency histogram
- `semsummarize_tokens_processed_total`: Total tokens processed
- `semsummarize_errors_total`: Total errors

## Configuration

Configure via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `SEMSUMMARIZE_MODEL` | `google/flan-t5-small` | HuggingFace model ID |
| `SEMSUMMARIZE_PORT` | `8083` | HTTP port |
| `RUST_LOG` | `info` | Log level (`debug`, `info`, `warn`, `error`) |

**Supported Models**:

- `google/flan-t5-small` (77M params) - Fast, CPU-friendly
- `google/flan-t5-base` (250M params) - Higher quality
- `facebook/bart-base` (140M params) - Narrative style

## Development Workflow (Task-based)

All development tasks use Docker - **no local Rust installation required**.

### Quick Commands

```bash
# Full development cycle (build + run + test)
task dev

# Build Docker image
task build

# Run service (background)
task run

# Run service (foreground, see logs)
task run:fg

# View logs
task logs

# Test endpoints
task test:health
task test:summarize
task test:all

# Restart service
task restart

# Open shell in container
task shell

# Clean up
task clean
task clean:all
```

### Docker Compose Workflow

```bash
# Start service
docker compose up -d

# View logs
docker compose logs -f

# Stop service
docker compose down
```

Or use Task shortcuts:

```bash
task compose:up
task compose:logs
task compose:down
```

## Docker Deployment

### Using Taskfile (Recommended)

```bash
# Build and run
task build
task run

# Test
task test:all

# View logs
task logs
```

### Manual Docker Commands

```bash
# Build
docker build -t semsummarize:latest .

# Run
docker run -d \
  --name semsummarize \
  -p 8083:8083 \
  -e SEMSUMMARIZE_MODEL=google/flan-t5-small \
  -e RUST_LOG=info \
  -v semsummarize-cache:/home/semsummarize/.cache/huggingface \
  semsummarize:latest
```

**Volume Mount**: Cache HuggingFace models to avoid re-downloading on restart.

### Docker Compose

Add to `docker-compose.services.yml`:

```yaml
semsummarize:
  profiles: ["summarization", "all"]
  build:
    context: ../semsummarize
    dockerfile: Dockerfile
  image: semstreams-semsummarize:latest
  container_name: semstreams-semsummarize
  ports:
    - "8083:8083"
  environment:
    - SEMSUMMARIZE_MODEL=google/flan-t5-small
    - SEMSUMMARIZE_PORT=8083
    - RUST_LOG=info
  volumes:
    - semsummarize-cache:/home/semsummarize/.cache/huggingface
  healthcheck:
    test: ["CMD", "curl", "-f", "http://localhost:8083/health"]
    interval: 30s
    timeout: 5s
    retries: 3
    start_period: 30s
  networks:
    - semstreams-services
  restart: unless-stopped
```

Start with:

```bash
docker compose -f docker-compose.services.yml --profile summarization up -d
```

## Local Development

### Containerized Development (Recommended)

**No Rust installation required!** Use Docker for all development:

```bash
# Install Task runner (if not already installed)
# macOS: brew install go-task
# Ubuntu: snap install task --classic
# Or: go install github.com/go-task/task/v3/cmd/task@latest

# Start development
task dev              # Build, run, test
task logs             # View logs
task test:summarize   # Test endpoints
task clean            # Clean up
```

See [Taskfile.yml](./Taskfile.yml) for all commands.

### Native Rust Development (Optional)

For direct Rust development without Docker:

**Prerequisites**:
- Rust 1.85+ (`rustup install stable`)
- OpenSSL development headers
- C++ compiler (for Candle)

**Ubuntu/Debian**:
```bash
sudo apt-get install pkg-config libssl-dev g++
```

**macOS**:
```bash
brew install openssl
```

**Build**:
```bash
cd semsummarize
cargo build --release
```

**Run**:
```bash
SEMSUMMARIZE_MODEL=google/flan-t5-small \
SEMSUMMARIZE_PORT=8083 \
RUST_LOG=info \
cargo run --release
```

**Test**:

```bash
# Health check
curl http://localhost:8083/health

# Summarize
curl -X POST http://localhost:8083/summarize \
  -H "Content-Type: application/json" \
  -d '{
    "input": "This is a long piece of text that needs to be summarized into a shorter form while preserving the key information.",
    "max_length": 50
  }'

# Metrics
curl http://localhost:8083/metrics
```

## Integration with SemStreams

### Go HTTP Client

```go
package graphclustering

import (
    "bytes"
    "encoding/json"
    "net/http"
    "time"
)

type SummarizerHTTP struct {
    baseURL string
    client  *http.Client
}

func NewSummarizerHTTP(baseURL string) *SummarizerHTTP {
    return &SummarizerHTTP{
        baseURL: baseURL,
        client: &http.Client{
            Timeout: 5 * time.Second,
        },
    }
}

func (s *SummarizerHTTP) Summarize(ctx context.Context, input string) (string, error) {
    req := map[string]interface{}{
        "input":      input,
        "max_length": 100,
    }

    body, _ := json.Marshal(req)
    resp, err := s.client.Post(
        s.baseURL+"/summarize",
        "application/json",
        bytes.NewReader(body),
    )
    if err != nil {
        return "", err
    }
    defer resp.Body.Close()

    var result struct {
        Summary string `json:"summary"`
    }
    json.NewDecoder(resp.Body).Decode(&result)
    return result.Summary, nil
}
```

### Configuration

```json
{
  "clustering": {
    "summarization": {
      "provider": "http",
      "http_url": "http://localhost:8083",
      "timeout": "5s",
      "fallback_to_statistical": true
    }
  }
}
```

## Performance

### Latency (CPU)

| Model | Input Tokens | P50 | P95 | P99 |
|-------|--------------|-----|-----|-----|
| flan-t5-small | 50 | 50ms | 100ms | 150ms |
| flan-t5-small | 200 | 150ms | 300ms | 450ms |
| flan-t5-base | 50 | 200ms | 400ms | 600ms |

### Memory Usage

| Model | Memory (Idle) | Memory (Active) |
|-------|---------------|-----------------|
| flan-t5-small | 200MB | 300MB |
| flan-t5-base | 600MB | 800MB |

### Throughput

- **Small model**: 10-20 summaries/sec (CPU)
- **Base model**: 3-5 summaries/sec (CPU)

## Troubleshooting

### Model Download Fails

**Problem**: Service fails to start with download errors

**Solution**:
```bash
# Check internet connectivity
curl -I https://huggingface.co

# Check disk space
df -h

# Manual download (optional)
mkdir -p ~/.cache/huggingface/hub
cd ~/.cache/huggingface/hub
git lfs install
git clone https://huggingface.co/google/flan-t5-small
```

### High Latency

**Problem**: Summarization takes >1 second

**Solutions**:
- Use smaller model (`flan-t5-small`)
- Reduce `max_length` parameter
- Check CPU resources (`docker stats`)
- Consider GPU deployment (future enhancement)

### Out of Memory

**Problem**: Service crashes with OOM errors

**Solutions**:
- Switch to smaller model
- Increase Docker memory limit
- Reduce concurrent requests
- Enable request queueing

## Comparison to Alternatives

| Feature | SemSummarize | OpenAI API | HuggingFace Inference API |
|---------|--------------|------------|---------------------------|
| **Deployment** | Self-hosted | Cloud | Cloud |
| **Cost** | Free | $0.0004/1K tokens | $0.0004/1K tokens |
| **Latency** | 50-200ms | 500-1500ms | 200-500ms |
| **Privacy** | Fully private | Data sent to OpenAI | Data sent to HF |
| **Customization** | Full control | Limited | Limited |
| **Edge Support** | Yes | No | No |

## CI/CD

GitHub Actions workflow automatically:
- Builds Docker image
- Runs health checks
- Tests all endpoints
- Security scanning (Trivy)
- Dockerfile linting (Hadolint)

**Workflow**: `.github/workflows/ci.yml`

**Run CI locally**:
```bash
task ci:test
```

## Related Documentation

- [GRAPHRAG_LESSONS_LEARNED.md](../semstreams/docs/architecture/GRAPHRAG_LESSONS_LEARNED.md) - Community summarization context
- [EMBEDDING_ARCHITECTURE.md](../semstreams/docs/architecture/EMBEDDING_ARCHITECTURE.md) - Provider pattern
- [Candle Documentation](https://github.com/huggingface/candle) - ML framework details
- [HuggingFace Hub](https://huggingface.co/docs/hub/index) - Model repository
- [Taskfile Documentation](https://taskfile.dev) - Task runner docs

## License

Same as SemStreams parent project.

---

**Implementation**: `src/main.rs`
**Docker**: `Dockerfile`
**Port**: `8083`
**Status**: Alpha (in development)
