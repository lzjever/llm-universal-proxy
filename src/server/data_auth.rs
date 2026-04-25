use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, Request, Response, StatusCode},
    middleware::Next,
};

use crate::config::{is_sensitive_header_name, AuthPolicy, Config};
use crate::formats::UpstreamFormat;

use super::errors::error_response;

pub(super) const DATA_AUTH_MODE_ENV: &str = "LLM_UNIVERSAL_PROXY_DATA_AUTH";
pub(super) const DATA_TOKEN_ENV: &str = "LLM_UNIVERSAL_PROXY_DATA_TOKEN";
pub(super) const DATA_TOKEN_HEADER: &str = "x-llmup-data-token";
pub(super) const CORS_ALLOWED_ORIGINS_ENV: &str = "LLM_UNIVERSAL_PROXY_CORS_ALLOWED_ORIGINS";

#[derive(Debug, Clone)]
pub(super) enum DataAccess {
    BearerToken(String),
    LoopbackOnly,
    Disabled,
    Misconfigured(String),
}

#[derive(Debug, Clone)]
pub(super) struct DataAuthState {
    access: DataAccess,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeConfigValidationPolicy {
    listener_addr: SocketAddr,
    access: DataAccess,
}

impl DataAuthState {
    pub(super) fn new(access: DataAccess) -> Self {
        Self { access }
    }
}

impl RuntimeConfigValidationPolicy {
    pub(super) fn new(listener_addr: SocketAddr, access: DataAccess) -> Self {
        Self {
            listener_addr,
            access,
        }
    }

    pub(super) fn validate(&self, config: &Config) -> Result<(), String> {
        validate_runtime_config(config, self.listener_addr, &self.access)
    }
}

impl DataAccess {
    pub(super) fn from_env() -> Self {
        let mode = std::env::var(DATA_AUTH_MODE_ENV);
        let token = std::env::var(DATA_TOKEN_ENV);
        Self::from_env_results(mode, token)
    }

    fn from_env_results(
        mode: Result<String, std::env::VarError>,
        token: Result<String, std::env::VarError>,
    ) -> Self {
        match mode {
            Ok(mode) => match mode.trim().to_ascii_lowercase().as_str() {
                "token" | "required" | "bearer" => Self::token_from_env_result(token),
                "loopback" | "loopback-only" | "loopback_only" => Self::LoopbackOnly,
                "disabled" | "disable" | "off" | "none" | "false" => Self::Disabled,
                "" => Self::Misconfigured(format!("{DATA_AUTH_MODE_ENV} must not be empty")),
                value => Self::Misconfigured(format!(
                    "{DATA_AUTH_MODE_ENV} has unsupported value `{value}`"
                )),
            },
            Err(std::env::VarError::NotPresent) => match token {
                Ok(token) if token.trim().is_empty() => {
                    Self::Misconfigured(format!("{DATA_TOKEN_ENV} must not be empty"))
                }
                Ok(token) => Self::BearerToken(token),
                Err(std::env::VarError::NotPresent) => Self::LoopbackOnly,
                Err(std::env::VarError::NotUnicode(_)) => {
                    Self::Misconfigured(format!("{DATA_TOKEN_ENV} must be valid UTF-8"))
                }
            },
            Err(std::env::VarError::NotUnicode(_)) => {
                Self::Misconfigured(format!("{DATA_AUTH_MODE_ENV} must be valid UTF-8"))
            }
        }
    }

    fn token_from_env_result(token: Result<String, std::env::VarError>) -> Self {
        match token {
            Ok(token) if token.trim().is_empty() => {
                Self::Misconfigured(format!("{DATA_TOKEN_ENV} must not be empty"))
            }
            Ok(token) => Self::BearerToken(token),
            Err(std::env::VarError::NotPresent) => {
                Self::Misconfigured(format!("{DATA_TOKEN_ENV} is required"))
            }
            Err(std::env::VarError::NotUnicode(_)) => {
                Self::Misconfigured(format!("{DATA_TOKEN_ENV} must be valid UTF-8"))
            }
        }
    }

    fn allows_without_token(&self) -> bool {
        matches!(self, Self::LoopbackOnly | Self::Disabled)
    }
}

pub(super) fn validate_startup(
    config: &Config,
    listener_addr: SocketAddr,
    access: &DataAccess,
) -> Result<(), String> {
    validate_runtime_config(config, listener_addr, access)
}

fn validate_runtime_config(
    config: &Config,
    listener_addr: SocketAddr,
    access: &DataAccess,
) -> Result<(), String> {
    if let DataAccess::Misconfigured(message) = access {
        return Err(format!("data auth misconfigured: {message}"));
    }

    if !listener_addr.ip().is_loopback()
        && access.allows_without_token()
        && config_uses_server_credentials(config)
    {
        return Err(
            "data auth token required when listening on non-loopback with server credentials, sensitive upstream headers, or force_server"
                .to_string(),
        );
    }

    Ok(())
}

fn config_uses_server_credentials(config: &Config) -> bool {
    config.upstreams.iter().any(|upstream| {
        upstream.auth_policy == AuthPolicy::ForceServer
            || upstream.fallback_api_key.is_some()
            || upstream.fallback_credential_actual.is_some()
            || upstream.fallback_credential_env.is_some()
            || upstream
                .upstream_headers
                .iter()
                .any(|(name, _)| is_sensitive_header_name(name))
    })
}

pub(super) async fn require_data_access(
    State(state): State<DataAuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Response<Body> {
    match authorize_data_request(&state.access, request.headers(), remote_addr(&request)) {
        Ok(DataAuthorization {
            strip_authorization,
        }) => {
            request.headers_mut().remove(DATA_TOKEN_HEADER);
            if strip_authorization {
                request.headers_mut().remove(header::AUTHORIZATION);
            }
            next.run(request).await
        }
        Err((status, message)) => error_response(UpstreamFormat::OpenAiCompletion, status, message),
    }
}

struct DataAuthorization {
    strip_authorization: bool,
}

fn authorize_data_request<'a>(
    access: &'a DataAccess,
    headers: &HeaderMap,
    remote_addr: Option<SocketAddr>,
) -> Result<DataAuthorization, (StatusCode, &'a str)> {
    match access {
        DataAccess::BearerToken(expected) => authorize_bearer_data_token(expected, headers),
        DataAccess::LoopbackOnly => {
            if contains_proxy_forwarding_headers(headers) {
                Err((
                    StatusCode::FORBIDDEN,
                    "data loopback access rejects proxy forwarding headers",
                ))
            } else if remote_addr.is_some_and(|addr| addr.ip().is_loopback()) {
                Ok(DataAuthorization {
                    strip_authorization: false,
                })
            } else {
                Err((
                    StatusCode::FORBIDDEN,
                    "data access allowed from loopback clients only",
                ))
            }
        }
        DataAccess::Disabled => Ok(DataAuthorization {
            strip_authorization: false,
        }),
        DataAccess::Misconfigured(_) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "data bearer token misconfigured",
        )),
    }
}

fn authorize_bearer_data_token<'a>(
    expected: &str,
    headers: &HeaderMap,
) -> Result<DataAuthorization, (StatusCode, &'a str)> {
    let explicit_token = headers
        .get(DATA_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty());
    let bearer_token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(extract_bearer_token);

    if explicit_token.is_none() && bearer_token.is_none() {
        return Err((StatusCode::UNAUTHORIZED, "data bearer token required"));
    }

    let explicit_matches = explicit_token.is_some_and(|token| token == expected);
    let bearer_matches = bearer_token.is_some_and(|token| token == expected);
    if explicit_matches || bearer_matches {
        Ok(DataAuthorization {
            strip_authorization: bearer_matches,
        })
    } else {
        Err((StatusCode::FORBIDDEN, "data bearer token invalid"))
    }
}

fn extract_bearer_token(value: &str) -> Option<&str> {
    let token = value
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("Bearer "))
        .map(|_| &value[7..])?;
    if token.trim().is_empty() {
        None
    } else {
        Some(token)
    }
}

fn remote_addr(request: &Request<Body>) -> Option<SocketAddr> {
    request
        .extensions()
        .get::<axum::extract::connect_info::ConnectInfo<SocketAddr>>()
        .map(|info| info.0)
        .or_else(|| request.extensions().get::<SocketAddr>().copied())
}

fn contains_proxy_forwarding_headers(headers: &HeaderMap) -> bool {
    const PROXY_HEADERS: &[&str] = &[
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
    ];

    headers.keys().any(|name| {
        PROXY_HEADERS
            .iter()
            .any(|forbidden| name.as_str().eq_ignore_ascii_case(forbidden))
    })
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
