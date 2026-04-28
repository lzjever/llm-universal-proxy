use axum::{
    body::Body,
    http::{HeaderMap, Response},
};
use tracing::debug;

use crate::formats::UpstreamFormat;
use crate::hooks::{fingerprint_credential, CredentialSource};

use super::state::UpstreamState;

#[derive(Clone)]
pub(super) struct EffectiveCredential {
    pub(super) source: CredentialSource,
    pub(super) fingerprint: Option<String>,
}

pub(super) fn apply_upstream_headers(
    headers: &mut Vec<(String, String)>,
    extra_headers: &[(String, String)],
    target_format: UpstreamFormat,
) {
    for (name, value) in default_protocol_headers(target_format) {
        if !headers
            .iter()
            .any(|(existing_name, _)| existing_name.eq_ignore_ascii_case(name))
        {
            headers.push((name.to_string(), value.to_string()));
        }
    }
    for (name, value) in extra_headers {
        upsert_header(headers, name.to_lowercase(), value.clone());
    }
}

pub(super) fn build_auth_headers(
    request_headers: &HeaderMap,
    upstream_state: &UpstreamState,
    upstream_format: UpstreamFormat,
) -> (Vec<(String, String)>, EffectiveCredential) {
    let mut headers = extract_forwardable_headers(request_headers);
    if let Some(provider_key) = upstream_state.provider_key.as_ref() {
        strip_auth_headers(&mut headers);
        headers.push(auth_header_for_format(upstream_format, provider_key));
        return (
            headers,
            EffectiveCredential {
                source: CredentialSource::Server,
                fingerprint: Some(fingerprint_credential(provider_key)),
            },
        );
    }

    let client_key = extract_api_key_from_headers(&headers);
    if let Some(client_key) = client_key {
        normalize_auth_headers(&mut headers, upstream_format);
        (
            headers,
            EffectiveCredential {
                source: CredentialSource::Client,
                fingerprint: Some(fingerprint_credential(&client_key)),
            },
        )
    } else {
        (
            headers,
            EffectiveCredential {
                source: CredentialSource::Client,
                fingerprint: None,
            },
        )
    }
}

pub(super) fn append_upstream_protocol_response_headers(
    response: &mut Response<Body>,
    upstream_headers: &reqwest::header::HeaderMap,
) {
    for (name, value) in upstream_headers.iter() {
        if is_forwardable_upstream_protocol_response_header(name.as_str())
            && !response.headers().contains_key(name)
        {
            response.headers_mut().append(name, value.clone());
        }
    }
}

/// Extract only protocol-relevant headers that are safe to forward to upstream.
/// Avoid forwarding generic browser/runtime headers from the client request.
pub(super) fn extract_forwardable_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    const FORWARDABLE: &[&str] = &[
        "authorization",
        "x-api-key",
        "api-key",
        "openai-api-key",
        "x-goog-api-key",
        "anthropic-api-key",
        "anthropic-version",
        "anthropic-beta",
        "openai-organization",
        "openai-project",
        "idempotency-key",
        "x-stainless-helper-method",
    ];

    let mut result = Vec::new();
    debug!("Extracting headers from request:");
    for (name, value) in headers.iter() {
        let name_str = name.as_str().to_lowercase();
        if FORWARDABLE.contains(&name_str.as_str()) {
            if let Ok(v) = value.to_str() {
                let display_value = if name_str.contains("key")
                    || name_str.contains("auth")
                    || name_str.contains("token")
                {
                    "***"
                } else {
                    v
                };
                debug!("Forwarding header: {} = {}", name_str, display_value);
                result.push((name_str, v.to_string()));
            }
        } else {
            debug!("Skipping non-forwardable header: {}", name_str);
        }
    }
    debug!("Total headers to forward: {}", result.len());
    result
}

fn default_protocol_headers(target_format: UpstreamFormat) -> Vec<(&'static str, &'static str)> {
    match target_format {
        UpstreamFormat::Anthropic => vec![("anthropic-version", "2023-06-01")],
        _ => Vec::new(),
    }
}

fn upsert_header(headers: &mut Vec<(String, String)>, name: String, value: String) {
    if let Some(existing) = headers
        .iter_mut()
        .find(|(existing_name, _)| existing_name.eq_ignore_ascii_case(&name))
    {
        existing.1 = value;
        return;
    }
    headers.push((name, value));
}

/// Generate auth header for the given upstream format.
/// Different providers use different header names:
/// - OpenAI/Responses: `Authorization: Bearer xxx`
/// - Anthropic: `x-api-key: xxx`
/// - Google: `x-goog-api-key: xxx`
fn auth_header_for_format(format: UpstreamFormat, api_key: &str) -> (String, String) {
    match format {
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses => {
            ("authorization".to_string(), format!("Bearer {api_key}"))
        }
        UpstreamFormat::Anthropic => ("x-api-key".to_string(), api_key.to_string()),
        UpstreamFormat::Google => ("x-goog-api-key".to_string(), api_key.to_string()),
    }
}

/// Normalize auth headers for the target upstream format.
/// Converts client-provided auth to the format expected by upstream.
fn normalize_auth_headers(headers: &mut Vec<(String, String)>, target_format: UpstreamFormat) {
    let extracted_key = extract_api_key_from_headers(headers);

    if let Some(key) = extracted_key {
        strip_auth_headers(headers);

        let auth_header = auth_header_for_format(target_format, &key);
        headers.push(auth_header);
    }
}

fn strip_auth_headers(headers: &mut Vec<(String, String)>) {
    headers.retain(|(k, _)| {
        let k = k.to_lowercase();
        !matches!(
            k.as_str(),
            "authorization"
                | "x-api-key"
                | "api-key"
                | "openai-api-key"
                | "x-goog-api-key"
                | "anthropic-api-key"
        )
    });
}

/// Extract API key from various auth header formats.
fn extract_api_key_from_headers(headers: &[(String, String)]) -> Option<String> {
    for (name, value) in headers {
        let name_lower = name.to_lowercase();
        match name_lower.as_str() {
            "authorization" => {
                if let Some(key) = value
                    .strip_prefix("Bearer ")
                    .or_else(|| value.strip_prefix("bearer "))
                {
                    return Some(key.to_string());
                }
            }
            "x-api-key" | "api-key" | "openai-api-key" | "x-goog-api-key" | "anthropic-api-key" => {
                if !value.is_empty() {
                    return Some(value.clone());
                }
            }
            _ => {}
        }
    }
    None
}

fn is_forwardable_upstream_protocol_response_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("request-id")
        || name.eq_ignore_ascii_case("x-request-id")
        || name.starts_with("anthropic-")
        || name.starts_with("openai-")
        || name.starts_with("ratelimit-")
        || name.starts_with("x-ratelimit-")
}
