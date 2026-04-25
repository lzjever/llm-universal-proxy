# Container Image and GHCR Release

This page records the converged container plan for `llmup` and gives the product-facing runtime entrypoints. The release image is:

```text
ghcr.io/lzjever/llm-universal-proxy
```

## Decisions

- Container releases publish to GHCR at `ghcr.io/lzjever/llm-universal-proxy`.
- Pull requests and `main` build an image and run container smoke, but never push.
- Release tags matching `v*` build, smoke, and push multi-arch images for `linux/amd64` and `linux/arm64`.
- Release images get `vX.Y.Z`, `X.Y.Z`, and `latest` tags. `latest` only moves on formal release tags.
- The first container release plan does not publish an `edge` tag.
- The image starts as a non-root user, exposes port `8080`, reads `/etc/llmup/config.yaml` by default, and declares a `/health` Docker `HEALTHCHECK`.
- Secrets must come from runtime environment variables or mounted secret managers. The image and examples do not bake provider keys or admin tokens into files.

## Admin Plane and Dashboard Boundary

The admin API and Web Admin Dashboard share one admin boundary:

- keep `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` as the admin credential
- do not introduce a separate service key
- dashboard login is admin-token based
- do not add multi-user accounts, readonly roles, or a complex session model in this plan

If `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` is unset, admin access is limited to loopback clients. Container deployments should normally set a non-empty token because the process runs behind Docker networking and may be exposed through an explicit port mapping.

## Run the Release Image

Use a container-oriented config whose `listen` value is `0.0.0.0:8080`, such as [examples/container-config.yaml](../examples/container-config.yaml). Do not mount the local quickstart config unchanged for container service mode: `listen: 127.0.0.1:8080` binds inside the container's own loopback namespace and will not serve traffic through the Docker port mapping.

```bash
export OPENAI_API_KEY="set-at-runtime"
export MINIMAX_API_KEY="set-at-runtime"
export LLM_UNIVERSAL_PROXY_ADMIN_TOKEN="set-a-random-admin-token"

docker run --rm --name llmup \
  -p 127.0.0.1:8080:8080 \
  -v "$PWD/examples/container-config.yaml:/etc/llmup/config.yaml:ro" \
  -e OPENAI_API_KEY \
  -e MINIMAX_API_KEY \
  -e LLM_UNIVERSAL_PROXY_ADMIN_TOKEN \
  ghcr.io/lzjever/llm-universal-proxy:latest
```

Check health:

```bash
curl -fsS http://127.0.0.1:8080/health
```

For Compose, start from [examples/docker-compose.yaml](../examples/docker-compose.yaml). It references environment variables and does not contain real secrets.

```bash
docker compose -f examples/docker-compose.yaml up
```

## Local Build and Smoke

The Makefile owns the local container loop:

```bash
make docker-build
make docker-smoke
```

`make docker-smoke` builds the local image, mounts a temporary config at `/etc/llmup/config.yaml`, sets `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` to a test value, checks the admin-token boundary, and sends a streaming request through a mock upstream.

## CI and Release Plan

CI uses the same shape as local smoke:

- `ci.yml`: build a local image, load it into Docker, and run `scripts/test_container_smoke.sh`; `push: false` is required.
- `release.yml`: build a local `linux/amd64` image for smoke first, then push the multi-arch GHCR image only when the ref is a release tag.
- Release metadata is passed through Docker build args for OCI labels: `VERSION` and `VCS_REF`.

This keeps the container path production-ready without changing the Rust server or adding secret-bearing files.
