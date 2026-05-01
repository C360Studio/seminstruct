# seminstruct

**OpenAI-compatible instruct-tier inference for SemStreams.**

`seminstruct` is the image factory for the SemStreams *instruct tier* —
chat completions, classification, summarization. Each published image is
[llama.cpp](https://github.com/ggml-org/llama.cpp)'s `llama-server` with
a curated GGUF baked in, configured for concurrent inference (`-np 4 -cb`)
and an OpenAI-compatible HTTP API on port `8083`.

The parallel project for the embedding tier is
[semembed](https://github.com/c360studio/semembed). Together they cover
the LLM workloads that semstreams's `model_registry` dispatches to.

**Status**: Alpha &nbsp;&nbsp; **Port**: `8083` &nbsp;&nbsp; **API**: OpenAI-compatible `/v1/chat/completions`

## Quick Start

```bash
docker compose up -d
# wait ~30s for the model to load
curl http://localhost:8083/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen3-0.6b",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

The default `compose` pulls `ghcr.io/c360studio/seminstruct:latest` (the
hot tier — Qwen3-0.6B Q4_K_M, `--reasoning off`). To run a different
tier, edit `docker-compose.yml` to point at `:qwen3-1.7b` or `:qwen3-8b`.

## Why this shape

SemStreams nodes call LLMs from inside stream processing — they have all
the context they need from the data flowing through the graph. They
don't need MCP-style tool discovery or conversation state. They need a
fast, OpenAI-compatible HTTP endpoint with concurrent inference so that
background batch work (community summaries) doesn't starve user-facing
classification work behind it.

`seminstruct` doesn't add a proxy layer — it ships llama-server directly
in a small image with a baked GGUF and the right flags. Routing,
retries, timeouts, and rate limiting live in semstreams's
`model_registry` on the caller side; that's the right place for them.

## Image Variants

CI publishes three variants to `ghcr.io/c360studio/seminstruct`, tagged
by **the model inside**:

| Tag | Model | Image | Memory (np=4, ctx=2048) | reasoning | Best for |
|---|---|---|---|---|---|
| `:qwen3-0.6b` (also `:latest`) | Qwen3-0.6B Q4_K_M | ~600MB | ~1.0GB | `off` | Hot path: intent + classify, `defaults.model`, every-message latency |
| `:qwen3-1.7b` | Qwen3-1.7B Q4_K_M | ~1.4GB | ~2.0GB | `auto` | Quality tier: community summaries, answer synthesis, anomaly review |
| `:qwen3-8b` | Qwen3-8B Q4_K_M | ~5.3GB | ~6.0GB | `auto` | Premium: deployments with the memory budget for higher answer quality |

Tagging by model name (not deployment role) is intentional — the tag
tells you what's *inside*; "hot" or "quality" or "premium" is deployment
intent that lives in your compose / registry config. `:latest` aliases
the 0.6B variant because that's the most common single-endpoint
deployment.

**Qwen3 thinking**: Qwen3 is a hybrid model that emits chain-of-thought
inside `<think>...</think>` tags (separated into `message.reasoning_content`
on the response). The hot image bakes `--reasoning off` because intent
classification doesn't benefit from 100+ tokens of thinking prefix; the
quality + premium images bake `--reasoning auto` so they think on harder
summary work. Override per-deployment with `-e MODEL_REASONING=on|off|auto`.

For other models (Mistral, Llama, etc.) build locally with build args:

```bash
docker build -t seminstruct:mistral \
  --build-arg MODEL_REPO=TheBloke/Mistral-7B-Instruct-v0.2-GGUF \
  --build-arg MODEL_FILE=mistral-7b-instruct-v0.2.Q4_K_M.gguf \
  --build-arg MODEL_ALIAS=mistral-7b \
  --build-arg MODEL_REASONING=off .
```

## API Reference

This is `llama-server`'s native OpenAI-compatible API. Key endpoints
SemStreams cares about:

### POST /v1/chat/completions

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

### GET /v1/models

Returns the baked-in model under its alias (e.g. `qwen3-0.6b`).

### GET /health

llama-server's native health endpoint. Returns 200 once the model is
loaded and the server is ready to serve requests.

For Kubernetes readiness probes, use `/health` directly on port 8083 —
once it returns 200, llama-server has loaded the GGUF and is accepting
requests.

## Configuration

Runtime overrides (all read from process env; the image bakes defaults
matching the variant's tier):

| Variable | Default in `:latest` | Description |
|---|---|---|
| `MODEL_ALIAS` | `qwen3-0.6b` | Model id surfaced in `/v1/models`. Operators can rename without rebuilding. |
| `MODEL_REASONING` | `off` | Qwen3 thinking: `on` \| `off` \| `auto`. |

Build-time arguments (set via `--build-arg`):

| Variable | Default | Description |
|---|---|---|
| `MODEL_REPO` | `unsloth/Qwen3-0.6B-GGUF` | Hugging Face repo containing the GGUF |
| `MODEL_FILE` | `Qwen3-0.6B-Q4_K_M.gguf` | GGUF filename within the repo |
| `MODEL_ALIAS` | `qwen3-0.6b` | Bakes the default `--alias` |
| `MODEL_REASONING` | `off` | Bakes the default `--reasoning` |
| `LLAMA_CPP_TAG` | `b8994` | llama.cpp build tag (immutable) |

## Deployment Patterns

> **One seminstruct = one model.** Each container runs llama-server with
> exactly one GGUF. To run multiple tiers, deploy multiple seminstruct
> containers (one per tier) on different ports and let the caller —
> typically SemStreams's `model_registry` — pick which URL to hit per
> capability. The registry routes; seminstruct serves.

For the simplest case, run one `seminstruct:latest` and point everything
at it. That works fine until two workload classes start sharing the
inference queue and the cheap one (e.g. intent classification) starves
behind the expensive one (e.g. graph community summaries). The fix is to
deploy a second container on a different port and route by capability.

### Three-tier reference deployment

The shape we recommend for SemStreams workloads with the new model
registry (capabilities → endpoints):

```text
                  ┌── seminstruct-hot      :8083 ── :qwen3-0.6b   reasoning=off
semstreams ──────┤
(model_registry)  ├── seminstruct-quality  :8084 ── :qwen3-1.7b   reasoning=auto
                  │
                  ├── seminstruct-premium  :8085 ── :qwen3-8b     reasoning=auto
                  │   (optional, when memory budget allows)
                  │
                  └── semembed             :8081  (embedding tier, separate project)
```

Each `seminstruct-*` deployment is an independent container running its
own llama-server. The host-side port mapping is what differs between
them; inside the container they all bind 8083.

| Capability route | Image tag | Model id (`--alias`) | Capabilities to route here |
|---|---|---|---|
| Hot | `:qwen3-0.6b` (= `:latest`) | `qwen3-0.6b` | `query_classification`, intent classification (hidden), `defaults.model` |
| Quality | `:qwen3-1.7b` | `qwen3-1.7b` | `community_summary`, `answer_synthesis` fallback, anomaly review (piggyback) |
| Premium (optional) | `:qwen3-8b` | `qwen3-8b` | High-stakes `answer_synthesis`, hard summaries when latency budget allows |
| Embedding | `semembed:latest` | n/a | `embedding` |

### Two operator notes that bite if missed

1. **Point `defaults.model` at the hot deployment, not quality or
   premium.** Several SemStreams call sites (intent classification on
   every user message, onboarding layer normalization) currently fall
   through to `defaults.model` — they have no capability constant yet.
   Putting `defaults.model` on a heavier endpoint means every user
   message queues behind background summary work, which is the failure
   mode this concurrency story exists to prevent.

2. **Anomaly relationship review piggybacks on `community_summary`.**
   It's not its own capability — it shares the LLMClient injected into
   graph-clustering. Whatever endpoint owns `community_summary` will
   also carry anomaly review load; size accordingly and don't be
   surprised when its request rate is higher than `community_summary`
   alone would predict.

## Resource Use

### Per-container budget

For a single seminstruct container running with `-np 4 -cb -c 2048`:

| Component | :qwen3-0.6b | :qwen3-1.7b | :qwen3-8b |
|---|---|---|---|
| Model weights (Q4_K_M) | ~430MB | ~1.1GB | ~4.7GB |
| KV cache (4 slots × 2048 ctx) | ~110MB | ~250MB | ~700MB |
| Compute / activations / overhead | ~400MB | ~600MB | ~600MB |
| **Process total** | **~1.0GB** | **~2.0GB** | **~6.0GB** |
| Container image on disk | ~600MB | ~1.4GB | ~5.3GB |

### CPU under concurrency

`-np 4` means four logical inference slots, **not** four parallel forward
passes. With `-cb` (continuous batching) llama-server batches active slot
requests into a single forward pass per step — throughput goes up,
per-token latency stays roughly constant for the best slot, and the
worst-case slot only pays a small batching overhead. So:

- 4 cores is a reasonable floor for `:qwen3-0.6b`; 8 cores comfortable.
- 8 cores is a reasonable floor for `:qwen3-1.7b` on CPU.
- 8B-class models really want a GPU; on CPU expect single-digit tok/s
  per slot with 8-16 cores.

If you raise `-np`, also raise `-c` only if you need longer per-request
context — KV cache scales linearly with `slots × ctx`.

### Three-tier reference total

Co-locating the full reference deployment on a single host:

| Component | Memory | Image |
|---|---|---|
| seminstruct hot (qwen3-0.6b) | ~1.0GB | ~600MB |
| seminstruct quality (qwen3-1.7b) | ~2.0GB | ~1.4GB |
| seminstruct premium (qwen3-8b, optional) | ~6.0GB | ~5.3GB |
| semembed | ~512MB | ~1GB |
| **Total without premium** | **~3.5GB** | **~3GB on disk** |
| **Total with premium** | **~9.5GB** | **~8.3GB on disk** |

Without premium: comfortable on 4-core / 8GB. With premium: 16GB host
minimum, GPU strongly recommended for premium tier.

## Migration Notes

### From the previous `seminstruct` HTTP proxy

Earlier versions of this repo shipped a Rust HTTP proxy at port 8083 in
front of a separate `semshimmy` / `semserve` backend. The proxy has been
removed; `seminstruct` is now the image factory for `llama-server`
directly, and the published `:latest` tag is the llama-server image, not
the Rust proxy.

For consumers:

- **HTTP API**: same OpenAI-compatible contract on the same port (8083).
  No client changes.
- **Compose / deployment**: the old two-service shape (proxy + backend)
  collapses to a single seminstruct service. Pull `:latest` and you get
  llama-server-in-a-container; restart any running containers off the
  old `:latest` to pick up the new image.
- **Caller-side reliability**: retry, timeouts, rate limiting now live
  in semstreams's `model_registry` on the caller side. The proxy used
  to handle these; that responsibility moved up the stack.
- **Old `:latest` digests**: still pullable by digest if you pin
  explicitly, but the moving `:latest` tag now points at the new image.

### From `ghcr.io/c360studio/semserve:*`

The `semserve` image namespace was a transient name during this sprint
(while seminstruct was still a proxy). It's been collapsed into
`seminstruct`:

| Was | Becomes |
|---|---|
| `ghcr.io/c360studio/semserve:qwen3-0.6b` | `ghcr.io/c360studio/seminstruct:qwen3-0.6b` |
| `ghcr.io/c360studio/semserve:qwen3-1.7b` | `ghcr.io/c360studio/seminstruct:qwen3-1.7b` |
| (new) | `ghcr.io/c360studio/seminstruct:qwen3-8b` |
| `SEMSERVE_ALIAS` build arg / env | `MODEL_ALIAS` |
| `SEMSERVE_REASONING` | `MODEL_REASONING` |
| Internal port `11435` | Single port `8083` |

## Project Structure

```shell
seminstruct/
├── Dockerfile              # llama-server + GGUF, multi-arch, multi-variant
├── docker-compose.yml      # Pulls :latest from GHCR
├── docker-compose.ci.yml   # CI override: builds from source
├── Taskfile.yml            # Common dev tasks
└── .github/workflows/ci.yml  # Build + test + matrix publish
```

That's everything. There is no source code — the Dockerfile + the CI
matrix are the product.

## License

MIT

---

**Image**: `ghcr.io/c360studio/seminstruct:{qwen3-0.6b|qwen3-1.7b|qwen3-8b|latest}`
**Port**: `8083`
**API**: OpenAI-compatible `/v1/chat/completions`
**Engine**: llama.cpp `llama-server` (build tag `b8994`)
