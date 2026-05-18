# syntax=docker/dockerfile:1.6

FROM rust:1.95-slim-bookworm AS builder

# Build dependencies first for caching: copy manifests, fetch deps,
# then copy sources and build.
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY rust-toolchain.toml Cargo.toml ./
COPY crates/shoebox-common/Cargo.toml crates/shoebox-common/Cargo.toml
COPY crates/shoebox-server/Cargo.toml crates/shoebox-server/Cargo.toml
COPY crates/shoebox-client/Cargo.toml crates/shoebox-client/Cargo.toml

# Stub sources so `cargo fetch` works without real code.
RUN mkdir -p crates/shoebox-common/src \
              crates/shoebox-server/src/migrations \
              crates/shoebox-client/src \
    && echo "fn main() {}" > crates/shoebox-server/src/main.rs \
    && echo "" > crates/shoebox-common/src/lib.rs \
    && echo "" > crates/shoebox-client/src/lib.rs \
    && cargo fetch

COPY crates ./crates
RUN cargo build --release -p shoebox-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates wget xz-utils \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /var/lib/shoebox shoebox

# Install sqld so shoebox-server can spawn it as a subprocess (Plan 1.3).
# Pin a specific release for reproducibility; verify against the published sha256.
# Per-arch URLs and sums are selected via TARGETARCH at buildx time.
ARG SQLD_VERSION=v0.24.32
ARG SQLD_AMD64_SHA256=71720fc8648c19efef416efebd47145ef59b62e198770533530a858e1336879f
ARG SQLD_ARM64_SHA256=37f9eee45b388a30192907ecf4565b93df945c079331657073b5b3caf8bb1cd0
ARG TARGETARCH
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) sqld_target=x86_64-unknown-linux-gnu;  sha=${SQLD_AMD64_SHA256} ;; \
      arm64) sqld_target=aarch64-unknown-linux-gnu; sha=${SQLD_ARM64_SHA256} ;; \
      *) echo "unsupported TARGETARCH=${TARGETARCH}"; exit 1 ;; \
    esac; \
    cd /tmp; \
    asset="libsql-server-${sqld_target}.tar.xz"; \
    wget -q "https://github.com/tursodatabase/libsql/releases/download/libsql-server-${SQLD_VERSION}/${asset}"; \
    echo "${sha}  ${asset}" | sha256sum -c -; \
    tar -xJf "${asset}"; \
    mv "libsql-server-${sqld_target}/sqld" /usr/local/bin/sqld; \
    chmod +x /usr/local/bin/sqld; \
    rm -rf "${asset}" "libsql-server-${sqld_target}"

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
