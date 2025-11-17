# SemSummarize Quick Start

**No Rust installation needed - everything runs in Docker!**

## Prerequisites

- Docker
- [Task](https://taskfile.dev/#/installation) (optional but recommended)

```bash
# Install Task
brew install go-task                  # macOS
snap install task --classic           # Ubuntu/Linux
go install github.com/go-task/task/v3/cmd/task@latest  # Go
```

## 5-Minute Quick Start

```bash
cd semsummarize

# 1. Build and run service
task dev

# 2. Test summarization
curl -X POST http://localhost:8083/summarize \
  -H "Content-Type: application/json" \
  -d '{
    "input": "Community with 15 drones in SF Bay Area, avg battery 78.5%",
    "max_length": 50
  }'

# 3. View logs
task logs

# 4. Clean up
task clean
```

## Common Tasks

```bash
# Build
task build              # Build Docker image
task build:no-cache     # Rebuild from scratch

# Run
task run                # Run in background
task run:fg             # Run in foreground (see logs)
task stop               # Stop service

# Test
task test:health        # Health check
task test:summarize     # Summarization test
task test:all           # All tests

# Monitor
task logs               # Follow logs
task logs:tail          # Last 50 lines
task metrics            # View Prometheus metrics

# Development
task restart            # Restart service
task shell              # Open container shell
task dev:rebuild        # Clean + rebuild + run

# Cleanup
task clean              # Remove container
task clean:all          # Remove everything
```

## Docker Compose Workflow

```bash
# Start
docker compose up -d

# Logs
docker compose logs -f

# Stop
docker compose down
```

Or use Task shortcuts:
```bash
task compose:up
task compose:logs
task compose:down
```

## Troubleshooting

### Service won't start
```bash
# Check logs
task logs:tail

# Rebuild from scratch
task clean:all
task build:no-cache
task run
```

### Port already in use
```bash
# Find what's using port 8083
lsof -i :8083

# Kill the process or change port in docker-compose.yml
```

### Model download slow/fails
```bash
# First run downloads ~300MB model - be patient
# Check progress in logs
task logs

# If download fails, clean cache and retry
task clean:cache
task run
```

## Next Steps

- Read [README.md](./README.md) for full documentation
- Check [Taskfile.yml](./Taskfile.yml) for all available tasks
- Review [.github/workflows/ci.yml](.github/workflows/ci.yml) for CI/CD setup
- Integrate with SemStreams via HTTP client (see README)

## Integration with SemStreams

The service is already configured in `../semstreams/docker-compose.services.yml`:

```bash
# Start from semstreams directory
cd ../semstreams
docker compose -f docker-compose.services.yml --profile summarization up -d
```

Or use the Task shortcut:
```bash
cd semsummarize
task compose:up
```

## Getting Help

```bash
# List all tasks
task --list

# Show detailed help
task help
```

## Quick Reference

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/health` | GET | Health check |
| `/models` | GET | List loaded models |
| `/summarize` | POST | Generate summary |
| `/metrics` | GET | Prometheus metrics |

**Service URL**: `http://localhost:8083`

**Default Model**: `google/flan-t5-small` (77M params)

**Expected Latency**: 50-200ms per summary (CPU)

**Memory Usage**: ~300MB with small model
