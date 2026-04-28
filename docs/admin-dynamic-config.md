# Admin and Dynamic Config

Static YAML is still the recommended default for most users. This page covers the next step: inspecting runtime state and updating namespace config without restarting the proxy.

Use admin-driven dynamic config when you need to:

- inspect the current runtime namespace state
- confirm what upstreams are currently available
- check how upstream proxy routing was resolved
- update a namespace config in place

For the basic YAML shape, start with [Configuration Guide](./configuration.md).
For direct binary startup, use `llm-universal-proxy --config <config.yaml>` when
you already have static config. Use
`LLM_UNIVERSAL_PROXY_ADMIN_TOKEN=<token> llm-universal-proxy --admin-bootstrap`
when a controller will create all namespaces through the Admin API after the
process starts. `--admin-bootstrap` starts with no namespaces; `/health` can
succeed immediately, while `/ready` stays unavailable until a namespace config
is loaded.
For CLI-wrapper entrypoints, the provider-neutral preset names are
`preset-openai-compatible` and `preset-anthropic-compatible`; dynamic admin
writes should send already-hydrated concrete URL/model values. `PRESET_*` URL/model placeholders such as `PRESET_OPENAI_ENDPOINT_BASE_URL` and
`PRESET_ENDPOINT_MODEL` should not be sent as `api_root` or `upstream_model`.
`provider_key_env` remains an environment variable name, so
`PRESET_ENDPOINT_API_KEY` is valid when that variable exists in the proxy
process environment.

## Admin Access Rules

The admin plane is separate from the client-facing data plane.

Current access policy:

- if `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` is set to a non-empty value, admin requests must send `Authorization: Bearer <token>`
- the `Bearer` scheme is case-insensitive, but the token must be non-empty and must match exactly
- if `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` is set to an empty or whitespace-only value, admin auth is misconfigured and admin requests fail closed
- if `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` is not set, admin access is limited to loopback clients such as `127.0.0.1` and `::1`
- in loopback-only mode, admin requests with proxy forwarding headers are rejected

In other words:

- local development can often use loopback admin access directly, without forwarding headers such as `Forwarded`, `X-Forwarded-For`, `X-Forwarded-Host`, `X-Forwarded-Proto`, or `X-Real-IP`
- shared or remote deployments should normally set a non-empty admin bearer token

Provider/model/resource routes use `LLM_UNIVERSAL_PROXY_AUTH_MODE` and do not accept the admin token. That data-plane auth mode is process-wide across namespaces. In `proxy_key` mode, dynamic namespace config can add upstream `provider_key_env` entries, and clients authenticate with `LLM_UNIVERSAL_PROXY_KEY` through their normal SDK API key or bearer token.

## Admin Dashboard Boundary

The dashboard boundary has two separate pieces:

Current product boundary:

- `/dashboard` shell and static assets are public UI resources. Loading the shell or assets does not grant admin API access.
- Dashboard JavaScript sends `Authorization: Bearer <admin-token>` only when it calls existing `/admin/*` APIs.
- Admin-plane routes use `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`; when the token is set to a non-empty value, admin API requests must provide a matching bearer token.
- provider/model/resource routes use `LLM_UNIVERSAL_PROXY_AUTH_MODE` separately and do not accept the admin token
- do not introduce a separate service key
- do not add multi-user accounts, readonly roles, or complex session behavior in this plan

For container-specific runtime notes, see [Container Image and GHCR Release](./container.md).

## Admin Endpoints

The current admin endpoints are:

- `GET /admin/state`
- `GET /admin/namespaces/:namespace/state`
- `POST /admin/namespaces/:namespace/config`

What each one is for:

- `GET /admin/state`: list namespaces currently loaded in the runtime
- `GET /admin/namespaces/:namespace/state`: inspect one namespace, including redacted config and resolved upstream state
- `POST /admin/namespaces/:namespace/config`: replace the namespace config with a new runtime payload

## Inspect Runtime State

### List namespaces

```bash
curl -fsS http://127.0.0.1:8080/admin/state
```

Typical response:

```json
{
  "namespaces": [
    {
      "namespace": "default",
      "revision": "rev-1",
      "upstream_count": 2,
      "model_alias_count": 3
    }
  ]
}
```

### Inspect one namespace

```bash
curl -fsS http://127.0.0.1:8080/admin/namespaces/default/state
```

This response is useful for two related questions:

- what config is currently loaded for this namespace
- what runtime state was actually resolved for each upstream

The response contains:

- `config`: a redacted view of the current namespace config
- `upstreams`: runtime summaries such as supported formats, availability, and resolved proxy behavior

Example runtime fields on each upstream summary:

- `supported_formats`
- `availability`
- `proxy_source`
- `proxy_mode`
- `proxy_url` when the proxy came from an explicit namespace or upstream config

`proxy_source` can be:

- `upstream`
- `namespace`
- `env`
- `none`

`proxy_mode` can be:

- `proxy`
- `direct`
- `inherited`

This makes the admin page useful for answering questions such as:

- did this upstream use its own proxy override
- did it inherit the namespace proxy
- did it fall back to environment proxy settings
- is it explicitly direct

## What the Admin Read View Redacts

Admin read responses are designed for runtime inspection, not for secret export.

The redacted view does not expose sensitive values such as:

- inline upstream credentials
- hook authorization tokens
- raw environment-derived proxy URLs
- userinfo, query strings, or fragments inside URLs

What you get instead is enough operational information to understand the runtime safely, for example:

- whether a provider key is configured through provider_key_env presence
- whether hook authorization is configured
- a sanitized `proxy` or `proxy_url` where that is safe to show

## Update a Namespace Without Restarting

`POST /admin/namespaces/:namespace/config` accepts a runtime config payload and replaces the namespace config.

The write flow supports revision checks so a client does not accidentally overwrite a newer config.

The payload is validated against the current process-wide data-plane auth mode. `LLM_UNIVERSAL_PROXY_AUTH_MODE` applies to all namespaces and is not set through the namespace payload.

- In `proxy_key` mode, every upstream that can receive traffic must include a resolvable `provider_key_env`; the named environment variable must exist in the proxy process environment.
- In `client_provider_key` mode, the payload does not require `provider_key_env`, and it is normally omitted; a non-empty field is not rejected just because this mode is active, but clients still send the real provider key through their normal SDK API key or bearer-token path.

Runtime writes use the same client-visible surface contract as static YAML. Raw HTTP tests can omit `surface_defaults`, but Codex, Claude Code, and Gemini wrapper/live-profile flows should provide at least the conservative text-only surface shown below, or an accurate alias-level `surface`.

Responses reasoning/compaction continuity follows the same compatibility policy
in dynamically written namespaces: default/max_compat may drop an opaque carrier
only when visible summary text or visible transcript history remains;
strict/balanced fail closed; opaque-only reasoning and opaque-only compaction
fail closed; same-provider/native passthrough preserves provider-owned state.

### Recommended write pattern

1. read the current namespace state
2. note the current `revision`
3. send a config update with `if_revision`
4. if the server returns `412 Precondition Failed`, reload and retry with the new revision

### API Configuration Examples

This YAML-shaped view is for readability. Send JSON to the API.

```yaml
# POST /admin/namespaces/default/config body, shown in YAML form.
if_revision: rev-1
config:
  listen: 127.0.0.1:8080
  upstream_timeout_secs: 120

  # Runtime payload uses a list. Each upstream carries its own name.
  upstreams:
    - name: PRESET-OPENAI-COMPATIBLE
      api_root: https://openai-compatible.example/v1
      fixed_upstream_format: openai-completion
      # Env var name read by the proxy in proxy_key mode.
      provider_key_env: PRESET_ENDPOINT_API_KEY
      surface_defaults:
        modalities:
          input: ["text"]
          output: ["text"]
        tools:
          supports_search: false
          supports_view_image: false
          apply_patch_transport: freeform
          supports_parallel_calls: false

  # Runtime aliases are objects, not "UPSTREAM:MODEL" strings.
  model_aliases:
    preset-openai-compatible:
      upstream_name: PRESET-OPENAI-COMPATIBLE
      upstream_model: provider-model-id
```

Actual write request:

```bash
curl -fsS \
  -X POST \
  -H 'Content-Type: application/json' \
  http://127.0.0.1:8080/admin/namespaces/default/config \
  -d @- <<'JSON'
{
  "if_revision": "rev-1",
  "config": {
    "listen": "127.0.0.1:8080",
    "upstream_timeout_secs": 120,
    "proxy": {
      "url": "http://corp-proxy.example:8080"
    },
    "upstreams": [
      {
        "name": "PRESET-OPENAI-COMPATIBLE",
        "api_root": "https://openai-compatible.example/v1",
        "fixed_upstream_format": "openai-completion",
        "provider_key_env": "PRESET_ENDPOINT_API_KEY",
        "surface_defaults": {
          "modalities": {
            "input": ["text"],
            "output": ["text"]
          },
          "tools": {
            "supports_search": false,
            "supports_view_image": false,
            "apply_patch_transport": "freeform",
            "supports_parallel_calls": false
          }
        }
      }
    ],
    "model_aliases": {
      "preset-openai-compatible": {
        "upstream_name": "PRESET-OPENAI-COMPATIBLE",
        "upstream_model": "provider-model-id"
      }
    },
    "hooks": {},
    "debug_trace": {
      "path": null,
      "max_text_chars": 16384
    }
  }
}
JSON
```

Successful writes return the new namespace revision.

## Static YAML vs Runtime Payload

The runtime payload is close to the static YAML structure, but not identical. Dynamic config is best treated as an operational API, not as a direct copy-paste replacement for your YAML file.

| Concern | Static YAML | Runtime Admin API payload |
| --- | --- | --- |
| Upstreams container | `upstreams` named map keyed by upstream name | `upstreams` list of objects; each item has `name` |
| Upstream format | `format` is the normal static field name | `fixed_upstream_format` |
| Alias shorthand | alias string such as `"UPSTREAM:MODEL"` is accepted | alias object with `upstream_name` and `upstream_model` |
| Structured alias metadata | object with `target`, plus optional `limits` and `surface` | object with `upstream_name`, `upstream_model`, plus optional `limits` and `surface` |
| Data-plane auth mode | `LLM_UNIVERSAL_PROXY_AUTH_MODE` in the process environment | same process environment; not in namespace payload |
| Provider key reference | `provider_key_env` names a proxy-side env var in `proxy_key` mode | same mode split; `client_provider_key` mode does not require `provider_key_env` and normally omits it, but non-empty values are not forbidden |

For provider-neutral wrapper sources, hydrate URL and model placeholders before sending an admin write:

- static config source `api_root: PRESET_OPENAI_ENDPOINT_BASE_URL` becomes runtime `api_root: https://openai-compatible.example/v1`
- static alias `"PRESET-OPENAI-COMPATIBLE:PRESET_ENDPOINT_MODEL"` becomes runtime `upstream_name: PRESET-OPENAI-COMPATIBLE` and `upstream_model: provider-model-id`
- static `provider_key_env: PRESET_ENDPOINT_API_KEY` can stay `provider_key_env: PRESET_ENDPOINT_API_KEY` because it is an environment variable name, not the secret value

Good default workflow:

1. keep the source of truth in static YAML
2. use admin reads to inspect the live runtime
3. use admin writes when you need a controlled live update

## When Dynamic Config Is Worth It

Dynamic config is a good fit when:

- you run a long-lived proxy process
- you want to change upstream routing without restarting
- you need a machine-readable runtime view for operations

Static YAML is still the better default when:

- you are running locally
- you are experimenting with aliases and upstream credentials
- you want the easiest path to a reproducible setup

For client attachment and wrappers, see [Client Setup Guide](./clients.md).
