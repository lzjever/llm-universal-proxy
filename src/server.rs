//! HTTP server: single POST endpoint, format detection, proxy to upstream with optional translation.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
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

use crate::config::{AuthPolicy, Config, RuntimeConfigPayload, UpstreamConfig};
use crate::dashboard::run_dashboard;
use crate::discovery::UpstreamCapability;
use crate::hooks::{
    capture_headers, fingerprint_credential, json_response_headers, new_request_id,
    now_timestamp_ms, sse_response_headers, CredentialSource, HookDispatcher, HookRequestContext,
};
use crate::streaming::{needs_stream_translation, TranslateSseStream};
use crate::telemetry::RuntimeMetrics;
use crate::translate::{translate_request, translate_response};
use crate::upstream;
use futures_util::StreamExt;
use std::time::{SystemTime, UNIX_EPOCH};

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
            .map_err(|e| format!("invalid config: {}", e))?;
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
        .map_err(|e| format!("listen addr: {}", e))?;
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
            .map_err(|e| format!("invalid config: {}", e))?;
    }
    let metrics = RuntimeMetrics::new(&config);
    let runtime = build_runtime_state(config, None).await?;
    let state = Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        metrics,
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
            .map_err(|e| format!("invalid config: {}", e))?;
    }
    let metrics = RuntimeMetrics::new(&config);
    let runtime = build_runtime_state(config.clone(), None).await?;
    let state = Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        metrics: metrics.clone(),
    });
    let server_state = state.clone();
    let config = Arc::new(config);
    let dashboard_hooks = HookDispatcher::new(&config.hooks);
    let mut server = tokio::spawn(async move { run_server(server_state, listener).await });
    tokio::select! {
        server_result = &mut server => {
            server_result.map_err(|e| std::io::Error::other(e.to_string()))?
        }
        dashboard_result = run_dashboard(config, metrics, dashboard_hooks) => {
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
            axum::http::Method::OPTIONS,
        ])
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/admin/state", get(handle_admin_state))
        .route(
            "/admin/namespaces/:namespace/config",
            post(handle_admin_namespace_config),
        )
        .route(
            "/admin/namespaces/:namespace/state",
            get(handle_admin_namespace_state),
        )
        .route(
            "/openai/v1/chat/completions",
            post(handle_openai_chat_completions),
        )
        .route("/openai/v1/responses", post(handle_openai_responses))
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
        .layer(cors)
        .with_state(state);

    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    runtime: Arc<RwLock<RuntimeState>>,
    metrics: Arc<RuntimeMetrics>,
}

#[derive(Clone)]
struct UpstreamState {
    config: UpstreamConfig,
    capability: UpstreamCapability,
}

#[derive(Clone)]
struct RuntimeNamespaceState {
    revision: String,
    config: Config,
    upstreams: BTreeMap<String, UpstreamState>,
    client: Client,
    hooks: Option<HookDispatcher>,
}

#[derive(Default)]
struct RuntimeState {
    namespaces: BTreeMap<String, RuntimeNamespaceState>,
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
    config: RuntimeConfigPayload,
    upstreams: Vec<NamespaceUpstreamStateResponse>,
}

#[derive(Debug, Clone, Serialize)]
struct NamespaceUpstreamStateResponse {
    name: String,
    api_root: String,
    fixed_upstream_format: Option<crate::formats::UpstreamFormat>,
    supported_formats: Vec<crate::formats::UpstreamFormat>,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminConfigRequest {
    revision: String,
    config: RuntimeConfigPayload,
}

#[derive(Debug, Clone, Serialize)]
struct AdminConfigResponse {
    namespace: String,
    revision: String,
    status: &'static str,
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
    Ok(RuntimeNamespaceState {
        revision,
        config,
        upstreams,
        client,
        hooks,
    })
}

async fn build_runtime_state(
    config: Config,
    revision: Option<String>,
) -> Result<RuntimeState, String> {
    let mut state = RuntimeState::default();
    if !config.upstreams.is_empty() {
        state.namespaces.insert(
            DEFAULT_NAMESPACE.to_string(),
            build_runtime_namespace_state(
                revision.unwrap_or_else(|| "startup".to_string()),
                config,
            )
            .await?,
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
            api_root: upstream.config.api_root.clone(),
            fixed_upstream_format: upstream.config.fixed_upstream_format,
            supported_formats: upstream.capability.supported.iter().copied().collect(),
        })
        .collect::<Vec<_>>();
    (
        StatusCode::OK,
        Json(NamespaceStateResponse {
            namespace,
            revision: item.revision.clone(),
            config: RuntimeConfigPayload::from(&item.config),
            upstreams,
        }),
    )
        .into_response()
}

async fn handle_admin_namespace_config(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Json(payload): Json<AdminConfigRequest>,
) -> impl IntoResponse {
    let config = match Config::try_from(payload.config) {
        Ok(config) => config,
        Err(error) => {
            return error_response(
                crate::formats::UpstreamFormat::OpenAiCompletion,
                StatusCode::BAD_REQUEST,
                &format!("invalid runtime config: {error}"),
            );
        }
    };
    let namespace_state = match build_runtime_namespace_state(payload.revision.clone(), config).await {
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
    if let Some(current) = runtime.namespaces.get(&namespace) {
        if current.revision >= payload.revision {
            return error_response(
                crate::formats::UpstreamFormat::OpenAiCompletion,
                StatusCode::CONFLICT,
                "stale or duplicate revision",
            );
        }
    }
    runtime.namespaces.insert(namespace.clone(), namespace_state);
    (
        StatusCode::OK,
        Json(AdminConfigResponse {
            namespace,
            revision: payload.revision,
            status: "applied",
        }),
    )
        .into_response()
}

async fn resolve_upstreams(config: &Config) -> BTreeMap<String, UpstreamState> {
    let mut upstreams = BTreeMap::new();
    for upstream in &config.upstreams {
        let capability = if let Some(f) = upstream.fixed_upstream_format {
            UpstreamCapability::fixed(f)
        } else {
            let supported = crate::discovery::discover_supported_formats(
                &upstream.api_root,
                config.upstream_timeout,
                upstream.fallback_api_key.as_deref(),
                &upstream.upstream_headers,
            )
            .await;
            if supported.is_empty() {
                UpstreamCapability::fixed(crate::formats::UpstreamFormat::OpenAiCompletion)
            } else {
                UpstreamCapability::from_supported(supported)
            }
        };
        upstreams.insert(
            upstream.name.clone(),
            UpstreamState {
                config: upstream.clone(),
                capability,
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
    let resolved_model = match namespace_state.config.resolve_model(&requested_model) {
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

    let upstream_format = upstream_state
        .capability
        .upstream_format_for_request(client_format);
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
        request_id,
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
        client_request_body: original_body,
    });

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
    let res = match upstream::call_upstream(&namespace_state.client, &url, &body, stream, &auth_headers).await
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
        let body_stream: Pin<
            Box<dyn futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
        > = if needs_stream_translation(upstream_format, client_format) {
            let translated =
                TranslateSseStream::new(upstream_stream, upstream_format, client_format);
            Box::pin(translated.map(|r| r.map_err(std::io::Error::other)))
        } else {
            Box::pin(upstream_stream.map(|r| r.map_err(std::io::Error::other)))
        };
        let body = if let (Some(dispatcher), Some(ctx)) = (namespace_state.hooks.clone(), hook_ctx.clone()) {
            let captured =
                dispatcher.wrap_stream(body_stream, ctx, status.as_u16(), sse_response_headers());
            Body::from_stream(TrackedBodyStream::new(captured, tracker, status.as_u16()))
        } else {
            Body::from_stream(TrackedBodyStream::new(
                body_stream,
                tracker,
                status.as_u16(),
            ))
        };
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
            &format!("model `{}` not found", id),
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

async fn handle_anthropic_models_inner(
    state: Arc<AppState>,
    namespace: String,
) -> Response<Body> {
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
            &format!("model `{}` not found", id),
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
            &format!("model `{}` not found", id),
        ),
    }
}

async fn namespace_config(state: &Arc<AppState>, namespace: &str) -> Option<Config> {
    let runtime = state.runtime.read().await;
    runtime.namespaces.get(namespace).map(|item| item.config.clone())
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
        crate::formats::UpstreamFormat::OpenAiCompletion => (
            status,
            Json(openai_error_body(&normalized_error)),
        )
            .into_response(),
        crate::formats::UpstreamFormat::OpenAiResponses => (
            status,
            Json(openai_error_body(&normalized_error)),
        )
            .into_response(),
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
    let response_id = format!(
        "resp_error_{}",
        uuid::Uuid::new_v4().simple()
    );
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
    let body = format!(
        "event: response.failed\ndata: {payload}\n\n"
    );

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
        assert!(forwarded.iter().any(|(k, v)| k == "authorization" && v == "Bearer test"));
        assert!(forwarded.iter().any(|(k, v)| k == "anthropic-version" && v == "2023-06-01"));
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
            ("authorization".to_string(), format!("Bearer {}", api_key))
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
            response.headers().get("content-type").and_then(|v| v.to_str().ok()),
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
}
