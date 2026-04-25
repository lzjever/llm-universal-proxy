# Admin and Dynamic Config

Static YAML is still the recommended default for most users. This page covers the next step: inspecting runtime state and updating namespace config without restarting the proxy.

Use admin-driven dynamic config when you need to:

- inspect the current runtime namespace state
- confirm what upstreams are currently available
- check how upstream proxy routing was resolved
- update a namespace config in place

For the basic YAML shape, start with [Configuration Guide](./configuration.md).

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

The data plane has its own token, `LLM_UNIVERSAL_PROXY_DATA_TOKEN`, and does not accept the admin token for provider/model/resource calls. Dynamic namespace config can add server-held credentials, but those credentials are only usable through the data-plane boundary; non-loopback service mode without a data token fails closed.

## Admin Dashboard Boundary

The Web Admin Dashboard uses the same admin plane and the same `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` boundary as the endpoints below.

Current product boundary:

- keep `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`
- do not introduce a separate service key
- dashboard login is admin-token based
- dashboard shell/admin actions are admin-plane operations, not a separate trust boundary
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

- whether a fallback credential is configured
- whether hook authorization is configured
- a sanitized `proxy` or `proxy_url` where that is safe to show

## Update a Namespace Without Restarting

`POST /admin/namespaces/:namespace/config` accepts a runtime config payload and replaces the namespace config.

The write flow supports revision checks so a client does not accidentally overwrite a newer config.

Runtime writes use the same client-visible surface contract as static YAML. Raw HTTP tests can omit `surface_defaults`, but Codex, Claude Code, and Gemini wrapper/live-profile flows should provide at least the conservative text-only surface shown below, or an accurate alias-level `surface`.

### Recommended write pattern

1. read the current namespace state
2. note the current `revision`
3. send a config update with `if_revision`
4. if the server returns `412 Precondition Failed`, reload and retry with the new revision

### Example write request

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
        "name": "OPENAI",
        "api_root": "https://api.openai.com/v1",
        "fixed_upstream_format": "openai-responses",
        "fallback_credential_env": "OPENAI_API_KEY",
        "auth_policy": "force_server",
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
      "coder-strong": {
        "upstream_name": "OPENAI",
        "upstream_model": "gpt-4o"
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

The runtime payload is close to the YAML structure, but not identical.

The biggest practical difference is:

- static YAML defines `upstreams` as a named map
- admin runtime writes send `upstreams` as a list of named upstream objects

That means dynamic config is best treated as an operational API, not as a direct copy-paste replacement for your YAML file.

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
