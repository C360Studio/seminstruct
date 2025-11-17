# Multi-stage build for semsummarize service
# Supports linux/amd64 and linux/arm64
# Uses Candle + Hugging Face Hub for T5/BART summarization models

# Stage 1: Cargo chef planner
FROM rust:1.85-slim AS chef
# Install build dependencies for OpenSSL and C++
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    g++ \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef
WORKDIR /app

# Stage 2: Prepare dependencies
FROM chef AS planner
COPY Cargo.toml ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Build dependencies (cached layer)
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Stage 4: Build application
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release --bin semsummarize

# Stage 5: Runtime image
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash semsummarize

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/semsummarize /usr/local/bin/semsummarize

# Create cache directory with proper ownership
RUN mkdir -p /home/semsummarize/.cache/huggingface && \
    chown -R semsummarize:semsummarize /home/semsummarize/.cache && \
    chown -R semsummarize:semsummarize /app

# Switch to non-root user
USER semsummarize

# Environment variables with defaults
ENV SEMSUMMARIZE_MODEL=google/flan-t5-small
ENV SEMSUMMARIZE_PORT=8083
ENV RUST_LOG=info

# Expose service port
EXPOSE 8083

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8083/health || exit 1

# Run the service
# Note: HuggingFace Hub will download the model on first startup to ~/.cache/huggingface
ENTRYPOINT ["/usr/local/bin/semsummarize"]
