FROM rust:1.87-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests

RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/llm-universal-proxy /usr/local/bin/llm-universal-proxy

EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/llm-universal-proxy"]

