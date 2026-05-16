use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Response},
};
use tracing::debug;

use crate::formats::UpstreamFormat;
use crate::hooks::{fingerprint_credential, CredentialSource};

use super::data_auth::{DataAccess, RequestAuthContext, RequestAuthorization};
use super::secret_redaction::SecretRedactor;
use super::state::UpstreamState;
use crate::config::DataAuthMode;

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
    auth_context: &RequestAuthContext,
    upstream_state: &UpstreamState,
    upstream_format: UpstreamFormat,
) -> Result<(Vec<(String, String)>, EffectiveCredential), String> {
    let mut headers = extract_forwardable_headers(request_headers);
    strip_auth_headers(&mut headers);
    match (
        auth_context.mode(),
        auth_context.access(),
        auth_context.authorization(),
    ) {
        (
            DataAuthMode::ClientProviderKey,
            DataAccess::ClientProviderKey,
            RequestAuthorization::ClientProviderKey { provider_key },
        ) => {
            if upstream_state.provider_key.is_some() {
                return Err(format!(
                    "request auth snapshot mismatch at generation {}: client_provider_key mode has server provider key",
                    auth_context.generation()
                ));
            }
            headers.push(auth_header_for_format(upstream_format, provider_key));
            Ok((
                headers,
                EffectiveCredential {
                    source: CredentialSource::Client,
                    fingerprint: Some(fingerprint_credential(provider_key)),
                },
            ))
        }
        (DataAuthMode::ProxyKey, DataAccess::ProxyKey { .. }, RequestAuthorization::ProxyKey) => {
            let Some(provider_key) = upstream_state.provider_key.as_ref() else {
                return Err(format!(
                    "request auth snapshot mismatch at generation {}: proxy_key mode missing server provider key",
                    auth_context.generation()
                ));
            };
            headers.push(auth_header_for_format(upstream_format, provider_key));
            Ok((
                headers,
                EffectiveCredential {
                    source: CredentialSource::Server,
                    fingerprint: Some(fingerprint_credential(provider_key)),
                },
            ))
        }
        (_, DataAccess::Unconfigured, _) => Err("data auth is not configured".to_string()),
        (_, DataAccess::Misconfigured(message), _) => {
            Err(format!("data auth misconfigured: {message}"))
        }
        _ => Err(format!(
            "request auth snapshot authorization does not match data auth mode at generation {}",
            auth_context.generation()
        )),
    }
}

pub(super) fn append_upstream_protocol_response_headers(
    response: &mut Response<Body>,
    upstream_headers: &reqwest::header::HeaderMap,
    redactor: &SecretRedactor,
) {
    for (name, value) in upstream_headers.iter() {
        if is_forwardable_upstream_protocol_response_header(name.as_str())
            && !response.headers().contains_key(name)
        {
            let Ok(value) = value.to_str() else {
                continue;
            };
            let redacted_value = redactor.redact_text(value);
            let Ok(redacted_value) = HeaderValue::from_str(&redacted_value) else {
                continue;
            };
            response.headers_mut().append(name, redacted_value);
        }
    }
}

/// Extract only protocol-relevant headers that are safe to forward to upstream.
/// Avoid forwarding generic browser/runtime headers from the client request.
pub(super) fn extract_forwardable_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    const FORWARDABLE: &[&str] = &[
        "authorization",
        "x-api-key",
        "x-goog-api-key",
        "api-key",
        "openai-api-key",
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
                debug!("Forwarding header: {}", name_str);
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
fn auth_header_for_format(format: UpstreamFormat, api_key: &str) -> (String, String) {
    match format {
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses => {
            ("authorization".to_string(), format!("Bearer {api_key}"))
        }
        UpstreamFormat::Anthropic => ("x-api-key".to_string(), api_key.to_string()),
    }
}

fn strip_auth_headers(headers: &mut Vec<(String, String)>) {
    headers.retain(|(k, _)| {
        let k = k.to_lowercase();
        !matches!(
            k.as_str(),
            "authorization"
                | "x-api-key"
                | "x-goog-api-key"
                | "api-key"
                | "openai-api-key"
                | "anthropic-api-key"
        )
    });
}

fn is_forwardable_upstream_protocol_response_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if matches!(name.as_str(), "authorization" | "cookie" | "set-cookie")
        || name.contains("api-key")
        || name.contains("token")
        || name.contains("secret")
        || name.contains("credential")
    {
        return false;
    }

    matches!(
        name.as_str(),
        "request-id" | "x-request-id" | "retry-after" | "rate-limit" | "openai-processing-ms"
    ) || name.starts_with("ratelimit-")
        || name.starts_with("x-ratelimit-")
        || name.starts_with("rate-limit-")
        || name.starts_with("anthropic-ratelimit-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_auth_headers_removes_x_goog_api_key() {
        let mut headers = vec![
            ("x-goog-api-key".to_string(), "google-secret".to_string()),
            ("openai-organization".to_string(), "org_123".to_string()),
        ];

        strip_auth_headers(&mut headers);

        assert!(!headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("x-goog-api-key")));
        assert!(headers
            .iter()
            .any(|(name, value)| name == "openai-organization" && value == "org_123"));
    }
}
