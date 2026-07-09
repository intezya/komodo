## Builds the Komodo Core, Periphery, and Util binaries
## for a specific architecture. Requires OpenSSL 3 or later.

# syntax=docker/dockerfile:1

FROM rust:1.95.0-bookworm AS builder

WORKDIR /builder
COPY Cargo.toml Cargo.lock ./
COPY ./lib ./lib
COPY ./client/core/rs ./client/core/rs
COPY ./client/periphery ./client/periphery
COPY ./bin/core ./bin/core
COPY ./bin/periphery ./bin/periphery
COPY ./bin/cli ./bin/cli
COPY ./xtask ./xtask

# Compile bin
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
  --mount=type=cache,target=/builder/target,sharing=locked \
  cargo build --release \
    -p komodo_core \
    -p komodo_periphery \
    -p komodo_cli && \
  strip \
    target/release/core \
    target/release/periphery \
    target/release/km && \
  mkdir -p /dist && \
  cp \
    target/release/core \
    target/release/periphery \
    target/release/km \
    /dist/

# Copy just the binaries to scratch image
FROM scratch

COPY --from=builder /dist/core /core
COPY --from=builder /dist/periphery /periphery
COPY --from=builder /dist/km /km

LABEL org.opencontainers.image.source="https://github.com/intezya/komodo"
LABEL org.opencontainers.image.description="Komodo Binaries"
LABEL org.opencontainers.image.licenses="GPL-3.0"
