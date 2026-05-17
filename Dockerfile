# syntax=docker/dockerfile:1.6

FROM rust:1.85-slim-bookworm AS builder

# Build dependencies first for caching: copy manifests, fetch deps,
# then copy sources and build.
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY rust-toolchain.toml Cargo.toml ./
COPY crates/shoebox-common/Cargo.toml crates/shoebox-common/Cargo.toml
COPY crates/shoebox-server/Cargo.toml crates/shoebox-server/Cargo.toml

# Stub sources so `cargo fetch` works without real code.
RUN mkdir -p crates/shoebox-common/src crates/shoebox-server/src/migrations \
    && echo "fn main() {}" > crates/shoebox-server/src/main.rs \
    && echo "" > crates/shoebox-common/src/lib.rs \
    && cargo fetch

COPY crates ./crates
RUN cargo build --release -p shoebox-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates wget \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /var/lib/shoebox shoebox

COPY --from=builder /build/target/release/shoebox-server /usr/local/bin/shoebox-server

USER shoebox
WORKDIR /var/lib/shoebox
EXPOSE 9000
EXPOSE 9001
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD wget -qO- http://127.0.0.1:9001/health || exit 1
ENV SHOEBOX_BIND_ADDR=0.0.0.0:9000 \
    SHOEBOX_DATA_DIR=/var/lib/shoebox \
    SHOEBOX_PHOTOS_DIR=/photos \
    SHOEBOX_CACHE_DIR=/shoebox-cache

ENTRYPOINT ["/usr/local/bin/shoebox-server"]
