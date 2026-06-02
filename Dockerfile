# Stage 1: build
# Rust 1.85+ required: transitive deps (e.g. time-macros 0.2.27) need edition 2024.
FROM rust:1.94-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates wget && rm -rf /var/lib/apt/lists/*
COPY litegen-core/Cargo.toml litegen-core/Cargo.lock litegen-core/
COPY litegen-core/src litegen-core/src
COPY litegen-core/migrations litegen-core/migrations
COPY models models
WORKDIR /build/litegen-core
RUN cargo build --release --bin litegen

# Stage 2: runtime
# Must match the builder's Debian release (trixie) so glibc versions line up —
# a bookworm runtime (glibc 2.36) can't run a binary built on trixie (glibc 2.41).
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates sqlite3 wget && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /build/litegen-core/target/release/litegen /app/litegen
COPY models /app/models
ENV LITEGEN_MODELS_DIR=/app/models \
    LITEGEN__SERVER__HOST=0.0.0.0 \
    LITEGEN__SERVER__PORT=4000
EXPOSE 4000
USER 1000:1000
ENTRYPOINT ["/app/litegen"]
