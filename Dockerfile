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
ARG VERSION=dev
ARG VCS_REF=unknown

LABEL org.opencontainers.image.title="LLM Universal Proxy" \
  org.opencontainers.image.description="Single-binary LLM HTTP protocol proxy" \
  org.opencontainers.image.source="https://github.com/lzjever/llm-universal-proxy" \
  org.opencontainers.image.url="https://github.com/lzjever/llm-universal-proxy" \
  org.opencontainers.image.licenses="MIT" \
  org.opencontainers.image.version="${VERSION}" \
  org.opencontainers.image.revision="${VCS_REF}"

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && groupadd --gid 10001 llmup \
  && useradd --uid 10001 --gid llmup --home-dir /nonexistent --shell /usr/sbin/nologin --no-create-home llmup \
  && mkdir -p /etc/llmup \
  && chown -R llmup:llmup /etc/llmup \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/llm-universal-proxy /usr/local/bin/llm-universal-proxy

USER llmup:llmup
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 CMD curl -fsS http://127.0.0.1:8080/health || exit 1
ENTRYPOINT ["/usr/local/bin/llm-universal-proxy"]
CMD ["--config", "/etc/llmup/config.yaml"]
