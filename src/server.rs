//! HTTP server: single POST endpoint, format detection, proxy to upstream with optional translation.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::{
    body::Body,
    extract::{connect_info::ConnectInfo, OriginalUri, Path, State},
    http::{HeaderMap, Response, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error, info, warn};

use crate::config::{
    sanitize_url_for_admin, AdminConfigView, AuthPolicy, Config, RuntimeConfigPayload,
    UpstreamConfig,
};
use crate::dashboard::run_dashboard;
use crate::debug_trace::{DebugTraceContext, DebugTraceRecorder};
use crate::discovery::{DiscoveredUpstream, UpstreamAvailability, UpstreamCapability};
use crate::hooks::{
    capture_headers, fingerprint_credential, json_response_headers, new_request_id,
    now_timestamp_ms, sse_response_headers, CredentialSource, HookDispatcher, HookRequestContext,
    HookSnapshot,
};
use crate::streaming::{needs_stream_translation, TranslateSseStream};
use crate::telemetry::RuntimeMetrics;
use crate::translate::{translate_request, translate_response};
use crate::upstream;
use futures_util::StreamExt;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const DEFAULT_NAMESPACE: &str = "default";

pub async fn run_with_config_path(
    path: impl AsRef<std::path::Path>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::from_yaml_path(path).map_err(std::io::Error::other)?;
    run_with_config(config).await
}

pub async fn run_with_config_path_and_dashboard(
    path: impl AsRef<std::path::Path>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::from_yaml_path(path).map_err(std::io::Error::other)?;
    run_with_config_and_dashboard(config).await
}

pub async fn run_with_config(
    config: Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_internal(config, false).await
}

pub async fn run_with_config_and_dashboard(
    config: Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_internal(config, true).await
}

async fn run_internal(
    config: Config,
    dashboard_enabled: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !config.upstreams.is_empty() {
        config
            .validate()
            .map_err(|e| format!("invalid config: {e}"))?;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("llm_universal_proxy=info".parse()?),
        )
        .init();

    let listen = config
        .listen
        .parse::<std::net::SocketAddr>()
        .map_err(|e| format!("listen addr: {e}"))?;
    info!("listening on {}", listen);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    if dashboard_enabled {
        run_with_listener_and_dashboard(config, listener).await
    } else {
        run_with_listener(config, listener).await
    }
}

/// Run the proxy on an already-bound listener. Used by integration tests to bind to port 0 and get the port.
pub async fn run_with_listener(
    config: Config,
    listener: tokio::net::TcpListener,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !config.upstreams.is_empty() {
        config
            .validate()
            .map_err(|e| format!("invalid config: {e}"))?;
    }
    let metrics = RuntimeMetrics::new(&config);
    let runtime = build_runtime_state(config).await?;
    let state = Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        metrics,
        admin_access: AdminAccess::from_env(),
    });
    run_server(state, listener).await
}

pub async fn run_with_listener_and_dashboard(
    config: Config,
    listener: tokio::net::TcpListener,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !config.upstreams.is_empty() {
        config
            .validate()
            .map_err(|e| format!("invalid config: {e}"))?;
    }
    let metrics = RuntimeMetrics::new(&config);
    let runtime = Arc::new(RwLock::new(build_runtime_state(config.clone()).await?));
    let dashboard_runtime = DashboardRuntimeHandle::new(runtime.clone());
    let state = Arc::new(AppState {
        runtime,
        metrics: metrics.clone(),
        admin_access: AdminAccess::from_env(),
    });
    let server_state = state.clone();
    let mut server = tokio::spawn(async move { run_server(server_state, listener).await });
    tokio::select! {
        server_result = &mut server => {
            server_result.map_err(|e| std::io::Error::other(e.to_string()))?
        }
        dashboard_result = run_dashboard(dashboard_runtime, metrics) => {
            server.abort();
            dashboard_result.map_err(std::io::Error::other)?;
            Ok(())
        }
    }
}

async fn run_server(
    state: Arc<AppState>,
    listener: tokio::net::TcpListener,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers(Any);

    let admin_router = Router::new()
        .route("/admin/state", get(handle_admin_state))
        .route(
            "/admin/namespaces/:namespace/config",
            post(handle_admin_namespace_config),
        )
        .route(
            "/admin/namespaces/:namespace/state",
            get(handle_admin_namespace_state),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_access,
        ));

    let data_router = Router::new()
        .route("/health", get(health))
        .route(
            "/openai/v1/chat/completions",
            post(handle_openai_chat_completions),
        )
        .route("/openai/v1/responses", post(handle_openai_responses))
        .route(
            "/openai/v1/responses/compact",
            post(handle_openai_responses_compact),
        )
        .route(
            "/openai/v1/responses/:response_id",
            get(handle_openai_response_get).delete(handle_openai_response_delete),
        )
        .route(
            "/openai/v1/responses/:response_id/cancel",
            post(handle_openai_response_cancel),
        )
        .route("/openai/v1/models", get(handle_openai_models))
        .route("/openai/v1/models/:id", get(handle_openai_model))
        .route("/anthropic/v1/messages", post(handle_anthropic_messages))
        .route("/anthropic/v1/models", get(handle_anthropic_models))
        .route("/anthropic/v1/models/:id", get(handle_anthropic_model))
        .route("/google/v1beta/models", get(handle_google_models))
        .route(
            "/namespaces/:namespace/openai/v1/chat/completions",
            post(handle_openai_chat_completions_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses",
            post(handle_openai_responses_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses/compact",
            post(handle_openai_responses_compact_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses/:response_id",
            get(handle_openai_response_get_namespaced)
                .delete(handle_openai_response_delete_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses/:response_id/cancel",
            post(handle_openai_response_cancel_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/models",
            get(handle_openai_models_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/models/:id",
            get(handle_openai_model_namespaced),
        )
        .route(
            "/namespaces/:namespace/anthropic/v1/messages",
            post(handle_anthropic_messages_namespaced),
        )
        .route(
            "/namespaces/:namespace/anthropic/v1/models",
            get(handle_anthropic_models_namespaced),
        )
        .route(
            "/namespaces/:namespace/anthropic/v1/models/:id",
            get(handle_anthropic_model_namespaced),
        )
        .route(
            "/namespaces/:namespace/google/v1beta/models",
            get(handle_google_models_namespaced),
        )
        .route(
            "/google/v1beta/models/:id",
            get(handle_google_model).post(handle_google_model_action),
        )
        .route(
            "/namespaces/:namespace/google/v1beta/models/:id",
            get(handle_google_model_namespaced).post(handle_google_model_action_namespaced),
        )
        .layer(cors);

    let app = Router::new()
        .merge(admin_router)
        .merge(data_router)
        .with_state(state);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    runtime: Arc<RwLock<RuntimeState>>,
    metrics: Arc<RuntimeMetrics>,
    admin_access: AdminAccess,
}

#[derive(Clone)]
enum AdminAccess {
    BearerToken(String),
    LoopbackOnly,
    Misconfigured,
}

impl AdminAccess {
    fn from_env() -> Self {
        Self::from_env_var_result(std::env::var("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN"))
    }

    fn from_env_var_result(var_result: Result<String, std::env::VarError>) -> Self {
        match var_result {
            Ok(token) if token.trim().is_empty() => Self::Misconfigured,
            Ok(token) => Self::BearerToken(token),
            Err(std::env::VarError::NotPresent) => Self::LoopbackOnly,
            Err(std::env::VarError::NotUnicode(_)) => Self::Misconfigured,
        }
    }
}

#[derive(Clone)]
struct UpstreamState {
    config: UpstreamConfig,
    capability: Option<UpstreamCapability>,
    availability: UpstreamAvailability,
}

#[derive(Clone)]
struct RuntimeNamespaceState {
    revision: String,
    config: Config,
    upstreams: BTreeMap<String, UpstreamState>,
    client: Client,
    hooks: Option<HookDispatcher>,
    debug_trace: Option<DebugTraceRecorder>,
}

#[derive(Default)]
pub(crate) struct RuntimeState {
    namespaces: BTreeMap<String, RuntimeNamespaceState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DashboardUpstreamStatus {
    pub name: String,
    pub availability_status: String,
    pub availability_reason: Option<String>,
}

#[derive(Clone)]
pub(crate) struct DashboardRuntimeHandle {
    runtime: Arc<RwLock<RuntimeState>>,
}

#[derive(Clone)]
pub(crate) struct DashboardNamespaceSnapshot {
    pub config: Config,
    pub upstreams: Vec<DashboardUpstreamStatus>,
    pub hooks: Option<HookSnapshot>,
}

impl DashboardRuntimeHandle {
    fn new(runtime: Arc<RwLock<RuntimeState>>) -> Self {
        Self { runtime }
    }

    pub(crate) fn snapshot(&self) -> DashboardNamespaceSnapshot {
        let Ok(runtime) = self.runtime.try_read() else {
            return DashboardNamespaceSnapshot::empty();
        };
        runtime
            .namespaces
            .get(DEFAULT_NAMESPACE)
            .map(|namespace| DashboardNamespaceSnapshot {
                config: namespace.config.clone(),
                upstreams: namespace
                    .upstreams
                    .values()
                    .map(|upstream| DashboardUpstreamStatus {
                        name: upstream.config.name.clone(),
                        availability_status: upstream.availability.status_label().to_string(),
                        availability_reason: upstream
                            .availability
                            .reason()
                            .map(ToString::to_string),
                    })
                    .collect(),
                hooks: namespace
                    .hooks
                    .as_ref()
                    .map(|dispatcher| dispatcher.snapshot()),
            })
            .unwrap_or_else(DashboardNamespaceSnapshot::empty)
    }
}

impl DashboardNamespaceSnapshot {
    fn empty() -> Self {
        Self {
            config: Config::default(),
            upstreams: Vec::new(),
            hooks: None,
        }
    }
}

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

async fn build_runtime_namespace_state(
    revision: String,
    config: Config,
) -> Result<RuntimeNamespaceState, String> {
    if !config.upstreams.is_empty() {
        config.validate()?;
    }
    let upstreams = resolve_upstreams(&config).await;
    let client = upstream::build_client(&config);
    let hooks = HookDispatcher::new(&config.hooks);
    let debug_trace = DebugTraceRecorder::new(&config.debug_trace);
    Ok(RuntimeNamespaceState {
        revision,
        config,
        upstreams,
        client,
        hooks,
        debug_trace,
    })
}

async fn build_runtime_state(config: Config) -> Result<RuntimeState, String> {
    let mut state = RuntimeState::default();
    if !config.upstreams.is_empty() {
        state.namespaces.insert(
            DEFAULT_NAMESPACE.to_string(),
            build_runtime_namespace_state(generate_admin_revision(), config).await?,
        );
    }
    Ok(state)
}

#[derive(Clone)]
struct EffectiveCredential {
    source: CredentialSource,
    fingerprint: Option<String>,
}

struct TrackedBodyStream<S> {
    inner: S,
    tracker: Option<crate::telemetry::RequestTracker>,
    status: u16,
}

impl<S> TrackedBodyStream<S> {
    fn new(inner: S, tracker: crate::telemetry::RequestTracker, status: u16) -> Self {
        Self {
            inner,
            tracker: Some(tracker),
            status,
        }
    }
}

impl<S> futures_util::Stream for TrackedBodyStream<S>
where
    S: futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => Poll::Ready(Some(Ok(bytes))),
            Poll::Ready(Some(Err(err))) => {
                if let Some(mut tracker) = this.tracker.take() {
                    info!(
                        "stream terminated with upstream error status={}",
                        this.status
                    );
                    tracker.finish_error(502);
                }
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                if let Some(mut tracker) = this.tracker.take() {
                    info!("stream completed status={}", this.status);
                    if (200..400).contains(&this.status) {
                        tracker.finish_success(this.status);
                    } else {
                        tracker.finish_error(this.status);
                    }
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Drop for TrackedBodyStream<S> {
    fn drop(&mut self) {
        if let Some(mut tracker) = self.tracker.take() {
            info!("stream cancelled by downstream client");
            tracker.finish_cancelled();
        }
    }
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

async fn require_admin_access(
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
        Err((status, message)) => error_response(
            crate::formats::UpstreamFormat::OpenAiCompletion,
            status,
            message,
        ),
    }
}

fn authorize_admin_request<'a>(
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

fn generate_admin_revision() -> String {
    Uuid::new_v4().to_string()
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

fn format_upstream_unavailable_message(name: &str, availability: &UpstreamAvailability) -> String {
    match availability {
        UpstreamAvailability::Available => {
            format!("resolved upstream `{name}` is unavailable")
        }
        UpstreamAvailability::Unavailable { reason } => {
            format!("resolved upstream `{name}` is unavailable: {reason}")
        }
    }
}

async fn handle_admin_state(State(state): State<Arc<AppState>>) -> impl IntoResponse {
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

async fn handle_admin_namespace_state(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    let runtime = state.runtime.read().await;
    let Some(item) = runtime.namespaces.get(&namespace) else {
        return error_response(
            crate::formats::UpstreamFormat::OpenAiCompletion,
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

async fn handle_admin_namespace_config(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let request = match parse_admin_config_request(payload) {
        Ok(request) => request,
        Err(error) => {
            return error_response(
                crate::formats::UpstreamFormat::OpenAiCompletion,
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
                crate::formats::UpstreamFormat::OpenAiCompletion,
                StatusCode::BAD_REQUEST,
                &format!("invalid runtime config: {error}"),
            );
        }
    };
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
                crate::formats::UpstreamFormat::OpenAiCompletion,
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

async fn resolve_upstreams(config: &Config) -> BTreeMap<String, UpstreamState> {
    let mut upstreams = BTreeMap::new();
    for upstream in &config.upstreams {
        let discovered = if let Some(f) = upstream.fixed_upstream_format {
            DiscoveredUpstream::fixed(f)
        } else {
            let supported = crate::discovery::discover_supported_formats(
                &upstream.api_root,
                config.upstream_timeout,
                upstream.fallback_api_key.as_deref(),
                &upstream.upstream_headers,
            )
            .await;
            DiscoveredUpstream::from_supported(supported)
        };
        upstreams.insert(
            upstream.name.clone(),
            UpstreamState {
                config: upstream.clone(),
                capability: discovered.capability,
                availability: discovered.availability,
            },
        );
    }
    upstreams
}

async fn handle_openai_chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_openai_chat_completions_inner(state, DEFAULT_NAMESPACE.to_string(), headers, body).await
}

async fn handle_openai_chat_completions_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_openai_chat_completions_inner(state, namespace, headers, body).await
}

async fn handle_openai_chat_completions_inner(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    body: Value,
) -> Response<Body> {
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    handle_request_core(
        state,
        namespace,
        headers,
        "/openai/v1/chat/completions".to_string(),
        body,
        requested_model,
        crate::formats::UpstreamFormat::OpenAiCompletion,
        None,
    )
    .await
}

async fn handle_openai_responses(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_openai_responses_inner(state, DEFAULT_NAMESPACE.to_string(), headers, body).await
}

async fn handle_openai_responses_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_openai_responses_inner(state, namespace, headers, body).await
}

async fn handle_openai_responses_inner(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    body: Value,
) -> Response<Body> {
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    handle_request_core(
        state,
        namespace,
        headers,
        "/openai/v1/responses".to_string(),
        body,
        requested_model,
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await
}

async fn handle_openai_responses_compact(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_openai_responses_compact_inner(state, DEFAULT_NAMESPACE.to_string(), headers, body).await
}

async fn handle_openai_responses_compact_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_openai_responses_compact_inner(state, namespace, headers, body).await
}

async fn handle_openai_responses_compact_inner(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    body: Value,
) -> Response<Body> {
    handle_openai_responses_resource(
        state,
        namespace,
        headers,
        reqwest::Method::POST,
        "responses/compact".to_string(),
        Some(body),
        None,
    )
    .await
}

async fn handle_openai_response_get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(response_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_get_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        response_id,
        headers,
    )
    .await
}

async fn handle_openai_response_get_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, response_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_get_inner(state, namespace, uri, response_id, headers).await
}

async fn handle_openai_response_get_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    response_id: String,
    headers: HeaderMap,
) -> Response<Body> {
    handle_openai_responses_resource(
        state,
        namespace,
        headers,
        reqwest::Method::GET,
        format!("responses/{response_id}"),
        None,
        uri.query().map(ToString::to_string),
    )
    .await
}

async fn handle_openai_response_delete(
    State(state): State<Arc<AppState>>,
    Path(response_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_delete_inner(state, DEFAULT_NAMESPACE.to_string(), response_id, headers)
        .await
}

async fn handle_openai_response_delete_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, response_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_delete_inner(state, namespace, response_id, headers).await
}

async fn handle_openai_response_delete_inner(
    state: Arc<AppState>,
    namespace: String,
    response_id: String,
    headers: HeaderMap,
) -> Response<Body> {
    handle_openai_responses_resource(
        state,
        namespace,
        headers,
        reqwest::Method::DELETE,
        format!("responses/{response_id}"),
        None,
        None,
    )
    .await
}

async fn handle_openai_response_cancel(
    State(state): State<Arc<AppState>>,
    Path(response_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_cancel_inner(state, DEFAULT_NAMESPACE.to_string(), response_id, headers)
        .await
}

async fn handle_openai_response_cancel_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, response_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_cancel_inner(state, namespace, response_id, headers).await
}

async fn handle_openai_response_cancel_inner(
    state: Arc<AppState>,
    namespace: String,
    response_id: String,
    headers: HeaderMap,
) -> Response<Body> {
    handle_openai_responses_resource(
        state,
        namespace,
        headers,
        reqwest::Method::POST,
        format!("responses/{response_id}/cancel"),
        None,
        None,
    )
    .await
}

async fn handle_anthropic_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_anthropic_messages_inner(state, DEFAULT_NAMESPACE.to_string(), headers, body).await
}

async fn handle_anthropic_messages_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_anthropic_messages_inner(state, namespace, headers, body).await
}

async fn handle_anthropic_messages_inner(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    body: Value,
) -> Response<Body> {
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    handle_request_core(
        state,
        namespace,
        headers,
        "/anthropic/v1/messages".to_string(),
        body,
        requested_model,
        crate::formats::UpstreamFormat::Anthropic,
        None,
    )
    .await
}

async fn handle_google_model_action(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_google_model_action_inner(state, DEFAULT_NAMESPACE.to_string(), id, headers, body).await
}

async fn handle_google_model_action_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_google_model_action_inner(state, namespace, id, headers, body).await
}

async fn handle_google_model_action_inner(
    state: Arc<AppState>,
    namespace: String,
    id: String,
    headers: HeaderMap,
    body: Value,
) -> Response<Body> {
    let Some((requested_model, action)) = id.split_once(':') else {
        return error_response(
            crate::formats::UpstreamFormat::Google,
            StatusCode::BAD_REQUEST,
            "google model action path must end with :generateContent or :streamGenerateContent",
        );
    };
    let forced_stream = match action {
        "generateContent" => false,
        "streamGenerateContent" => true,
        _ => {
            return error_response(
                crate::formats::UpstreamFormat::Google,
                StatusCode::BAD_REQUEST,
                "unsupported google model action",
            );
        }
    };
    handle_request_core(
        state,
        namespace,
        headers,
        format!("/google/v1beta/models/{id}"),
        body,
        requested_model.to_string(),
        crate::formats::UpstreamFormat::Google,
        Some(forced_stream),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn handle_request_core(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    path: String,
    mut body: Value,
    requested_model: String,
    client_format: crate::formats::UpstreamFormat,
    forced_stream: Option<bool>,
) -> Response<Body> {
    let request_id = new_request_id();
    let request_timestamp = now_timestamp_ms();
    let original_body = body.clone();
    let original_headers = capture_headers(&headers);
    let stream = forced_stream
        .unwrap_or_else(|| body.get("stream").and_then(Value::as_bool).unwrap_or(false));
    if let Some(obj) = body.as_object_mut() {
        if let Some(forced_stream) = forced_stream {
            obj.insert("stream".to_string(), Value::Bool(forced_stream));
        }
    }

    debug!("Request path: {}", path);
    debug!(
        "Request body: {}",
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
    );

    let namespace_state = {
        let runtime = state.runtime.read().await;
        match runtime.namespaces.get(&namespace) {
            Some(item) => item.clone(),
            None => {
                return error_response(
                    client_format,
                    StatusCode::NOT_FOUND,
                    &format!("namespace `{namespace}` is not configured"),
                );
            }
        }
    };

    let mut tracker = state
        .metrics
        .start_request(path.as_str(), requested_model.clone(), stream);
    let resolved_model = match resolve_requested_model_or_error(
        &namespace_state.config,
        &requested_model,
        client_format,
        &original_body,
    ) {
        Ok(v) => v,
        Err(e) => {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return error_response(client_format, StatusCode::BAD_REQUEST, &e);
        }
    };
    let upstream_state = match namespace_state.upstreams.get(&resolved_model.upstream_name) {
        Some(v) => v,
        None => {
            tracker.finish_error(StatusCode::INTERNAL_SERVER_ERROR.as_u16());
            return error_response(
                client_format,
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!(
                    "resolved upstream `{}` is not configured",
                    resolved_model.upstream_name
                ),
            );
        }
    };
    tracker.set_upstream(
        resolved_model.upstream_name.clone(),
        resolved_model.upstream_model.clone(),
    );
    if !upstream_state.availability.is_available() {
        tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
        return error_response(
            client_format,
            StatusCode::SERVICE_UNAVAILABLE,
            &format_upstream_unavailable_message(
                &resolved_model.upstream_name,
                &upstream_state.availability,
            ),
        );
    }

    let Some(capability) = upstream_state.capability.as_ref() else {
        tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
        return error_response(
            client_format,
            StatusCode::SERVICE_UNAVAILABLE,
            &format_upstream_unavailable_message(
                &resolved_model.upstream_name,
                &upstream_state.availability,
            ),
        );
    };
    let upstream_format = capability.upstream_format_for_request(client_format);
    let compatibility_warnings =
        collect_request_compatibility_warnings(client_format, upstream_format, &original_body);
    for warning in &compatibility_warnings {
        warn!(
            "compatibility downgrade: client_format={} upstream_format={} warning={}",
            client_format, upstream_format, warning
        );
    }

    if client_format != upstream_format {
        if let Err(e) = translate_request(
            client_format,
            upstream_format,
            &resolved_model.upstream_model,
            &mut body,
            stream,
        ) {
            error!("Translation failed: {}", e);
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return error_response(client_format, StatusCode::BAD_REQUEST, &e);
        }
    }

    if let Some(obj) = body.as_object_mut() {
        match upstream_format {
            crate::formats::UpstreamFormat::Google => {
                obj.remove("model");
            }
            _ => {
                obj.insert(
                    "model".to_string(),
                    Value::String(resolved_model.upstream_model.clone()),
                );
            }
        }
    }

    debug!(
        "Translated body for upstream: {}",
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
    );

    let (mut auth_headers, effective_credential) =
        build_auth_headers(&headers, upstream_state, upstream_format);
    apply_upstream_headers(
        &mut auth_headers,
        &upstream_state.config.upstream_headers,
        upstream_format,
    );
    let hook_ctx = namespace_state.hooks.as_ref().map(|_| HookRequestContext {
        request_id: request_id.clone(),
        timestamp_ms: request_timestamp,
        path: path.clone(),
        method: "POST".to_string(),
        stream,
        client_model: requested_model.clone(),
        upstream_name: resolved_model.upstream_name.clone(),
        upstream_model: resolved_model.upstream_model.clone(),
        client_format,
        upstream_format,
        credential_source: effective_credential.source,
        credential_fingerprint: effective_credential.fingerprint.clone(),
        client_request_headers: original_headers,
        client_request_body: original_body.clone(),
    });
    let debug_ctx = namespace_state
        .debug_trace
        .as_ref()
        .map(|_| DebugTraceContext {
            request_id: request_id.clone(),
            timestamp_ms: request_timestamp,
            path: path.clone(),
            stream,
            client_model: requested_model.clone(),
            upstream_name: resolved_model.upstream_name.clone(),
            upstream_model: resolved_model.upstream_model.clone(),
            client_format,
            upstream_format,
        });
    if let (Some(recorder), Some(ctx)) = (namespace_state.debug_trace.as_ref(), debug_ctx.as_ref())
    {
        recorder.record_request_with_upstream(ctx, &original_body, &body);
    }

    let url = upstream::upstream_url(
        &namespace_state.config,
        &upstream_state.config,
        upstream_format,
        if upstream_format == crate::formats::UpstreamFormat::Google {
            Some(resolved_model.upstream_model.as_str())
        } else {
            None
        },
        stream,
    );
    debug!("Calling upstream URL: {}", url);
    let res =
        match upstream::call_upstream(&namespace_state.client, &url, &body, stream, &auth_headers)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
                return streaming_error_response(
                    client_format,
                    StatusCode::BAD_GATEWAY,
                    &e.to_string(),
                );
            }
        };

    if stream {
        let status = res.status();
        debug!("Upstream streaming response status: {}", status);
        if !status.is_success() {
            let error_body = res
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            error!(
                "Upstream returned error for streaming request: {} - {}",
                status, error_body
            );
            tracker.finish_error(status.as_u16());
            return streaming_error_response(
                client_format,
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                &error_body,
            );
        }
        let upstream_stream = res.bytes_stream();
        let mut body_stream: Pin<
            Box<dyn futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
        > = if needs_stream_translation(upstream_format, client_format) {
            let translated =
                TranslateSseStream::new(upstream_stream, upstream_format, client_format);
            Box::pin(translated.map(|r| r.map_err(std::io::Error::other)))
        } else {
            Box::pin(upstream_stream.map(|r| r.map_err(std::io::Error::other)))
        };
        if let (Some(dispatcher), Some(ctx)) = (namespace_state.hooks.clone(), hook_ctx.clone()) {
            body_stream = Box::pin(dispatcher.wrap_stream(
                body_stream,
                ctx,
                status.as_u16(),
                sse_response_headers(),
            ));
        }
        if let (Some(recorder), Some(ctx)) =
            (namespace_state.debug_trace.as_ref(), debug_ctx.clone())
        {
            body_stream = Box::pin(recorder.wrap_stream(body_stream, ctx, status.as_u16()));
        }
        let body = Body::from_stream(TrackedBodyStream::new(
            body_stream,
            tracker,
            status.as_u16(),
        ));
        let mut response = Response::builder()
            .status(status)
            .header("Content-Type", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .header("Connection", "keep-alive")
            .body(body)
            .unwrap();
        append_compatibility_warning_headers(&mut response, &compatibility_warnings);
        return response;
    }

    let status = res.status();
    let bytes = match res.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return error_response(client_format, StatusCode::BAD_GATEWAY, &e.to_string());
        }
    };
    if !status.is_success() {
        error!("Upstream returned non-success status: {}", status);
        error!(
            "Upstream response body: {}",
            String::from_utf8_lossy(&bytes)
        );
        tracker.finish_error(status.as_u16());
        return error_response(
            client_format,
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            &String::from_utf8_lossy(&bytes),
        );
    }
    let upstream_body: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            error!(
                "Upstream returned invalid JSON body: {}",
                String::from_utf8_lossy(&bytes)
            );
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return error_response(
                client_format,
                StatusCode::BAD_GATEWAY,
                "upstream returned invalid JSON",
            );
        }
    };
    if let Some((status, message)) =
        normalized_non_stream_upstream_error(upstream_format, client_format, &upstream_body)
    {
        tracker.finish_error(status.as_u16());
        return error_response(client_format, status, &message);
    }
    let out = match translate_response(upstream_format, client_format, &upstream_body) {
        Ok(v) => v,
        Err(e) => {
            tracker.finish_error(StatusCode::INTERNAL_SERVER_ERROR.as_u16());
            return error_response(client_format, StatusCode::INTERNAL_SERVER_ERROR, &e);
        }
    };
    if let (Some(dispatcher), Some(ctx)) = (namespace_state.hooks.as_ref(), hook_ctx) {
        dispatcher.emit_non_stream(ctx, 200, json_response_headers(), out.clone());
    }
    if let (Some(recorder), Some(ctx)) = (namespace_state.debug_trace.as_ref(), debug_ctx.as_ref())
    {
        recorder.record_non_stream_response(ctx, StatusCode::OK.as_u16(), &out);
    }
    tracker.finish_success(StatusCode::OK.as_u16());
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&out).unwrap_or_else(|_| b"{}".to_vec()),
        ))
        .unwrap();
    append_compatibility_warning_headers(&mut response, &compatibility_warnings);
    response
}

async fn handle_openai_responses_resource(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    method: reqwest::Method,
    resource_path: String,
    body: Option<Value>,
    query: Option<String>,
) -> Response<Body> {
    let request_path = format!("/openai/v1/{resource_path}");
    let mut tracker = state
        .metrics
        .start_request(&request_path, String::new(), false);
    let namespace_state = {
        let runtime = state.runtime.read().await;
        match runtime.namespaces.get(&namespace) {
            Some(item) => item.clone(),
            None => {
                tracker.finish_error(StatusCode::NOT_FOUND.as_u16());
                return error_response(
                    crate::formats::UpstreamFormat::OpenAiResponses,
                    StatusCode::NOT_FOUND,
                    &format!("namespace `{namespace}` is not configured"),
                );
            }
        }
    };

    let matching = namespace_state
        .upstreams
        .values()
        .filter(|upstream| {
            upstream.availability.is_available()
                && upstream.capability.as_ref().is_some_and(|capability| {
                    capability
                        .supported
                        .contains(&crate::formats::UpstreamFormat::OpenAiResponses)
                })
        })
        .collect::<Vec<_>>();

    let upstream_state = match matching.as_slice() {
        [upstream] => *upstream,
        [] => {
            tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
            return error_response(
                crate::formats::UpstreamFormat::OpenAiResponses,
                StatusCode::SERVICE_UNAVAILABLE,
                "Responses lifecycle endpoints require an available upstream that natively supports OpenAI Responses",
            );
        }
        _ => {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return error_response(
                crate::formats::UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_REQUEST,
                "Responses lifecycle endpoint is ambiguous across multiple Responses-capable upstreams in this namespace",
            );
        }
    };

    tracker.set_upstream(upstream_state.config.name.clone(), String::new());
    let (mut auth_headers, _effective_credential) = build_auth_headers(
        &headers,
        upstream_state,
        crate::formats::UpstreamFormat::OpenAiResponses,
    );
    apply_upstream_headers(
        &mut auth_headers,
        &upstream_state.config.upstream_headers,
        crate::formats::UpstreamFormat::OpenAiResponses,
    );

    let mut url =
        crate::config::build_upstream_resource_url(&upstream_state.config.api_root, &resource_path);
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        url.push('?');
        url.push_str(&query);
    }

    let response = match upstream::call_upstream_resource(
        &namespace_state.client,
        method,
        &url,
        body.as_ref(),
        &auth_headers,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return error_response(
                crate::formats::UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
    };

    let status = response.status();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return error_response(
                crate::formats::UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
    };

    if status.is_success() {
        tracker.finish_success(status.as_u16());
    } else {
        tracker.finish_error(status.as_u16());
    }

    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| {
            error_response(
                crate::formats::UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                "failed to build upstream resource response",
            )
        })
}

async fn handle_openai_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    handle_openai_models_inner(state, DEFAULT_NAMESPACE.to_string()).await
}

async fn handle_openai_models_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    handle_openai_models_inner(state, namespace).await
}

async fn handle_openai_models_inner(state: Arc<AppState>, namespace: String) -> Response<Body> {
    match namespace_config(&state, &namespace).await {
        Some(config) => (StatusCode::OK, Json(openai_model_list(&config))).into_response(),
        None => error_response(
            crate::formats::UpstreamFormat::OpenAiCompletion,
            StatusCode::NOT_FOUND,
            "namespace not found",
        ),
    }
}

async fn handle_openai_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    handle_openai_model_inner(state, DEFAULT_NAMESPACE.to_string(), id).await
}

async fn handle_openai_model_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
) -> impl IntoResponse {
    handle_openai_model_inner(state, namespace, id).await
}

async fn handle_openai_model_inner(
    state: Arc<AppState>,
    namespace: String,
    id: String,
) -> Response<Body> {
    let Some(config) = namespace_config(&state, &namespace).await else {
        return error_response(
            crate::formats::UpstreamFormat::OpenAiCompletion,
            StatusCode::NOT_FOUND,
            "namespace not found",
        );
    };
    match openai_model_object(&config, &id) {
        Some(model) => (StatusCode::OK, Json(model)).into_response(),
        None => error_response(
            crate::formats::UpstreamFormat::OpenAiCompletion,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
        ),
    }
}

async fn handle_anthropic_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    handle_anthropic_models_inner(state, DEFAULT_NAMESPACE.to_string()).await
}

async fn handle_anthropic_models_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    handle_anthropic_models_inner(state, namespace).await
}

async fn handle_anthropic_models_inner(state: Arc<AppState>, namespace: String) -> Response<Body> {
    match namespace_config(&state, &namespace).await {
        Some(config) => (StatusCode::OK, Json(anthropic_model_list(&config))).into_response(),
        None => error_response(
            crate::formats::UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            "namespace not found",
        ),
    }
}

async fn handle_anthropic_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    handle_anthropic_model_inner(state, DEFAULT_NAMESPACE.to_string(), id).await
}

async fn handle_anthropic_model_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
) -> impl IntoResponse {
    handle_anthropic_model_inner(state, namespace, id).await
}

async fn handle_anthropic_model_inner(
    state: Arc<AppState>,
    namespace: String,
    id: String,
) -> Response<Body> {
    let Some(config) = namespace_config(&state, &namespace).await else {
        return error_response(
            crate::formats::UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            "namespace not found",
        );
    };
    match anthropic_model_object(&config, &id) {
        Some(model) => (StatusCode::OK, Json(model)).into_response(),
        None => error_response(
            crate::formats::UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
        ),
    }
}

async fn handle_google_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    handle_google_models_inner(state, DEFAULT_NAMESPACE.to_string()).await
}

async fn handle_google_models_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    handle_google_models_inner(state, namespace).await
}

async fn handle_google_models_inner(state: Arc<AppState>, namespace: String) -> Response<Body> {
    match namespace_config(&state, &namespace).await {
        Some(config) => (StatusCode::OK, Json(google_model_list(&config))).into_response(),
        None => error_response(
            crate::formats::UpstreamFormat::Google,
            StatusCode::NOT_FOUND,
            "namespace not found",
        ),
    }
}

async fn handle_google_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    handle_google_model_inner(state, DEFAULT_NAMESPACE.to_string(), id).await
}

async fn handle_google_model_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
) -> impl IntoResponse {
    handle_google_model_inner(state, namespace, id).await
}

async fn handle_google_model_inner(
    state: Arc<AppState>,
    namespace: String,
    id: String,
) -> Response<Body> {
    let Some(config) = namespace_config(&state, &namespace).await else {
        return error_response(
            crate::formats::UpstreamFormat::Google,
            StatusCode::NOT_FOUND,
            "namespace not found",
        );
    };
    match google_model_object(&config, &id) {
        Some(model) => (StatusCode::OK, Json(model)).into_response(),
        None => error_response(
            crate::formats::UpstreamFormat::Google,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
        ),
    }
}

async fn namespace_config(state: &Arc<AppState>, namespace: &str) -> Option<Config> {
    let runtime = state.runtime.read().await;
    runtime
        .namespaces
        .get(namespace)
        .map(|item| item.config.clone())
}

fn configured_aliases(config: &Config) -> Vec<(&String, &crate::config::ModelAlias)> {
    config.model_aliases.iter().collect()
}

fn openai_model_list(config: &Config) -> Value {
    serde_json::json!({
        "object": "list",
        "data": configured_aliases(config)
            .into_iter()
            .map(|(alias, target)| serde_json::json!({
                "id": alias,
                "object": "model",
                "created": 0,
                "owned_by": "proxec",
                "proxec": {
                    "upstream_name": target.upstream_name,
                    "upstream_model": target.upstream_model,
                }
            }))
            .collect::<Vec<_>>()
    })
}

fn openai_model_object(config: &Config, id: &str) -> Option<Value> {
    let target = config.model_aliases.get(id)?;
    Some(serde_json::json!({
        "id": id,
        "object": "model",
        "created": 0,
        "owned_by": "proxec",
        "proxec": {
            "upstream_name": target.upstream_name,
            "upstream_model": target.upstream_model,
        }
    }))
}

fn anthropic_model_list(config: &Config) -> Value {
    let data = configured_aliases(config)
        .into_iter()
        .map(|(alias, target)| anthropic_model_value(alias, target))
        .collect::<Vec<_>>();
    let first_id = data
        .first()
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let last_id = data
        .last()
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    serde_json::json!({
        "data": data,
        "has_more": false,
        "first_id": first_id,
        "last_id": last_id
    })
}

fn anthropic_model_object(config: &Config, id: &str) -> Option<Value> {
    let target = config.model_aliases.get(id)?;
    Some(anthropic_model_value(id, target))
}

fn anthropic_model_value(id: &str, target: &crate::config::ModelAlias) -> Value {
    serde_json::json!({
        "id": id,
        "type": "model",
        "display_name": id,
        "created_at": "1970-01-01T00:00:00Z",
        "proxec": {
            "upstream_name": target.upstream_name,
            "upstream_model": target.upstream_model,
        }
    })
}

fn google_model_list(config: &Config) -> Value {
    serde_json::json!({
        "models": configured_aliases(config)
            .into_iter()
            .map(|(alias, target)| google_model_value(alias, target))
            .collect::<Vec<_>>()
    })
}

fn google_model_object(config: &Config, id: &str) -> Option<Value> {
    let target = config.model_aliases.get(id)?;
    Some(google_model_value(id, target))
}

fn google_model_value(id: &str, target: &crate::config::ModelAlias) -> Value {
    serde_json::json!({
        "name": format!("models/{}", id),
        "baseModelId": id,
        "version": "proxec",
        "displayName": id,
        "description": format!("proxec alias -> {}:{}", target.upstream_name, target.upstream_model),
        "inputTokenLimit": 0,
        "outputTokenLimit": 0,
        "supportedGenerationMethods": ["generateContent"],
        "thinking": false
    })
}

fn error_response(
    format: crate::formats::UpstreamFormat,
    status: StatusCode,
    message: &str,
) -> Response<Body> {
    let normalized_error = normalize_upstream_error(status, message);
    match format {
        crate::formats::UpstreamFormat::OpenAiCompletion => {
            (status, Json(openai_error_body(&normalized_error))).into_response()
        }
        crate::formats::UpstreamFormat::OpenAiResponses => {
            (status, Json(openai_error_body(&normalized_error))).into_response()
        }
        crate::formats::UpstreamFormat::Anthropic => (
            status,
            Json(serde_json::json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": message
                }
            })),
        )
            .into_response(),
        crate::formats::UpstreamFormat::Google => (
            status,
            Json(serde_json::json!({
                "error": {
                    "code": status.as_u16(),
                    "message": message,
                    "status": google_status_text(status),
                }
            })),
        )
            .into_response(),
    }
}

fn streaming_error_response(
    format: crate::formats::UpstreamFormat,
    status: StatusCode,
    message: &str,
) -> Response<Body> {
    if format != crate::formats::UpstreamFormat::OpenAiResponses {
        return error_response(format, status, message);
    }

    let normalized_error = normalize_upstream_error(status, message);
    let response_id = format!("resp_error_{}", uuid::Uuid::new_v4().simple());
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let payload = serde_json::json!({
        "type": "response.failed",
        "sequence_number": 0,
        "response": {
            "id": response_id,
            "object": "response",
            "created_at": created_at,
            "status": "failed",
            "background": false,
            "error": {
                "type": normalized_error.error_type,
                "code": normalized_error.code,
                "message": normalized_error.message,
            },
            "incomplete_details": null,
            "usage": null,
            "metadata": {}
        }
    });
    let body = format!("event: response.failed\ndata: {payload}\n\n");

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(Body::from(body))
        .unwrap()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedUpstreamError {
    message: String,
    error_type: &'static str,
    code: Option<&'static str>,
}

fn normalize_upstream_error(status: StatusCode, raw_message: &str) -> NormalizedUpstreamError {
    let parsed = serde_json::from_str::<Value>(raw_message).ok();
    let extracted_message = parsed
        .as_ref()
        .and_then(extract_error_message)
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| raw_message.to_string());
    let signal = parsed
        .as_ref()
        .map(extract_error_signal)
        .unwrap_or_default();
    let signal = signal.to_ascii_lowercase();
    let message_lc = extracted_message.to_ascii_lowercase();
    let combined = format!("{signal} {message_lc}");

    let (error_type, code) = if status == StatusCode::TOO_MANY_REQUESTS
        || combined.contains("rate limit")
        || combined.contains("rate_limit")
    {
        ("rate_limit_error", Some("rate_limit_exceeded"))
    } else if combined.contains("quota")
        || combined.contains("insufficient_quota")
        || combined.contains("credit balance")
    {
        ("insufficient_quota", Some("insufficient_quota"))
    } else if status.is_server_error()
        || combined.contains("overloaded")
        || combined.contains("slow down")
        || combined.contains("server_is_overloaded")
        || combined.contains("temporarily unavailable")
        || combined.contains("service unavailable")
    {
        ("server_error", Some("server_is_overloaded"))
    } else if combined.contains("context_length_exceeded")
        || combined.contains("context window")
        || combined.contains("maximum context length")
        || combined.contains("prompt is too long")
        || combined.contains("prompt too long")
        || combined.contains("too many tokens")
        || combined.contains("token limit exceeded")
    {
        ("invalid_request_error", Some("context_length_exceeded"))
    } else if combined.contains("invalid_prompt")
        || combined.contains("safety reasons")
        || combined.contains("prompt blocked")
    {
        ("invalid_request_error", Some("invalid_prompt"))
    } else if status.is_client_error() {
        ("invalid_request_error", Some("invalid_request_error"))
    } else {
        ("server_error", None)
    };

    NormalizedUpstreamError {
        message: extracted_message,
        error_type,
        code,
    }
}

fn extract_error_message(body: &Value) -> Option<String> {
    body.get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            body.get("error")
                .and_then(|error| error.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
        })
        .or_else(|| body.get("message").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn extract_error_signal(body: &Value) -> String {
    let candidates = [
        body.get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str),
        body.get("error")
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str),
        body.get("error")
            .and_then(|error| error.get("status"))
            .and_then(Value::as_str),
        body.get("code").and_then(Value::as_str),
        body.get("type").and_then(Value::as_str),
        body.get("status").and_then(Value::as_str),
    ];

    candidates
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
}

fn openai_error_body(error: &NormalizedUpstreamError) -> Value {
    serde_json::json!({
        "error": {
            "message": error.message,
            "type": error.error_type,
            "code": error.code,
        }
    })
}

fn collect_request_compatibility_warnings(
    client_format: crate::formats::UpstreamFormat,
    upstream_format: crate::formats::UpstreamFormat,
    body: &Value,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if client_format == crate::formats::UpstreamFormat::OpenAiResponses
        && upstream_format != crate::formats::UpstreamFormat::OpenAiResponses
    {
        for field in [
            "previous_response_id",
            "truncation",
            "max_tool_calls",
            "include",
            "reasoning",
            "prompt_cache_key",
        ] {
            if body.get(field).is_some() {
                warnings.push(format!(
                    "field `{field}` is not portable from Responses to {upstream_format} and may be dropped or degraded"
                ));
            }
        }
        if upstream_format != crate::formats::UpstreamFormat::OpenAiCompletion
            && body.get("store").is_some()
        {
            warnings.push(format!(
                "field `store` is not portable from Responses to {upstream_format} and will be dropped"
            ));
        }
        if upstream_format != crate::formats::UpstreamFormat::OpenAiResponses {
            if let Some(tools) = body.get("tools").and_then(Value::as_array) {
                if tools
                    .iter()
                    .any(|tool| tool.get("name").is_none() && tool.get("function").is_none())
                {
                    warnings.push(format!(
                        "non-function Responses tools are not portable to {upstream_format} and will be dropped"
                    ));
                }
            }
        }
    }
    if upstream_format == crate::formats::UpstreamFormat::Anthropic
        && body.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false)
    {
        warnings.push(
            "Anthropic does not support `parallel_tool_calls`; proxy approximates this with `disable_parallel_tool_use`".to_string(),
        );
    }
    if client_format == crate::formats::UpstreamFormat::Anthropic
        && matches!(
            upstream_format,
            crate::formats::UpstreamFormat::OpenAiCompletion
                | crate::formats::UpstreamFormat::OpenAiResponses
        )
        && body.get("metadata").is_some()
    {
        warnings.push(
            "Anthropic `metadata` is not universally supported by OpenAI-style upstreams; proxy may drop it for compatibility".to_string(),
        );
    }
    warnings
}

fn resolve_requested_model_or_error(
    namespace_config: &crate::config::Config,
    requested_model: &str,
    client_format: crate::formats::UpstreamFormat,
    body: &Value,
) -> Result<crate::config::ResolvedModel, String> {
    if requested_model.trim().is_empty() && namespace_config.upstreams.len() > 1 {
        if client_format == crate::formats::UpstreamFormat::OpenAiResponses
            && body.get("previous_response_id").is_some()
        {
            return Err(
                "Responses requests with `previous_response_id` must also include a routable `model` when this namespace has multiple upstreams; the proxy does not reconstruct response-to-upstream state"
                    .to_string(),
            );
        }

        return Err(
            "request must include a routable `model` when this namespace has multiple upstreams; use `upstream:model` or configure `model_aliases`"
                .to_string(),
        );
    }

    namespace_config.resolve_model(requested_model)
}

fn append_compatibility_warning_headers(response: &mut Response<Body>, warnings: &[String]) {
    for warning in warnings {
        let sanitized = warning.replace(['\r', '\n'], " ");
        if let Ok(value) = axum::http::HeaderValue::from_str(&sanitized) {
            response
                .headers_mut()
                .append("x-proxy-compat-warning", value);
        }
    }
}

fn normalized_non_stream_upstream_error(
    upstream_format: crate::formats::UpstreamFormat,
    client_format: crate::formats::UpstreamFormat,
    upstream_body: &Value,
) -> Option<(StatusCode, String)> {
    if !matches!(
        client_format,
        crate::formats::UpstreamFormat::OpenAiCompletion
            | crate::formats::UpstreamFormat::OpenAiResponses
    ) {
        return None;
    }

    match upstream_format {
        crate::formats::UpstreamFormat::Anthropic => {
            let stop_reason = upstream_body.get("stop_reason").and_then(Value::as_str)?;
            if stop_reason == "model_context_window_exceeded" {
                return Some((
                    StatusCode::BAD_REQUEST,
                    "Your input exceeds the context window of this model. Please adjust your input and try again.".to_string(),
                ));
            }
            None
        }
        _ => None,
    }
}

fn google_status_text(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "INVALID_ARGUMENT",
        StatusCode::UNAUTHORIZED => "UNAUTHENTICATED",
        StatusCode::FORBIDDEN => "PERMISSION_DENIED",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        StatusCode::TOO_MANY_REQUESTS => "RESOURCE_EXHAUSTED",
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => {
            "UNAVAILABLE"
        }
        _ => "INTERNAL",
    }
}

fn apply_upstream_headers(
    headers: &mut Vec<(String, String)>,
    extra_headers: &[(String, String)],
    target_format: crate::formats::UpstreamFormat,
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

fn build_auth_headers(
    request_headers: &HeaderMap,
    upstream_state: &UpstreamState,
    upstream_format: crate::formats::UpstreamFormat,
) -> (Vec<(String, String)>, EffectiveCredential) {
    let mut headers = extract_forwardable_headers(request_headers);
    let client_key = extract_api_key_from_headers(&headers);
    match upstream_state.config.auth_policy {
        AuthPolicy::ForceServer => {
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
                        | "bearer"
                )
            });
            let server_key = upstream_state
                .config
                .fallback_api_key
                .as_ref()
                .expect("validated force_server requires fallback_api_key");
            headers.push(auth_header_for_format(upstream_format, server_key));
            (
                headers,
                EffectiveCredential {
                    source: CredentialSource::Server,
                    fingerprint: Some(fingerprint_credential(server_key)),
                },
            )
        }
        AuthPolicy::ClientOrFallback => {
            if let Some(client_key) = client_key {
                normalize_auth_headers(&mut headers, upstream_format);
                (
                    headers,
                    EffectiveCredential {
                        source: CredentialSource::Client,
                        fingerprint: Some(fingerprint_credential(&client_key)),
                    },
                )
            } else if let Some(server_key) = upstream_state.config.fallback_api_key.as_ref() {
                headers.push(auth_header_for_format(upstream_format, server_key));
                (
                    headers,
                    EffectiveCredential {
                        source: CredentialSource::Server,
                        fingerprint: Some(fingerprint_credential(server_key)),
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
    }
}

fn default_protocol_headers(
    target_format: crate::formats::UpstreamFormat,
) -> Vec<(&'static str, &'static str)> {
    match target_format {
        crate::formats::UpstreamFormat::Anthropic => vec![("anthropic-version", "2023-06-01")],
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

/// Extract only protocol-relevant headers that are safe to forward to upstream.
/// Avoid forwarding generic browser/runtime headers from the client request.
fn extract_forwardable_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    const FORWARDABLE: &[&str] = &[
        "authorization",
        "x-api-key",
        "api-key",
        "openai-api-key",
        "x-goog-api-key",
        "anthropic-api-key",
        "anthropic-version",
        "anthropic-beta",
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

#[cfg(test)]
mod header_tests {
    use super::extract_forwardable_headers;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn extract_forwardable_headers_keeps_only_protocol_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer test"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("accept-language", HeaderValue::from_static("*"));
        headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));

        let forwarded = extract_forwardable_headers(&headers);
        assert!(forwarded
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer test"));
        assert!(forwarded
            .iter()
            .any(|(k, v)| k == "anthropic-version" && v == "2023-06-01"));
        assert!(!forwarded.iter().any(|(k, _)| k == "content-type"));
        assert!(!forwarded.iter().any(|(k, _)| k == "accept-language"));
        assert!(!forwarded.iter().any(|(k, _)| k == "sec-fetch-mode"));
    }
}

/// Generate auth header for the given upstream format.
/// Different providers use different header names:
/// - OpenAI/Responses: `Authorization: Bearer xxx`
/// - Anthropic: `x-api-key: xxx`
/// - Google: `x-goog-api-key: xxx`
fn auth_header_for_format(
    format: crate::formats::UpstreamFormat,
    api_key: &str,
) -> (String, String) {
    match format {
        crate::formats::UpstreamFormat::OpenAiCompletion
        | crate::formats::UpstreamFormat::OpenAiResponses => {
            ("authorization".to_string(), format!("Bearer {api_key}"))
        }
        crate::formats::UpstreamFormat::Anthropic => ("x-api-key".to_string(), api_key.to_string()),
        crate::formats::UpstreamFormat::Google => {
            ("x-goog-api-key".to_string(), api_key.to_string())
        }
    }
}

/// Normalize auth headers for the target upstream format.
/// Converts client-provided auth to the format expected by upstream.
fn normalize_auth_headers(
    headers: &mut Vec<(String, String)>,
    target_format: crate::formats::UpstreamFormat,
) {
    // Extract the API key from whatever auth header the client provided
    let extracted_key = extract_api_key_from_headers(headers);

    if let Some(key) = extracted_key {
        // Remove all existing auth-related headers
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
                    | "bearer"
            )
        });

        // Add auth header in the correct format for upstream
        let auth_header = auth_header_for_format(target_format, &key);
        headers.push(auth_header);
    }
}

/// Extract API key from various auth header formats.
fn extract_api_key_from_headers(headers: &[(String, String)]) -> Option<String> {
    for (name, value) in headers {
        let name_lower = name.to_lowercase();
        match name_lower.as_str() {
            "authorization" | "bearer" => {
                // Handle "Bearer xxx" format
                if let Some(key) = value
                    .strip_prefix("Bearer ")
                    .or_else(|| value.strip_prefix("bearer "))
                {
                    return Some(key.to_string());
                }
                // Handle raw token
                if !value.is_empty() {
                    return Some(value.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    struct ScopedEnvVar {
        key: &'static str,
        previous: Option<String>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: impl AsRef<str>) -> Self {
            let previous = std::env::var(key).ok();
            std::env::set_var(key, value.as_ref());
            Self { key, previous }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            if let Some(value) = &self.previous {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn normalize_upstream_error_maps_context_window_messages() {
        let error = normalize_upstream_error(
            StatusCode::BAD_REQUEST,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 215000 tokens > 200000 limit"}}"#,
        );

        assert_eq!(
            error,
            NormalizedUpstreamError {
                message: "prompt is too long: 215000 tokens > 200000 limit".to_string(),
                error_type: "invalid_request_error",
                code: Some("context_length_exceeded"),
            }
        );
    }

    #[test]
    fn normalize_upstream_error_preserves_rate_limit_signal() {
        let error = normalize_upstream_error(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"message":"Please slow down.","type":"rate_limit_error"}}"#,
        );

        assert_eq!(
            error,
            NormalizedUpstreamError {
                message: "Please slow down.".to_string(),
                error_type: "rate_limit_error",
                code: Some("rate_limit_exceeded"),
            }
        );
    }

    #[test]
    fn streaming_error_response_returns_responses_failed_event() {
        let response = streaming_error_response(
            crate::formats::UpstreamFormat::OpenAiResponses,
            StatusCode::BAD_REQUEST,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long"}}"#,
        );

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("text/event-stream")
        );

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let body = runtime.block_on(async move {
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body bytes");
            String::from_utf8(bytes.to_vec()).expect("utf8 body")
        });

        assert!(body.contains("event: response.failed"));
        assert!(body.contains("\"code\":\"context_length_exceeded\""));
        assert!(body.contains("\"message\":\"prompt is too long\""));
    }

    #[test]
    fn normalized_non_stream_upstream_error_maps_anthropic_context_window_stop() {
        let upstream_body = serde_json::json!({
            "type": "message",
            "stop_reason": "model_context_window_exceeded"
        });

        let actual = normalized_non_stream_upstream_error(
            crate::formats::UpstreamFormat::Anthropic,
            crate::formats::UpstreamFormat::OpenAiResponses,
            &upstream_body,
        );

        assert_eq!(
            actual,
            Some((
                StatusCode::BAD_REQUEST,
                "Your input exceeds the context window of this model. Please adjust your input and try again.".to_string()
            ))
        );
    }

    #[test]
    fn collect_request_compatibility_warnings_flags_non_portable_responses_fields() {
        let body = serde_json::json!({
            "previous_response_id": "resp_1",
            "truncation": "auto",
            "store": true,
            "tools": [{ "type": "web_search" }]
        });
        let warnings = collect_request_compatibility_warnings(
            crate::formats::UpstreamFormat::OpenAiResponses,
            crate::formats::UpstreamFormat::Anthropic,
            &body,
        );
        assert!(warnings.iter().any(|w| w.contains("previous_response_id")));
        assert!(warnings.iter().any(|w| w.contains("truncation")));
        assert!(warnings.iter().any(|w| w.contains("store")));
        assert!(warnings
            .iter()
            .any(|w| w.contains("non-function Responses tools")));
    }

    #[test]
    fn append_compatibility_warning_headers_exposes_each_warning() {
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .expect("response");
        let warnings = vec![
            "first warning".to_string(),
            "second warning with\nnewline".to_string(),
        ];

        append_compatibility_warning_headers(&mut response, &warnings);

        let values: Vec<_> = response
            .headers()
            .get_all("x-proxy-compat-warning")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        assert_eq!(values, vec!["first warning", "second warning with newline"]);
    }

    #[test]
    fn resolve_requested_model_or_error_requires_model_for_multi_upstream_namespace() {
        let config = crate::config::Config {
            listen: "127.0.0.1:0".to_string(),
            upstream_timeout: std::time::Duration::from_secs(30),
            upstreams: vec![
                crate::config::UpstreamConfig {
                    name: "a".to_string(),
                    api_root: "https://example.com/v1".to_string(),
                    fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                    fallback_credential_env: None,
                    fallback_credential_actual: None,
                    fallback_api_key: None,
                    auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                    upstream_headers: Vec::new(),
                },
                crate::config::UpstreamConfig {
                    name: "b".to_string(),
                    api_root: "https://example.org/v1".to_string(),
                    fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                    fallback_credential_env: None,
                    fallback_credential_actual: None,
                    fallback_api_key: None,
                    auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                    upstream_headers: Vec::new(),
                },
            ],
            model_aliases: Default::default(),
            hooks: Default::default(),
            debug_trace: crate::config::DebugTraceConfig::default(),
        };

        let error = resolve_requested_model_or_error(
            &config,
            "",
            crate::formats::UpstreamFormat::OpenAiResponses,
            &serde_json::json!({}),
        )
        .expect_err("missing model should fail");

        assert!(error.contains("request must include a routable `model`"));
    }

    #[test]
    fn resolve_requested_model_or_error_explains_previous_response_boundary() {
        let config = crate::config::Config {
            listen: "127.0.0.1:0".to_string(),
            upstream_timeout: std::time::Duration::from_secs(30),
            upstreams: vec![
                crate::config::UpstreamConfig {
                    name: "a".to_string(),
                    api_root: "https://example.com/v1".to_string(),
                    fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                    fallback_credential_env: None,
                    fallback_credential_actual: None,
                    fallback_api_key: None,
                    auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                    upstream_headers: Vec::new(),
                },
                crate::config::UpstreamConfig {
                    name: "b".to_string(),
                    api_root: "https://example.org/v1".to_string(),
                    fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                    fallback_credential_env: None,
                    fallback_credential_actual: None,
                    fallback_api_key: None,
                    auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                    upstream_headers: Vec::new(),
                },
            ],
            model_aliases: Default::default(),
            hooks: Default::default(),
            debug_trace: crate::config::DebugTraceConfig::default(),
        };

        let error = resolve_requested_model_or_error(
            &config,
            "",
            crate::formats::UpstreamFormat::OpenAiResponses,
            &serde_json::json!({ "previous_response_id": "resp_1" }),
        )
        .expect_err("missing model should fail");

        assert!(error.contains("previous_response_id"));
        assert!(error.contains("does not reconstruct response-to-upstream state"));
    }

    #[test]
    fn authorize_admin_request_accepts_matching_bearer_token() {
        let access = AdminAccess::BearerToken("secret-token".to_string());
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret-token"),
        );

        assert_eq!(
            authorize_admin_request(
                &access,
                &headers,
                Some("203.0.113.10:8080".parse().unwrap())
            ),
            Ok(())
        );

        let mut lowercase = HeaderMap::new();
        lowercase.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("bearer secret-token"),
        );
        assert_eq!(
            authorize_admin_request(
                &access,
                &lowercase,
                Some("203.0.113.10:8080".parse().unwrap())
            ),
            Ok(())
        );
    }

    #[test]
    fn authorize_admin_request_rejects_missing_or_invalid_bearer_token() {
        let access = AdminAccess::BearerToken("secret-token".to_string());
        let missing = HeaderMap::new();
        assert_eq!(
            authorize_admin_request(&access, &missing, Some("127.0.0.1:8080".parse().unwrap())),
            Err((StatusCode::UNAUTHORIZED, "admin bearer token required"))
        );

        let mut wrong = HeaderMap::new();
        wrong.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer wrong-token"),
        );
        assert_eq!(
            authorize_admin_request(&access, &wrong, Some("127.0.0.1:8080".parse().unwrap())),
            Err((StatusCode::UNAUTHORIZED, "admin bearer token invalid"))
        );
    }

    #[test]
    fn extract_bearer_token_rejects_blank_values() {
        assert_eq!(extract_bearer_token("Bearer "), None);
        assert_eq!(extract_bearer_token("bearer   "), None);
        assert_eq!(extract_bearer_token("Bearer\t"), None);
    }

    #[test]
    fn authorize_admin_request_allows_loopback_only_without_token() {
        let access = AdminAccess::LoopbackOnly;

        assert_eq!(
            authorize_admin_request(
                &access,
                &HeaderMap::new(),
                Some("127.0.0.1:8080".parse().unwrap())
            ),
            Ok(())
        );
        assert_eq!(
            authorize_admin_request(
                &access,
                &HeaderMap::new(),
                Some("[::1]:8080".parse().unwrap())
            ),
            Ok(())
        );
        let mut proxied = HeaderMap::new();
        proxied.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.10"));
        assert_eq!(
            authorize_admin_request(&access, &proxied, Some("127.0.0.1:8080".parse().unwrap())),
            Err((
                StatusCode::FORBIDDEN,
                "admin loopback access rejects proxy forwarding headers"
            ))
        );
        assert_eq!(
            authorize_admin_request(
                &access,
                &HeaderMap::new(),
                Some("203.0.113.10:8080".parse().unwrap())
            ),
            Err((
                StatusCode::FORBIDDEN,
                "admin access allowed from loopback clients only"
            ))
        );
        assert_eq!(
            authorize_admin_request(&access, &HeaderMap::new(), None),
            Err((
                StatusCode::FORBIDDEN,
                "admin access allowed from loopback clients only"
            ))
        );
    }

    #[test]
    fn admin_access_from_env_treats_blank_value_as_misconfigured() {
        let _admin_token = ScopedEnvVar::set("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN", "   ");

        assert!(matches!(
            AdminAccess::from_env(),
            AdminAccess::Misconfigured
        ));
    }

    #[test]
    fn admin_access_from_env_var_result_treats_not_present_as_loopback_only() {
        assert!(matches!(
            AdminAccess::from_env_var_result(Err(std::env::VarError::NotPresent)),
            AdminAccess::LoopbackOnly
        ));
    }

    #[cfg(unix)]
    #[test]
    fn admin_access_from_env_var_result_treats_non_unicode_as_misconfigured() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        assert!(matches!(
            AdminAccess::from_env_var_result(Err(std::env::VarError::NotUnicode(
                OsString::from_vec(vec![0x66, 0x80])
            ))),
            AdminAccess::Misconfigured
        ));
    }

    #[tokio::test]
    async fn admin_namespace_state_sanitizes_urls_and_redacts_sensitive_headers() {
        let config = crate::config::Config {
            listen: "127.0.0.1:0".to_string(),
            upstream_timeout: std::time::Duration::from_secs(30),
            upstreams: vec![crate::config::UpstreamConfig {
                name: "default".to_string(),
                api_root: "https://user:pass@api.openai.com/v1?api_key=inline-secret#frag"
                    .to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                fallback_credential_env: Some("DEMO_KEY".to_string()),
                fallback_credential_actual: Some("inline-secret".to_string()),
                fallback_api_key: Some("inline-secret".to_string()),
                auth_policy: crate::config::AuthPolicy::ForceServer,
                upstream_headers: vec![
                    ("x-tenant".to_string(), "demo".to_string()),
                    (
                        "authorization".to_string(),
                        "Bearer upstream-secret".to_string(),
                    ),
                ],
            }],
            model_aliases: Default::default(),
            hooks: crate::config::HookConfig {
                exchange: Some(crate::config::HookEndpointConfig {
                    url: "https://user:pass@example.com/hooks/exchange?token=exchange-secret#frag"
                        .to_string(),
                    authorization: Some("Bearer exchange-secret".to_string()),
                }),
                ..crate::config::HookConfig::default()
            },
            debug_trace: crate::config::DebugTraceConfig::default(),
        };
        let mut upstreams = BTreeMap::new();
        upstreams.insert(
            "default".to_string(),
            UpstreamState {
                config: config.upstreams[0].clone(),
                capability: Some(UpstreamCapability::fixed(
                    crate::formats::UpstreamFormat::OpenAiResponses,
                )),
                availability: UpstreamAvailability::Available,
            },
        );

        let mut runtime = RuntimeState::default();
        runtime.namespaces.insert(
            "demo".to_string(),
            RuntimeNamespaceState {
                revision: "rev-1".to_string(),
                client: upstream::build_client(&config),
                hooks: None,
                debug_trace: None,
                upstreams,
                config,
            },
        );

        let state = Arc::new(AppState {
            runtime: Arc::new(RwLock::new(runtime)),
            metrics: crate::telemetry::RuntimeMetrics::new(&crate::config::Config::default()),
            admin_access: AdminAccess::LoopbackOnly,
        });

        let response = handle_admin_namespace_state(State(state), Path("demo".to_string()))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(
            body["config"]["upstreams"][0]["api_root"],
            "https://api.openai.com/v1"
        );
        assert_eq!(
            body["upstreams"][0]["api_root"],
            "https://api.openai.com/v1"
        );
        assert_eq!(
            body["config"]["hooks"]["exchange"]["url"],
            "https://example.com/hooks/exchange"
        );
        assert!(body["config"]["upstreams"][0]["upstream_headers"][1]["value"].is_null());
        assert_eq!(
            body["config"]["upstreams"][0]["upstream_headers"][1]["value_redacted"],
            true
        );
        let body_string = serde_json::to_string(&body).expect("body string");
        assert!(!body_string.contains("user:pass@"));
        assert!(!body_string.contains("inline-secret"));
        assert!(!body_string.contains("exchange-secret"));
        assert!(!body_string.contains("upstream-secret"));
        assert!(!body_string.contains("api_key="));
        assert!(!body_string.contains("token="));
        assert!(!body_string.contains("#frag"));
    }

    #[tokio::test]
    async fn dashboard_runtime_snapshot_tracks_live_namespace_state() {
        let mut config = crate::config::Config {
            listen: "127.0.0.1:0".to_string(),
            upstream_timeout: std::time::Duration::from_secs(30),
            upstreams: vec![crate::config::UpstreamConfig {
                name: "auto".to_string(),
                api_root: "https://example.com/v1".to_string(),
                fixed_upstream_format: None,
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            }],
            model_aliases: Default::default(),
            hooks: crate::config::HookConfig {
                exchange: Some(crate::config::HookEndpointConfig {
                    url: "https://example.com/hooks/exchange".to_string(),
                    authorization: Some("Bearer hook-1".to_string()),
                }),
                ..Default::default()
            },
            debug_trace: crate::config::DebugTraceConfig::default(),
        };
        config.model_aliases.insert(
            "alias-1".to_string(),
            crate::config::ModelAlias {
                upstream_name: "auto".to_string(),
                upstream_model: "model-a".to_string(),
            },
        );
        let initial_hooks = HookDispatcher::new(&config.hooks);
        let mut upstreams = BTreeMap::new();
        upstreams.insert(
            "auto".to_string(),
            UpstreamState {
                config: config.upstreams[0].clone(),
                capability: None,
                availability: UpstreamAvailability::Unavailable {
                    reason: "protocol discovery returned no supported formats".to_string(),
                },
            },
        );

        let mut runtime = RuntimeState::default();
        runtime.namespaces.insert(
            DEFAULT_NAMESPACE.to_string(),
            RuntimeNamespaceState {
                revision: "rev-1".to_string(),
                config: config.clone(),
                client: upstream::build_client(&config),
                hooks: initial_hooks,
                debug_trace: None,
                upstreams,
            },
        );

        let handle = DashboardRuntimeHandle::new(Arc::new(RwLock::new(runtime)));
        let snapshot = handle.snapshot();

        assert_eq!(snapshot.config.model_aliases.len(), 1);
        assert_eq!(snapshot.upstreams.len(), 1);
        assert_eq!(snapshot.upstreams[0].name, "auto");
        assert_eq!(snapshot.upstreams[0].availability_status, "unavailable");
        assert_eq!(
            snapshot.upstreams[0].availability_reason.as_deref(),
            Some("protocol discovery returned no supported formats")
        );
        assert!(snapshot.hooks.is_some());

        {
            let mut runtime = handle.runtime.write().await;
            let namespace = runtime
                .namespaces
                .get_mut(DEFAULT_NAMESPACE)
                .expect("default namespace");
            namespace.config.model_aliases.insert(
                "alias-2".to_string(),
                crate::config::ModelAlias {
                    upstream_name: "auto".to_string(),
                    upstream_model: "model-b".to_string(),
                },
            );
            namespace.upstreams.get_mut("auto").unwrap().availability =
                UpstreamAvailability::Available;
            namespace.hooks = HookDispatcher::new(&crate::config::HookConfig::default());
        }

        let updated = handle.snapshot();
        assert_eq!(updated.config.model_aliases.len(), 2);
        assert_eq!(updated.upstreams[0].availability_status, "available");
        assert!(updated.upstreams[0].availability_reason.is_none());
        assert!(updated.hooks.is_none());
    }
}
