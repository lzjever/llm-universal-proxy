# Container Image and GHCR Release

This page is the operator path for using the published `llmup` container image
from GHCR. The image repository is:

```text
ghcr.io/agentsmith-project/llm-universal-proxy
```

## Current Release

The current published container image is `v0.2.25`, as recorded in
[docs/release-artifacts/container-image.json](release-artifacts/container-image.json):

```text
ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.25
ghcr.io/agentsmith-project/llm-universal-proxy:0.2.25
ghcr.io/agentsmith-project/llm-universal-proxy:latest
ghcr.io/agentsmith-project/llm-universal-proxy@sha256:a6d5b309f25f17cafbd7fadb601fef5f80726c4a299509820e8e863be0928058
```

Cargo package version `0.2.27` is the next release identity; it is not a published container tag yet.
Do not bind `v0.2.27` to the digest above until a release workflow has pushed
that tag and refreshed the manifest with the new digest.

`latest` is a convenience tag for quick trials and local experiments. It moves
when a formal release tag is published, so production deployments should pin the
release tag or the digest below.

## GHCR Access

Treat authenticated pulls as the reliable path for this image. Do not assume the
current package is anonymously readable until GHCR package visibility has been
confirmed.

If the package is public, Docker can pull anonymously and you can skip login. If
`docker pull`, `docker manifest inspect`, or the package page returns
unauthorized, 403, or package page appears 404 while logged out, authenticate
with a personal access token (classic) that has `read:packages` access to the
package owner.

```bash
export GITHUB_USERNAME="your-github-user"
export GITHUB_TOKEN_OR_PAT="classic-pat-with-read-packages"

echo "$GITHUB_TOKEN_OR_PAT" | docker login ghcr.io -u "$GITHUB_USERNAME" --password-stdin
```

The release workflow uses the repository `GITHUB_TOKEN` with package write
permissions only while publishing. Local operators should use their own GitHub
username and a personal access token (classic) for authenticated pulls.

## Pull

Complete [GHCR Access](#ghcr-access) first if the package is private, has not
been made public yet, or your organization requires authenticated package pulls.

Pull the current release tag:

```bash
docker pull ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.25
```

Pull the immutable digest when you need the exact published artifact:

```bash
docker pull ghcr.io/agentsmith-project/llm-universal-proxy@sha256:a6d5b309f25f17cafbd7fadb601fef5f80726c4a299509820e8e863be0928058
```

## Run the Release Image

Use a container-oriented config whose `listen` value is `0.0.0.0:8080`, such as
[examples/container-config.yaml](../examples/container-config.yaml). Do not mount the local quickstart config unchanged for container service mode:
`listen: 127.0.0.1:8080` binds inside the container's own loopback namespace and
will not serve traffic through the Docker port mapping.

The sample config follows provider-neutral compatible upstreams with
`OPENAI_COMPATIBLE_API_KEY` and `ANTHROPIC_COMPATIBLE_API_KEY`. Do not use the unedited example config for real provider requests. Before sending real model traffic, replace the placeholder base URLs and model aliases in
[examples/container-config.yaml](../examples/container-config.yaml) with the
providers and models you intend to use.

```bash
export OPENAI_COMPATIBLE_API_KEY="set-at-runtime"
export ANTHROPIC_COMPATIBLE_API_KEY="set-at-runtime"
export LLM_UNIVERSAL_PROXY_ADMIN_TOKEN="set-a-random-admin-token"
export LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key
export LLM_UNIVERSAL_PROXY_KEY="set-a-random-proxy-key"

docker run --rm --name llmup \
  -p 127.0.0.1:8080:8080 \
  -v "$PWD/examples/container-config.yaml:/etc/llmup/config.yaml:ro" \
  -e OPENAI_COMPATIBLE_API_KEY \
  -e ANTHROPIC_COMPATIBLE_API_KEY \
  -e LLM_UNIVERSAL_PROXY_ADMIN_TOKEN \
  -e LLM_UNIVERSAL_PROXY_AUTH_MODE \
  -e LLM_UNIVERSAL_PROXY_KEY \
  ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.25
```

Provider/model/resource requests must send the proxy key through the normal
client SDK API-key setting or as `Authorization: Bearer <proxy-key>`. Do not
send provider keys in custom proxy headers.

## API Bootstrap

API bootstrap is the control-plane-managed path for container services. It is
available in the current published `v0.2.25` image, so a container can start
without any static namespace config and load namespaces through the Admin API.

The release image includes a built-in empty bootstrap config at
`/etc/llmup/config.yaml`:

```yaml
listen: 0.0.0.0:8080
```

Start the image without mounting `/etc/llmup/config.yaml`. No static config
mount is required; the built-in empty bootstrap config starts the process with
no namespaces. Then POST a runtime payload to
`/admin/namespaces/:namespace/config`.

Important boundaries:

- Admin API writes are not persisted by the proxy. Keep the source of truth in
  your controller, init job, or deployment system, and replay the Admin API
  write after every container restart.
- Global data-plane auth is separate from namespace config. `GET /admin/data-auth`
  returns a redacted snapshot, and `PUT /admin/data-auth`
  rotates or switches `data_auth` with CAS. These writes are also not persisted
  by the proxy, so controllers must replay them after restart when they manage
  data auth through the Admin API.
- `/health` is liveness: it only says the process is running.
- `/ready` is readiness: it returns success only after at least one namespace
  has been loaded.
- The Admin API accepts the runtime payload shape, not static YAML. In runtime
  payload shape, `upstreams` is a list, `fixed_upstream_format` names the
  upstream protocol, and aliases use `upstream_name` plus `upstream_model`.
- Set `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` for container deployments. Admin calls
  should send `Authorization: Bearer <admin-token>`.

Minimal API bootstrap run:

```bash
docker pull ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.25

export LLM_UNIVERSAL_PROXY_ADMIN_TOKEN="set-a-random-admin-token"
export LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key
export LLM_UNIVERSAL_PROXY_KEY="set-a-random-proxy-key"
export OPENAI_COMPATIBLE_API_KEY="set-at-runtime"

docker run --rm --name llmup-bootstrap \
  -p 127.0.0.1:8080:8080 \
  -e LLM_UNIVERSAL_PROXY_ADMIN_TOKEN \
  -e LLM_UNIVERSAL_PROXY_AUTH_MODE \
  -e LLM_UNIVERSAL_PROXY_KEY \
  -e OPENAI_COMPATIBLE_API_KEY \
  ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.25
```

Apply a runtime config:

```bash
curl -fsS \
  -X POST \
  -H "Authorization: Bearer $LLM_UNIVERSAL_PROXY_ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  http://127.0.0.1:8080/admin/namespaces/default/config \
  -d @- <<'JSON'
{
  "if_revision": null,
  "config": {
    "listen": "0.0.0.0:8080",
    "upstream_timeout_secs": 120,
    "upstreams": [
      {
        "name": "OPENAI-COMPATIBLE",
        "api_root": "https://openai-compatible.example/v1",
        "fixed_upstream_format": "openai-completion",
        "provider_key_env": "OPENAI_COMPATIBLE_API_KEY"
      }
    ],
    "model_aliases": {
      "coding": {
        "upstream_name": "OPENAI-COMPATIBLE",
        "upstream_model": "provider-model-id"
      }
    }
  }
}
JSON
```

After the write succeeds, `/ready` should return success:

```bash
curl -fsS http://127.0.0.1:8080/ready
```

## Verify in One Minute

This smoke path checks the published image, `/health`, and `/ready` without
making a real provider request. It mounts the example config so the process can
load a static namespace immediately. The current published `v0.2.25` image also
supports no-mount API bootstrap through the Admin API path above.

Before starting, complete [GHCR Access](#ghcr-access) if `docker pull` returns
unauthorized, 403, or 404, or if you know the package is private.

In one terminal:

```bash
export OPENAI_COMPATIBLE_API_KEY="not-used-for-health"
export ANTHROPIC_COMPATIBLE_API_KEY="not-used-for-health"
export LLM_UNIVERSAL_PROXY_ADMIN_TOKEN="local-admin-token"
export LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key
export LLM_UNIVERSAL_PROXY_KEY="local-proxy-key"

docker pull ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.25

docker run --rm --name llmup-smoke \
  -p 127.0.0.1:8080:8080 \
  -v "$PWD/examples/container-config.yaml:/etc/llmup/config.yaml:ro" \
  -e OPENAI_COMPATIBLE_API_KEY \
  -e ANTHROPIC_COMPATIBLE_API_KEY \
  -e LLM_UNIVERSAL_PROXY_ADMIN_TOKEN \
  -e LLM_UNIVERSAL_PROXY_AUTH_MODE \
  -e LLM_UNIVERSAL_PROXY_KEY \
  ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.25
```

In another terminal:

```bash
curl -fsS http://127.0.0.1:8080/health
curl -fsS http://127.0.0.1:8080/ready
```

Stop the first terminal with `Ctrl-C` when both checks return successfully.

## Production Pinning

Pin a release tag or digest for production:

```text
ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.25
ghcr.io/agentsmith-project/llm-universal-proxy@sha256:a6d5b309f25f17cafbd7fadb601fef5f80726c4a299509820e8e863be0928058
```

Use the `v0.2.25` tag when you want a readable release reference. Use the
`sha256:a6d5b309f25f17cafbd7fadb601fef5f80726c4a299509820e8e863be0928058`
digest when rollout tooling requires an immutable artifact identity.

Do not use `latest` for production pinning. Keep `latest` to quick trials,
ad-hoc verification, and convenience flows where automatic movement to the next
formal release is acceptable.

## Compose

[examples/docker-compose.yaml](../examples/docker-compose.yaml) defaults to the
published image recorded in the manifest and can be overridden with
`LLMUP_IMAGE` when you need a different pinned tag or digest. It references
environment variables and does not contain real secrets.

```bash
docker compose -f examples/docker-compose.yaml up
```

## Troubleshooting

- If the container starts but the host cannot connect, confirm the mounted
  config uses `listen: 0.0.0.0:8080` and Docker maps
  `127.0.0.1:8080:8080`.
- If `/health` succeeds but `/ready` fails, the process is alive but no
  namespace has been loaded yet. Apply runtime config through the Admin API, or
  mount a static config with at least one upstream.
- If provider requests fail with authentication errors, confirm the
  `OPENAI_COMPATIBLE_API_KEY` and `ANTHROPIC_COMPATIBLE_API_KEY` values match
  the edited config you mounted.
- If admin calls fail, use `Authorization: Bearer <admin-token>` with
  `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`; provider/model/resource calls use
  `LLM_UNIVERSAL_PROXY_AUTH_MODE` and, in `proxy_key` mode,
  `LLM_UNIVERSAL_PROXY_KEY`.
- If GHCR returns unauthorized, 403, or 404 for private or not-yet-public
  packages, run `docker login ghcr.io` with a personal access token (classic)
  that has `read:packages`.

## Admin Plane and Dashboard Boundary

Container deployments expose the same split between public dashboard resources
and protected admin API calls:

- `/dashboard` shell and static assets are public UI resources. Loading the
  shell or assets does not grant admin API access.
- Dashboard JavaScript sends `Authorization: Bearer <admin-token>` only when it calls existing `/admin/*` APIs.
- Admin-plane routes use `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`; when the token is
  set to a non-empty value, `/admin/*` requests must provide a matching bearer
  token.
- provider/model/resource routes use `LLM_UNIVERSAL_PROXY_AUTH_MODE` separately
  and do not accept the admin token
- do not introduce a separate service key
- do not add multi-user accounts, readonly roles, or a complex session model in
  this plan

If `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` is unset, admin API access is limited to
loopback clients. Container deployments should normally set a non-empty token
because the process runs behind Docker networking and may be exposed through an
explicit port mapping.

## Provider Route Auth

Container deployments should normally use `LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key`.
In this environment fallback mode, `LLM_UNIVERSAL_PROXY_KEY` is the
client-facing SDK key and each upstream's `provider_key_env` or
`provider_key.env` points at the provider key held by the container environment.
Mounted static config can instead use top-level `data_auth`, including
`proxy_key.env`, for the same process-wide data-plane auth state.
`client_provider_key` mode is available for deployments where clients send
provider keys directly and the proxy does not hold provider keys.

When a controller uses `PUT /admin/data-auth` for key rotation, the new proxy
key applies to new requests immediately. The Admin API does not persist
plaintext keys or runtime config, so the controller should replay both
`/admin/data-auth` and namespace writes after every container restart.

CORS response headers are not emitted by default. Set
`LLM_UNIVERSAL_PROXY_CORS_ALLOWED_ORIGINS` to exact browser origins only when a
browser client needs cross-origin access.

The historical concrete OpenAI + MiniMax example remains available at
[examples/quickstart-openai-minimax.yaml](../examples/quickstart-openai-minimax.yaml)
for users who want a named-provider sample. It is optional example material, not
the container main path or a GA release-gate requirement.

## Local Build and Smoke

The Makefile owns the local container loop:

```bash
make docker-build
make docker-smoke
```

`make docker-smoke` builds the local image, mounts a temporary config at
`/etc/llmup/config.yaml`, sets `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` to a test value,
checks the admin-token boundary, and sends a streaming request through a mock
upstream.

## Release Policy Decisions

- Container releases publish to GHCR at
  `ghcr.io/agentsmith-project/llm-universal-proxy`.
- Pull requests and `main` build an image and run container smoke, but never
  push.
- Release tags matching `v*` build, smoke, and push multi-arch images for
  `linux/amd64` and `linux/arm64`.
- Release images get `vX.Y.Z`, `X.Y.Z`, and `latest` tags. `latest` only moves
  on formal release tags.
- The first container release plan does not publish an `edge` tag.
- The release image contract starts as a non-root user, exposes port
  `8080`, reads `/etc/llmup/config.yaml` by default, ships a secret-free empty
  bootstrap config at that path, and declares a `/ready` Docker `HEALTHCHECK`.
- Secrets must come from runtime environment variables or mounted secret
  managers. The image and examples do not bake provider keys or admin tokens
  into files.

## CI and Release Plan

CI uses the same shape as local smoke:

- `ci.yml`: build a local image, load it into Docker, and run
  `scripts/test_container_smoke.sh`; `push: false` is required.
- `release.yml`: run the same Rust and Python contract test gates as CI, then
  require the mock endpoint matrix, CLI wrapper matrix, perf gate, the protected
  `release-compatible-provider` smoke job, and supply-chain gates before the
  GHCR publishing job can run. The job builds a local `linux/amd64` image for
  smoke first, then pushes the multi-arch GHCR image only when the ref is a
  release tag.
- The mock endpoint matrix runs `scripts/real_endpoint_matrix.py --mock`
  against a local mock upstream and covers unary, stream, tool, and error paths
  before release publication.
- The CLI wrapper matrix gates the wrapper surface in two deterministic parts: a
  structure gate expands the tracked basic matrix. The hermetic scripted interactive Codex wrapper gate runs `scripts/run_codex_proxy.sh` against a
  fake Codex binary and local mock proxy for two stdin turns. It is not a full live multi-client/provider matrix; real live client evidence remains
  GA/operator validation when CLIs and provider credentials are available.
- The perf gate runs `scripts/real_endpoint_matrix.py --mock --perf` against the
  same local mock path and emits threshold-checked JSON.
- The compatible provider smoke gate is separate from container smoke and runs
  only in the protected `release-compatible-provider` environment. It should run
  as provider-neutral compatible live evidence over the OpenAI-compatible chat-completions route `/openai/v1/chat/completions` and the
  Anthropic-compatible messages route `/anthropic/v1/messages`; it does not
  imply legacy `/openai/v1/completions` coverage. Configure either
  `COMPAT_PROVIDER_API_KEY` for one compatible provider that exposes both
  surfaces, or `COMPAT_OPENAI_API_KEY` plus `COMPAT_ANTHROPIC_API_KEY` when the
  surfaces use separate credentials; also set `COMPAT_OPENAI_BASE_URL`,
  `COMPAT_OPENAI_MODEL`, `COMPAT_ANTHROPIC_BASE_URL`, and
  `COMPAT_ANTHROPIC_MODEL`, with optional `COMPAT_PROVIDER_LABEL`. The job
  uploads `artifacts/compatible-provider-smoke.json` as the
  `compatible-provider-smoke` GitHub Actions artifact for external release evidence; it is not a GitHub Release asset unless the workflow is changed to
  attach it to the release.
- Official OpenAI Responses, Gemini, and broader four-provider live smoke can be
  kept as optional extended evidence, but they do not block portable-core GA
  once the provider-neutral compatible smoke and deterministic gates pass.
- The GHCR image tags, including `latest`, are published only after those
  release gates pass.
- Governance runs a local secret scan over tracked fixtures, docs, examples, and
  scripts before CI or release jobs proceed.
- Release metadata is passed through Docker build args for OCI labels: `VERSION`
  and `VCS_REF`.

This keeps the container path production-ready without changing the Rust server
or adding secret-bearing files.
