# Multi-stage build for seminstruct service
# Lightweight proxy to shimmy for OpenAI-compatible inference
# Supports linux/amd64 and linux/arm64

# Stage 1: Build
FROM rust:1.85-slim AS builder

# Install build dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Create dummy src to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src

# Copy actual source and build
COPY src ./src
RUN touch src/main.rs && cargo build --release --bin seminstruct

# Stage 2: Runtime
FROM debian:bookworm-slim

# Install runtime dependencies (minimal)
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash seminstruct

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/seminstruct /usr/local/bin/seminstruct

# Set ownership
RUN chown -R seminstruct:seminstruct /app

# Switch to non-root user
USER seminstruct

# Environment variables with defaults
ENV SEMINSTRUCT_SHIMMY_URL=http://shimmy:8080
ENV SEMINSTRUCT_PORT=8083
ENV SEMINSTRUCT_TIMEOUT_SECONDS=120
ENV SEMINSTRUCT_MAX_RETRIES=3
ENV RUST_LOG=info

# Expose service port
EXPOSE 8083

# Health check (fast startup since no model loading)
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8083/health || exit 1

# Run the service
ENTRYPOINT ["/usr/local/bin/seminstruct"]
