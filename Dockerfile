ARG RUST_TOOLCHAIN=1.88.0
ARG RUST_BASE_IMAGE=rust:${RUST_TOOLCHAIN}-bookworm
ARG RUNTIME_BASE_IMAGE=debian:bookworm-slim

FROM ${RUST_BASE_IMAGE} AS builder
WORKDIR /app

COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests

RUN cargo build --locked --release

FROM ${RUNTIME_BASE_IMAGE}
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/llm-universal-proxy /usr/local/bin/llm-universal-proxy

EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/llm-universal-proxy"]
