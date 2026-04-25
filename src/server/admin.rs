use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{connect_info::ConnectInfo, Path, State},
    http::{HeaderMap, Response, StatusCode},
    middleware::Next,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{sanitize_url_for_admin, AdminConfigView, Config, RuntimeConfigPayload};
use crate::discovery::UpstreamAvailability;
use crate::formats::UpstreamFormat;

use super::errors::error_response;
use super::state::{build_runtime_namespace_state, generate_admin_revision, AdminAccess, AppState};

#[derive(Debug, Clone, Serialize)]
struct RuntimeNamespaceSummary {
    namespace: String,
    revision: String,
    upstream_count: usize,
    model_alias_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeStateResponse {
    namespaces: Vec<RuntimeNamespaceSummary>,
}

#[derive(Debug, Clone, Serialize)]
struct NamespaceStateResponse {
    namespace: String,
    revision: String,
    config: AdminConfigView,
    upstreams: Vec<NamespaceUpstreamStateResponse>,
}

#[derive(Debug, Clone, Serialize)]
struct NamespaceUpstreamStateResponse {
    name: String,
    api_root: String,
    fixed_upstream_format: Option<crate::formats::UpstreamFormat>,
    supported_formats: Vec<crate::formats::UpstreamFormat>,
    availability: UpstreamAvailabilityResponse,
    proxy_source: &'static str,
    proxy_mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct UpstreamAvailabilityResponse {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl From<&UpstreamAvailability> for UpstreamAvailabilityResponse {
    fn from(value: &UpstreamAvailability) -> Self {
        Self {
            status: value.status_label(),
            reason: value.reason().map(ToString::to_string),
        }
    }
}

fn proxy_source_label(source: &crate::upstream::ResolvedProxySource) -> &'static str {
    match source {
        crate::upstream::ResolvedProxySource::Upstream => "upstream",
        crate::upstream::ResolvedProxySource::Namespace => "namespace",
        crate::upstream::ResolvedProxySource::Environment => "env",
        crate::upstream::ResolvedProxySource::None => "none",
    }
}

fn proxy_mode_label(target: &crate::upstream::ResolvedProxyTarget) -> &'static str {
    match target {
        crate::upstream::ResolvedProxyTarget::Proxy { .. } => "proxy",
        crate::upstream::ResolvedProxyTarget::Direct => "direct",
        crate::upstream::ResolvedProxyTarget::Inherited => "inherited",
    }
}

fn admin_proxy_url(metadata: &crate::upstream::ResolvedProxyMetadata) -> Option<String> {
    match (&metadata.source, &metadata.target) {
        (
            crate::upstream::ResolvedProxySource::Upstream
            | crate::upstream::ResolvedProxySource::Namespace,
            crate::upstream::ResolvedProxyTarget::Proxy { url },
        ) => Some(sanitize_url_for_admin(url)),
        _ => None,
    }
}

#[derive(Debug, Clone, Deserialize)]
struct AdminConfigCasRequest {
    #[serde(default)]
    if_revision: Option<String>,
    config: RuntimeConfigPayload,
}

#[derive(Debug, Clone)]
struct AdminConfigRequest {
    if_revision: Option<String>,
    config: RuntimeConfigPayload,
}

#[derive(Debug, Clone, Serialize)]
struct AdminConfigResponse {
    namespace: String,
    revision: String,
    status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct AdminConfigPreconditionFailedResponse {
    error: &'static str,
    current_revision: Option<String>,
}

enum AdminWriteError {
    PreconditionFailed { current_revision: Option<String> },
}

pub(super) async fn require_admin_access(
    State(state): State<Arc<AppState>>,
    request: axum::http::Request<Body>,
    next: Next,
) -> Response<Body> {
    let remote_addr = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0)
        .or_else(|| request.extensions().get::<SocketAddr>().copied());

    match authorize_admin_request(&state.admin_access, request.headers(), remote_addr) {
        Ok(()) => next.run(request).await,
        Err((status, message)) => error_response(UpstreamFormat::OpenAiCompletion, status, message),
    }
}

pub(super) fn authorize_admin_request<'a>(
    access: &'a AdminAccess,
    headers: &HeaderMap,
    remote_addr: Option<SocketAddr>,
) -> Result<(), (StatusCode, &'a str)> {
    match access {
        AdminAccess::BearerToken(expected) => {
            let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
                return Err((StatusCode::UNAUTHORIZED, "admin bearer token required"));
            };
            let Ok(value) = value.to_str() else {
                return Err((StatusCode::UNAUTHORIZED, "admin bearer token required"));
            };
            let Some(token) = extract_bearer_token(value) else {
                return Err((StatusCode::UNAUTHORIZED, "admin bearer token required"));
            };
            if token == expected {
                Ok(())
            } else {
                Err((StatusCode::UNAUTHORIZED, "admin bearer token invalid"))
            }
        }
        AdminAccess::Misconfigured => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "admin bearer token misconfigured",
        )),
        AdminAccess::LoopbackOnly => {
            if contains_proxy_forwarding_headers(headers) {
                Err((
                    StatusCode::FORBIDDEN,
                    "admin loopback access rejects proxy forwarding headers",
                ))
            } else if remote_addr.is_some_and(|addr| addr.ip().is_loopback()) {
                Ok(())
            } else {
                Err((
                    StatusCode::FORBIDDEN,
                    "admin access allowed from loopback clients only",
                ))
            }
        }
    }
}

pub(super) fn extract_bearer_token(value: &str) -> Option<&str> {
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

fn parse_admin_config_request(payload: Value) -> Result<AdminConfigRequest, String> {
    let Some(object) = payload.as_object() else {
        return Err("admin config request must be a JSON object".to_string());
    };
    if object.contains_key("revision") {
        return Err(
            "legacy admin config request shape is no longer supported; use `if_revision`"
                .to_string(),
        );
    }

    let request: AdminConfigCasRequest = serde_json::from_value(payload)
        .map_err(|error| format!("invalid admin config request: {error}"))?;
    Ok(AdminConfigRequest {
        if_revision: request.if_revision,
        config: request.config,
    })
}

fn validate_admin_cas_precondition(
    current_revision: Option<&str>,
    if_revision: Option<&str>,
) -> Result<(), AdminWriteError> {
    match (current_revision, if_revision) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(AdminWriteError::PreconditionFailed {
            current_revision: None,
        }),
        (Some(current), Some(expected)) if current == expected => Ok(()),
        (Some(current), _) => Err(AdminWriteError::PreconditionFailed {
            current_revision: Some(current.to_string()),
        }),
    }
}

fn admin_write_error_response(error: AdminWriteError) -> Response<Body> {
    match error {
        AdminWriteError::PreconditionFailed { current_revision } => (
            StatusCode::PRECONDITION_FAILED,
            Json(AdminConfigPreconditionFailedResponse {
                error: "admin config revision precondition failed",
                current_revision,
            }),
        )
            .into_response(),
    }
}

pub(super) async fn handle_admin_state(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let runtime = state.runtime.read().await;
    let namespaces = runtime
        .namespaces
        .iter()
        .map(|(namespace, item)| RuntimeNamespaceSummary {
            namespace: namespace.clone(),
            revision: item.revision.clone(),
            upstream_count: item.config.upstreams.len(),
            model_alias_count: item.config.model_aliases.len(),
        })
        .collect::<Vec<_>>();
    (StatusCode::OK, Json(RuntimeStateResponse { namespaces })).into_response()
}

pub(super) async fn handle_admin_namespace_state(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    let runtime = state.runtime.read().await;
    let Some(item) = runtime.namespaces.get(&namespace) else {
        return error_response(
            UpstreamFormat::OpenAiCompletion,
            StatusCode::NOT_FOUND,
            "namespace not found",
        );
    };
    let upstreams = item
        .upstreams
        .values()
        .map(|upstream| NamespaceUpstreamStateResponse {
            name: upstream.config.name.clone(),
            api_root: sanitize_url_for_admin(&upstream.config.api_root),
            fixed_upstream_format: upstream.config.fixed_upstream_format,
            supported_formats: upstream
                .capability
                .as_ref()
                .map(|capability| capability.supported.iter().copied().collect())
                .unwrap_or_default(),
            availability: UpstreamAvailabilityResponse::from(&upstream.availability),
            proxy_source: proxy_source_label(&upstream.resolved_proxy.source),
            proxy_mode: proxy_mode_label(&upstream.resolved_proxy.target),
            proxy_url: admin_proxy_url(&upstream.resolved_proxy),
        })
        .collect::<Vec<_>>();
    (
        StatusCode::OK,
        Json(NamespaceStateResponse {
            namespace,
            revision: item.revision.clone(),
            config: AdminConfigView::from(&item.config),
            upstreams,
        }),
    )
        .into_response()
}

pub(super) async fn handle_admin_namespace_config(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let request = match parse_admin_config_request(payload) {
        Ok(request) => request,
        Err(error) => {
            return error_response(
                UpstreamFormat::OpenAiCompletion,
                StatusCode::BAD_REQUEST,
                &error,
            );
        }
    };
    let runtime_config = request.config.clone();
    let config = match Config::try_from(runtime_config) {
        Ok(config) => config,
        Err(error) => {
            return error_response(
                UpstreamFormat::OpenAiCompletion,
                StatusCode::BAD_REQUEST,
                &format!("invalid runtime config: {error}"),
            );
        }
    };
    if let Err(error) = state.data_auth_policy.validate(&config) {
        return error_response(
            UpstreamFormat::OpenAiCompletion,
            StatusCode::BAD_REQUEST,
            &format!("invalid runtime config: {error}"),
        );
    }
    let current = {
        let runtime = state.runtime.read().await;
        runtime
            .namespaces
            .get(&namespace)
            .map(|item| item.revision.clone())
    };
    if let Err(response) =
        validate_admin_cas_precondition(current.as_deref(), request.if_revision.as_deref())
    {
        return admin_write_error_response(response);
    }
    let revision = generate_admin_revision();
    let namespace_state = match build_runtime_namespace_state(revision.clone(), config).await {
        Ok(state) => state,
        Err(error) => {
            return error_response(
                UpstreamFormat::OpenAiCompletion,
                StatusCode::BAD_REQUEST,
                &format!("failed to resolve namespace config: {error}"),
            );
        }
    };
    let mut runtime = state.runtime.write().await;
    if let Err(response) = validate_admin_cas_precondition(
        runtime
            .namespaces
            .get(&namespace)
            .map(|item| item.revision.as_str()),
        request.if_revision.as_deref(),
    ) {
        return admin_write_error_response(response);
    }
    runtime
        .namespaces
        .insert(namespace.clone(), namespace_state);
    (
        StatusCode::OK,
        Json(AdminConfigResponse {
            namespace,
            revision,
            status: "applied",
        }),
    )
        .into_response()
}
