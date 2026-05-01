# seminstruct - llama-server (llama.cpp) image with a GGUF model baked in.
#
# This repo is the image factory for the SemStreams instruct tier (chat
# completions, classification, summarization). semembed is the parallel
# project for the embedding tier. Together they cover the LLM workloads
# that semstreams's model_registry dispatches to.
#
# What's inside:
#   - llama.cpp's `llama-server` (built from a pinned tag, multi-arch)
#   - One GGUF baked into /models/model.gguf
#   - Concurrent inference: -np 4 -cb (4 parallel slots, continuous
#     batching). The fix for stacked graph-search requests timing out.
#
# Default model: Qwen3-0.6B Q4_K_M (~440MB) - hot tier (intent + classify).
# CI also publishes :qwen3-1.7b for the quality tier (community summary,
# answer synthesis). See README's "Image Variants" and "Deployment
# Patterns" sections.
#
# Custom builds via build args:
#
#   docker build -t seminstruct:mistral \
#     --build-arg MODEL_REPO=TheBloke/Mistral-7B-Instruct-v0.2-GGUF \
#     --build-arg MODEL_FILE=mistral-7b-instruct-v0.2.Q4_K_M.gguf \
#     --build-arg MODEL_ALIAS=mistral-7b \
#     --build-arg MODEL_REASONING=off .

# ============================================================================
# Stage 1: Build llama-server from source
# ============================================================================
FROM debian:bookworm-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    cmake \
    git \
    ca-certificates \
    libcurl4-openssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Pin to a specific build tag for reproducibility. Bump intentionally.
# llama.cpp uses bNNNN build-number tags (immutable).
ARG LLAMA_CPP_TAG=b8994

RUN git clone --depth 1 --branch ${LLAMA_CPP_TAG} \
    https://github.com/ggml-org/llama.cpp.git .

# GGML_NATIVE=OFF: portable build (works for amd64 + arm64, no -march=native).
# LLAMA_CURL=ON: enables -hf model fetching, needed by the server target.
# -j2 caps parallelism: llama.cpp's heavier translation units consume
# ~1-2GB each in cc1plus, and the default `-j` (all cores) ooms on Docker
# Desktop and 4-core CI runners. The build time trade is worth it.
RUN cmake -B build \
    -DGGML_NATIVE=OFF \
    -DLLAMA_CURL=ON \
    -DCMAKE_BUILD_TYPE=Release \
    && cmake --build build --config Release -j2 --target llama-server

# ============================================================================
# Stage 2: Download GGUF model
# ============================================================================
FROM python:3.11-slim AS model-downloader

RUN pip install --no-cache-dir huggingface-hub

WORKDIR /models

ARG MODEL_REPO=unsloth/Qwen3-0.6B-GGUF
ARG MODEL_FILE=Qwen3-0.6B-Q4_K_M.gguf

# Download then rename to a fixed path so the runtime CMD doesn't depend
# on the build-arg filename.
RUN python -c "from huggingface_hub import hf_hub_download; \
    hf_hub_download('${MODEL_REPO}', '${MODEL_FILE}', \
                    local_dir='.', local_dir_use_symlinks=False)" \
    && mv "${MODEL_FILE}" model.gguf

# ============================================================================
# Stage 3: Runtime
# ============================================================================
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libcurl4 \
    libgomp1 \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the llama-server binary plus its shared libs from the builder.
COPY --from=builder /build/build/bin/llama-server /usr/local/bin/llama-server
COPY --from=builder /build/build/bin/*.so* /usr/local/lib/
RUN ldconfig

# Copy the baked-in model.
COPY --from=model-downloader /models/model.gguf /models/model.gguf

EXPOSE 8083

HEALTHCHECK --interval=30s --timeout=10s --retries=3 --start-period=30s \
    CMD curl -f http://localhost:8083/health || exit 1

# Build-time → runtime knobs. Each ARG becomes an ENV with the same name,
# so operators can also override at runtime via `-e VAR=...`.
#
# MODEL_ALIAS: stable model id surfaced in /v1/models. CI matrix sets
#   per-variant (qwen3-0.6b, qwen3-1.7b, ...). Operators can rename
#   without rebuilding.
#
# MODEL_REASONING: Qwen3 has hybrid thinking. Default `off` because the
#   hot-tier deployment (classification, intent, defaults.model) needs
#   short, latency-bounded responses, not 100+ tokens of chain-of-thought
#   prefix. The quality tier (:qwen3-1.7b) overrides to `auto` so it can
#   think on harder summary work where latency cost is acceptable.
#   Values: on | off | auto. See `llama-server --help`.
ARG MODEL_ALIAS=qwen3-0.6b
ARG MODEL_REASONING=off
ENV MODEL_ALIAS=${MODEL_ALIAS}
ENV MODEL_REASONING=${MODEL_REASONING}

# Flags:
#   --host/--port: bind 0.0.0.0:8083 (the canonical seminstruct port).
#   -m: path to baked-in GGUF.
#   --alias: stable model id in /v1/models.
#   --reasoning: Qwen3 thinking on/off/auto.
#   -np 4: four parallel inference slots — the concurrency win.
#   -cb: continuous batching across slots (group prefill + decode work).
#   -c 2048: context size per slot (8192 total). Plenty for the small
#            tier models and keeps memory bounded.
#
# Shell-form CMD with `exec` so the env vars expand at runtime while
# llama-server still becomes PID 1 for clean signal handling.
CMD ["sh", "-c", "exec llama-server \
    --host 0.0.0.0 \
    --port 8083 \
    -m /models/model.gguf \
    --alias ${MODEL_ALIAS} \
    --reasoning ${MODEL_REASONING} \
    -np 4 \
    -cb \
    -c 2048"]
