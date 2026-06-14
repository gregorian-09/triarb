# syntax=docker/dockerfile:1
FROM rust:1.85-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    openssl ca-certificates pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY ta-core/Cargo.toml ta-core/
COPY ta-config/Cargo.toml ta-config/
COPY ta-feed/Cargo.toml ta-feed/
COPY ta-detect/Cargo.toml ta-detect/
COPY ta-exec/Cargo.toml ta-exec/
COPY ta-sim/Cargo.toml ta-sim/
COPY ta-bin/Cargo.toml ta-bin/

# Build dependencies first (cached layer)
RUN mkdir -p ta-core/src ta-config/src ta-feed/src ta-detect/src ta-exec/src ta-sim/src ta-bin/src && \
    echo "fn main() {}" > ta-bin/src/main.rs && \
    for d in ta-core ta-config ta-feed ta-detect ta-exec ta-sim; do \
        echo "pub fn dummy() {}" > $d/src/lib.rs; \
    done && \
    cargo build --release -p ta-bin 2>/dev/null || true

# Real source
COPY . .
RUN touch ta-bin/src/main.rs ta-core/src/lib.rs ta-config/src/lib.rs ta-feed/src/lib.rs \
    ta-detect/src/lib.rs ta-exec/src/lib.rs ta-sim/src/lib.rs
RUN cargo build --release -p ta-bin

# ---- Runtime image ----
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    openssl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -u 1000 tabot
USER tabot
WORKDIR /app

COPY --from=builder /app/target/release/ta-bin /app/ta-bin
COPY production.toml /app/production.toml

EXPOSE 9100
HEALTHCHECK --interval=5s --timeout=3s --start-period=5s --retries=3 \
    CMD wget -qO- http://localhost:9100/health || exit 1

ENTRYPOINT ["/app/ta-bin", "--config", "/app/production.toml"]
