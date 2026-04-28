use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode},
    middleware::Next,
};

use crate::config::{Config, UpstreamConfig};
use crate::formats::UpstreamFormat;

use super::errors::error_response;

pub(super) const AUTH_MODE_ENV: &str = "LLM_UNIVERSAL_PROXY_AUTH_MODE";
pub(super) const PROXY_KEY_ENV: &str = "LLM_UNIVERSAL_PROXY_KEY";
pub(super) const LEGACY_DATA_TOKEN_HEADER: &str = "x-llmup-data-token";
pub(super) const CORS_ALLOWED_ORIGINS_ENV: &str = "LLM_UNIVERSAL_PROXY_CORS_ALLOWED_ORIGINS";

#[derive(Debug, Clone)]
pub(super) enum DataAccess {
    ClientProviderKey,
    ProxyKey { key: String },
    Misconfigured(String),
}

#[derive(Debug, Clone)]
pub(super) struct DataAuthState {
    access: DataAccess,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeConfigValidationPolicy {
    pub(super) access: DataAccess,
}

impl DataAuthState {
    pub(super) fn new(access: DataAccess) -> Self {
        Self { access }
    }
}

impl RuntimeConfigValidationPolicy {
    pub(super) fn new(_listener_addr: SocketAddr, access: DataAccess) -> Self {
        Self { access }
    }

    pub(super) fn validate(&self, config: &Config) -> Result<(), String> {
        validate_runtime_config(config, &self.access)
    }
}

impl DataAccess {
    pub(super) fn from_env() -> Self {
        Self::from_env_results(std::env::var(AUTH_MODE_ENV), std::env::var(PROXY_KEY_ENV))
    }

    fn from_env_results(
        mode: Result<String, std::env::VarError>,
        proxy_key: Result<String, std::env::VarError>,
    ) -> Self {
        match mode {
            Ok(mode) => match mode.trim().to_ascii_lowercase().as_str() {
                "client_provider_key" => Self::ClientProviderKey,
                "proxy_key" => Self::proxy_key_from_env_result(proxy_key),
                "" => Self::Misconfigured(format!("{AUTH_MODE_ENV} must not be empty")),
                value => {
                    Self::Misconfigured(format!("{AUTH_MODE_ENV} has unsupported value `{value}`"))
                }
            },
            Err(std::env::VarError::NotPresent) => {
                Self::Misconfigured(format!("{AUTH_MODE_ENV} is required"))
            }
            Err(std::env::VarError::NotUnicode(_)) => {
                Self::Misconfigured(format!("{AUTH_MODE_ENV} must be valid UTF-8"))
            }
        }
    }

    fn proxy_key_from_env_result(proxy_key: Result<String, std::env::VarError>) -> Self {
        match proxy_key {
            Ok(key) if key.trim().is_empty() => {
                Self::Misconfigured(format!("{PROXY_KEY_ENV} must not be empty"))
            }
            Ok(key) => Self::ProxyKey { key },
            Err(std::env::VarError::NotPresent) => Self::Misconfigured(format!(
                "{PROXY_KEY_ENV} is required when {AUTH_MODE_ENV}=proxy_key"
            )),
            Err(std::env::VarError::NotUnicode(_)) => {
                Self::Misconfigured(format!("{PROXY_KEY_ENV} must be valid UTF-8"))
            }
        }
    }

    pub(super) fn provider_key_for_upstream(
        &self,
        upstream: &UpstreamConfig,
    ) -> Result<Option<String>, String> {
        match self {
            Self::ClientProviderKey => Ok(None),
            Self::ProxyKey { .. } => resolve_provider_key(upstream).map(Some),
            Self::Misconfigured(message) => Err(format!("data auth misconfigured: {message}")),
        }
    }
}

pub(super) fn validate_startup(
    config: &Config,
    _listener_addr: SocketAddr,
    access: &DataAccess,
) -> Result<(), String> {
    validate_runtime_config(config, access)
}

fn validate_runtime_config(config: &Config, access: &DataAccess) -> Result<(), String> {
    if let DataAccess::Misconfigured(message) = access {
        return Err(format!("data auth misconfigured: {message}"));
    }

    if matches!(access, DataAccess::ProxyKey { .. }) {
        for upstream in &config.upstreams {
            resolve_provider_key(upstream)?;
        }
    }

    Ok(())
}

fn resolve_provider_key(upstream: &UpstreamConfig) -> Result<String, String> {
    let env_name = upstream
        .provider_key_env
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "upstream `{}` provider_key_env is required when {AUTH_MODE_ENV}=proxy_key",
                upstream.name
            )
        })?;
    match std::env::var(env_name) {
        Ok(value) if value.trim().is_empty() => Err(format!(
            "upstream `{}` provider_key_env `{env_name}` must not be empty",
            upstream.name
        )),
        Ok(value) => Ok(value),
        Err(std::env::VarError::NotPresent) => Err(format!(
            "upstream `{}` provider_key_env `{env_name}` is not set",
            upstream.name
        )),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!(
            "upstream `{}` provider_key_env `{env_name}` must be valid UTF-8",
            upstream.name
        )),
    }
}

pub(super) async fn require_data_access(
    State(state): State<DataAuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Response<Body> {
    match authorize_data_request(&state.access, request.headers()) {
        Ok(DataAuthorization {
            normalized_provider_key,
        }) => {
            strip_client_credential_headers(request.headers_mut());
            if let Some(provider_key) = normalized_provider_key {
                let value = HeaderValue::from_str(&format!("Bearer {provider_key}"))
                    .expect("provider key came from a valid client header");
                request.headers_mut().insert(header::AUTHORIZATION, value);
            }
            next.run(request).await
        }
        Err((status, message)) => error_response(UpstreamFormat::OpenAiCompletion, status, message),
    }
}

struct DataAuthorization {
    normalized_provider_key: Option<String>,
}

fn authorize_data_request<'a>(
    access: &'a DataAccess,
    headers: &HeaderMap,
) -> Result<DataAuthorization, (StatusCode, &'a str)> {
    if headers.contains_key(LEGACY_DATA_TOKEN_HEADER) {
        return Err((
            StatusCode::BAD_REQUEST,
            "x-llmup-data-token is no longer supported",
        ));
    }

    let credential = match extract_standard_credential(headers) {
        Ok(value) => value,
        Err(error) => return Err((StatusCode::BAD_REQUEST, error.message())),
    };

    match access {
        DataAccess::ClientProviderKey => {
            let Some(provider_key) = credential else {
                return Err((StatusCode::UNAUTHORIZED, "provider credential required"));
            };
            Ok(DataAuthorization {
                normalized_provider_key: Some(provider_key),
            })
        }
        DataAccess::ProxyKey { key } => {
            let Some(proxy_key) = credential else {
                return Err((StatusCode::UNAUTHORIZED, "proxy key required"));
            };
            if proxy_key == *key {
                Ok(DataAuthorization {
                    normalized_provider_key: None,
                })
            } else {
                Err((StatusCode::FORBIDDEN, "proxy key invalid"))
            }
        }
        DataAccess::Misconfigured(_) => {
            Err((StatusCode::SERVICE_UNAVAILABLE, "data auth misconfigured"))
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CredentialError {
    Empty,
    Multiple,
    NonBearerAuthorization,
    InvalidHeaderValue,
}

impl CredentialError {
    fn message(self) -> &'static str {
        match self {
            Self::Empty => "credential must not be empty",
            Self::Multiple => "multiple credential headers are not supported",
            Self::NonBearerAuthorization => "authorization credential must use Bearer",
            Self::InvalidHeaderValue => "credential header must be valid UTF-8",
        }
    }
}

fn strip_client_credential_headers(headers: &mut HeaderMap) {
    for name in [
        header::AUTHORIZATION,
        HeaderName::from_static("x-api-key"),
        HeaderName::from_static("api-key"),
        HeaderName::from_static("openai-api-key"),
        HeaderName::from_static("x-goog-api-key"),
        HeaderName::from_static("anthropic-api-key"),
        HeaderName::from_static(LEGACY_DATA_TOKEN_HEADER),
    ] {
        headers.remove(name);
    }
}

fn extract_standard_credential(headers: &HeaderMap) -> Result<Option<String>, CredentialError> {
    let mut credentials = Vec::new();
    collect_authorization_credentials(headers, &mut credentials)?;
    for name in [
        "x-api-key",
        "api-key",
        "openai-api-key",
        "x-goog-api-key",
        "anthropic-api-key",
    ] {
        collect_api_key_credentials(headers, name, &mut credentials)?;
    }
    match credentials.len() {
        0 => Ok(None),
        1 => Ok(credentials.pop()),
        _ => Err(CredentialError::Multiple),
    }
}

fn collect_authorization_credentials(
    headers: &HeaderMap,
    credentials: &mut Vec<String>,
) -> Result<(), CredentialError> {
    for value in headers.get_all(header::AUTHORIZATION).iter() {
        let value = value
            .to_str()
            .map_err(|_| CredentialError::InvalidHeaderValue)?;
        let Some(token) = value
            .get(..7)
            .filter(|prefix| prefix.eq_ignore_ascii_case("Bearer "))
            .map(|_| &value[7..])
        else {
            return Err(CredentialError::NonBearerAuthorization);
        };
        if token.trim().is_empty() {
            return Err(CredentialError::Empty);
        }
        credentials.push(token.to_string());
    }
    Ok(())
}

fn collect_api_key_credentials(
    headers: &HeaderMap,
    name: &'static str,
    credentials: &mut Vec<String>,
) -> Result<(), CredentialError> {
    for value in headers.get_all(HeaderName::from_static(name)).iter() {
        let value = value
            .to_str()
            .map_err(|_| CredentialError::InvalidHeaderValue)?;
        if value.trim().is_empty() {
            return Err(CredentialError::Empty);
        }
        credentials.push(value.to_string());
    }
    Ok(())
}

pub(super) fn cors_allowed_origins_from_env() -> Result<Vec<HeaderValue>, String> {
    let raw = match std::env::var(CORS_ALLOWED_ORIGINS_ENV) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(Vec::new()),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{CORS_ALLOWED_ORIGINS_ENV} must be valid UTF-8"));
        }
    };

    raw.split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(parse_allowed_origin)
        .collect()
}

fn parse_allowed_origin(origin: &str) -> Result<HeaderValue, String> {
    if origin == "*" {
        return Err(format!("{CORS_ALLOWED_ORIGINS_ENV} must not include `*`"));
    }
    let parsed = url::Url::parse(origin).map_err(|error| {
        format!("{CORS_ALLOWED_ORIGINS_ENV} origin `{origin}` is invalid: {error}")
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "{CORS_ALLOWED_ORIGINS_ENV} origin `{origin}` must use http or https"
        ));
    }
    if parsed.host_str().is_none() {
        return Err(format!(
            "{CORS_ALLOWED_ORIGINS_ENV} origin `{origin}` must include a host"
        ));
    }
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(format!(
            "{CORS_ALLOWED_ORIGINS_ENV} origin `{origin}` must be an origin, not a URL with path, userinfo, query, or fragment"
        ));
    }
    HeaderValue::from_str(origin).map_err(|error| {
        format!("{CORS_ALLOWED_ORIGINS_ENV} origin `{origin}` is not a valid header value: {error}")
    })
}
