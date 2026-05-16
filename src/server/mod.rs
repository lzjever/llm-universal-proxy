//! HTTP server: single POST endpoint, format detection, proxy to upstream with optional translation.

mod admin;
mod body_limits;
mod data_auth;
mod errors;
mod headers;
mod models;
mod proxy;
mod public_boundary;
mod responses_resources;
mod secret_redaction;
mod state;
#[cfg(test)]
mod tests;
mod tracked_body;
mod web_dashboard;

use std::sync::Arc;
use std::{convert::Infallible, io, time::Duration};

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderName},
    middleware,
    response::Response,
    routing::{get, post},
    Extension, Router,
};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};
use tracing::info;

use crate::config::Config;
use crate::dashboard::run_dashboard;
use crate::dashboard_logs;
use crate::downstream::{
    cancellation_channel, wrap_body_with_cancellation, DownstreamCancellation,
    DownstreamCancellationHandle,
};
use crate::telemetry::RuntimeMetrics;

use state::{build_runtime_state, AdminAccess, AppState};
pub(crate) use state::{
    DashboardNamespaceSnapshot, DashboardRuntimeHandle, DashboardUpstreamStatus,
};

#[derive(Debug, Clone)]
#[doc(hidden)]
pub struct DataAuthConfig {
    access: data_auth::DataAccess,
}

impl DataAuthConfig {
    pub fn client_provider_key() -> Self {
        Self {
            access: data_auth::DataAccess::ClientProviderKey,
        }
    }

    pub fn proxy_key(key: impl Into<String>) -> Self {
        let key = key.into();
        let access = if key.trim().is_empty() {
            data_auth::DataAccess::Misconfigured(format!(
                "{} must not be empty",
                data_auth::PROXY_KEY_ENV
            ))
        } else {
            data_auth::DataAccess::ProxyKey { key }
        };
        Self { access }
    }
}

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
    init_tracing(dashboard_enabled)?;

    let listen = config
        .listen
        .parse::<std::net::SocketAddr>()
        .map_err(|e| format!("listen addr: {e}"))?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    info!("listening on {}", listen);
    if dashboard_enabled {
        run_with_listener_and_dashboard(config, listener).await
    } else {
        run_with_listener(config, listener).await
    }
}

fn init_tracing(dashboard_enabled: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let env_filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("llm_universal_proxy=info".parse()?);

    if dashboard_enabled {
        tracing_subscriber::fmt()
            .compact()
            .without_time()
            .with_target(false)
            .with_env_filter(env_filter)
            .with_ansi(false)
            .with_writer(dashboard_logs::make_writer())
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }

    Ok(())
}

/// Run the proxy on an already-bound listener. Used by integration tests to bind to port 0 and get the port.
pub async fn run_with_listener(
    config: Config,
    listener: tokio::net::TcpListener,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let data_auth = data_auth::RuntimeDataAuthState::from_static_config(config.data_auth.as_ref());
    run_with_listener_internal(config, listener, data_auth).await
}

#[doc(hidden)]
pub async fn run_with_listener_with_data_auth(
    config: Config,
    listener: tokio::net::TcpListener,
    data_auth: DataAuthConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_with_listener_internal(
        config,
        listener,
        data_auth::RuntimeDataAuthState::from_access(data_auth.access),
    )
    .await
}

async fn run_with_listener_internal(
    mut config: Config,
    listener: tokio::net::TcpListener,
    data_auth: data_auth::RuntimeDataAuthState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(data_auth) = &config.data_auth {
        data_auth
            .validate_static()
            .map_err(|e| format!("invalid config: {e}"))?;
    }
    if !config.upstreams.is_empty() {
        config
            .validate()
            .map_err(|e| format!("invalid config: {e}"))?;
    }
    let listener_addr = listener.local_addr()?;
    let data_access = data_auth.access().clone();
    data_auth::validate_startup(&config, listener_addr, &data_access).map_err(io::Error::other)?;
    config.data_auth = None;
    let data_auth_manager = data_auth::DataAuthManager::new(data_auth);
    let data_auth_policy =
        data_auth::RuntimeConfigValidationPolicy::from_manager(listener_addr, data_auth_manager);
    let metrics = RuntimeMetrics::new(&config);
    let runtime = build_runtime_state(config, &data_access).await?;
    let state = Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        admin_update_lock: Arc::new(Mutex::new(())),
        metrics,
        admin_access: AdminAccess::from_env(),
        data_auth_policy,
    });
    run_server(state, listener).await
}

pub async fn run_with_listener_and_dashboard(
    mut config: Config,
    listener: tokio::net::TcpListener,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(data_auth) = &config.data_auth {
        data_auth
            .validate_static()
            .map_err(|e| format!("invalid config: {e}"))?;
    }
    if !config.upstreams.is_empty() {
        config
            .validate()
            .map_err(|e| format!("invalid config: {e}"))?;
    }
    let data_auth = data_auth::RuntimeDataAuthState::from_static_config(config.data_auth.as_ref());
    let listener_addr = listener.local_addr()?;
    let data_access = data_auth.access().clone();
    data_auth::validate_startup(&config, listener_addr, &data_access).map_err(io::Error::other)?;
    config.data_auth = None;
    let data_auth_manager = data_auth::DataAuthManager::new(data_auth);
    let data_auth_policy =
        data_auth::RuntimeConfigValidationPolicy::from_manager(listener_addr, data_auth_manager);
    let metrics = RuntimeMetrics::new(&config);
    let runtime = Arc::new(RwLock::new(
        build_runtime_state(config.clone(), &data_access).await?,
    ));
    let dashboard_runtime = DashboardRuntimeHandle::new(runtime.clone());
    let state = Arc::new(AppState {
        runtime,
        admin_update_lock: Arc::new(Mutex::new(())),
        metrics: metrics.clone(),
        admin_access: AdminAccess::from_env(),
        data_auth_policy,
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
    let cors = data_cors_layer_from_env().map_err(io::Error::other)?;

    let admin_router = Router::new()
        .route("/admin/state", get(admin::handle_admin_state))
        .route(
            "/admin/data-auth",
            get(admin::handle_admin_data_auth_state).put(admin::handle_admin_data_auth_config),
        )
        .route(
            "/admin/namespaces/:namespace/config",
            post(admin::handle_admin_namespace_config),
        )
        .route(
            "/admin/namespaces/:namespace/state",
            get(admin::handle_admin_namespace_state),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            admin::require_admin_access,
        ));

    let dashboard_router = Router::new()
        .route("/dashboard", get(web_dashboard::handle_dashboard_index))
        .route("/dashboard/", get(web_dashboard::handle_dashboard_index))
        .route(
            "/dashboard/assets/app.css",
            get(web_dashboard::handle_dashboard_css),
        )
        .route(
            "/dashboard/assets/app.js",
            get(web_dashboard::handle_dashboard_js),
        );

    let protected_data_router = Router::new()
        .route(
            "/openai/v1/chat/completions",
            post(proxy::handle_openai_chat_completions),
        )
        .route("/openai/v1/responses", post(proxy::handle_openai_responses))
        .route(
            "/openai/v1/responses/compact",
            post(responses_resources::handle_openai_responses_compact),
        )
        .route(
            "/openai/v1/responses/input_tokens",
            post(responses_resources::handle_openai_responses_input_tokens),
        )
        .route(
            "/openai/v1/responses/:response_id/input_items",
            get(responses_resources::handle_openai_response_input_items),
        )
        .route(
            "/openai/v1/responses/:response_id",
            get(responses_resources::handle_openai_response_get)
                .delete(responses_resources::handle_openai_response_delete),
        )
        .route(
            "/openai/v1/responses/:response_id/cancel",
            post(responses_resources::handle_openai_response_cancel),
        )
        .route(
            "/openai/v1/conversations",
            post(responses_resources::handle_openai_conversations_create),
        )
        .route(
            "/openai/v1/conversations/:conversation_id",
            get(responses_resources::handle_openai_conversation_get)
                .post(responses_resources::handle_openai_conversation_update)
                .delete(responses_resources::handle_openai_conversation_delete),
        )
        .route(
            "/openai/v1/conversations/:conversation_id/items",
            get(responses_resources::handle_openai_conversation_items)
                .post(responses_resources::handle_openai_conversation_item_create),
        )
        .route(
            "/openai/v1/conversations/:conversation_id/items/:item_id",
            get(responses_resources::handle_openai_conversation_item_get)
                .delete(responses_resources::handle_openai_conversation_item_delete),
        )
        .route("/openai/v1/models", get(models::handle_openai_models))
        .route("/openai/v1/models/:id", get(models::handle_openai_model))
        .route(
            "/anthropic/v1/messages",
            post(proxy::handle_anthropic_messages),
        )
        .route("/anthropic/v1/models", get(models::handle_anthropic_models))
        .route(
            "/anthropic/v1/models/:id",
            get(models::handle_anthropic_model),
        )
        .route(
            "/namespaces/:namespace/openai/v1/chat/completions",
            post(proxy::handle_openai_chat_completions_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses",
            post(proxy::handle_openai_responses_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses/compact",
            post(responses_resources::handle_openai_responses_compact_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses/input_tokens",
            post(responses_resources::handle_openai_responses_input_tokens_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses/:response_id/input_items",
            get(responses_resources::handle_openai_response_input_items_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses/:response_id",
            get(responses_resources::handle_openai_response_get_namespaced)
                .delete(responses_resources::handle_openai_response_delete_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses/:response_id/cancel",
            post(responses_resources::handle_openai_response_cancel_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/conversations",
            post(responses_resources::handle_openai_conversations_create_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/conversations/:conversation_id",
            get(responses_resources::handle_openai_conversation_get_namespaced)
                .post(responses_resources::handle_openai_conversation_update_namespaced)
                .delete(responses_resources::handle_openai_conversation_delete_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/conversations/:conversation_id/items",
            get(responses_resources::handle_openai_conversation_items_namespaced)
                .post(responses_resources::handle_openai_conversation_item_create_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/conversations/:conversation_id/items/:item_id",
            get(responses_resources::handle_openai_conversation_item_get_namespaced)
                .delete(responses_resources::handle_openai_conversation_item_delete_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/models",
            get(models::handle_openai_models_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/models/:id",
            get(models::handle_openai_model_namespaced),
        )
        .route(
            "/namespaces/:namespace/anthropic/v1/messages",
            post(proxy::handle_anthropic_messages_namespaced),
        )
        .route(
            "/namespaces/:namespace/anthropic/v1/models",
            get(models::handle_anthropic_models_namespaced),
        )
        .route(
            "/namespaces/:namespace/anthropic/v1/models/:id",
            get(models::handle_anthropic_model_namespaced),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            data_auth::require_data_access,
        ));

    let data_router = Router::new()
        .route("/health", get(proxy::health))
        .route("/ready", get(proxy::ready))
        .merge(protected_data_router);
    let data_router = if let Some(cors) = cors {
        data_router.layer(cors)
    } else {
        data_router
    };

    let app = Router::new()
        .merge(admin_router)
        .merge(dashboard_router)
        .merge(data_router)
        .layer(middleware::from_fn(with_request_downstream_cancellation))
        .with_state(state);

    loop {
        let (tcp_stream, remote_addr) = listener.accept().await?;
        let (server_stream, watcher_stream) = duplicate_stream_for_disconnect_watch(tcp_stream)?;
        let (downstream_cancel_handle, downstream_cancel) = cancellation_channel();
        let tower_service = app
            .clone()
            .layer(Extension(remote_addr))
            .layer(Extension(downstream_cancel.clone()));

        tokio::spawn(async move {
            let watcher_cancel_handle = downstream_cancel_handle.clone();
            let disconnect_watcher = tokio::spawn(async move {
                watch_downstream_disconnect(watcher_stream, watcher_cancel_handle).await;
            });

            let hyper_service = service_fn(move |request: hyper::Request<Incoming>| {
                let mut tower_service = tower_service.clone();
                async move {
                    let response = tower::Service::call(&mut tower_service, request.map(Body::new))
                        .await
                        .unwrap_or_else(|err| match err {});
                    Ok::<_, Infallible>(response)
                }
            });

            let result = Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(server_stream), hyper_service)
                .await;

            downstream_cancel_handle.cancel();
            disconnect_watcher.abort();
            let _ = disconnect_watcher.await;

            if let Err(_err) = result {
                // Axum's default server ignores disconnect-related connection errors too.
            }
        });
    }
}

fn data_cors_layer_from_env() -> Result<Option<CorsLayer>, String> {
    let origins = data_auth::cors_allowed_origins_from_env()?;
    if origins.is_empty() {
        return Ok(None);
    }

    Ok(Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::DELETE,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers(AllowHeaders::list([
                header::ACCEPT,
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                HeaderName::from_static("x-api-key"),
                HeaderName::from_static("x-goog-api-key"),
                HeaderName::from_static("api-key"),
                HeaderName::from_static("openai-api-key"),
                HeaderName::from_static("anthropic-api-key"),
                HeaderName::from_static("anthropic-version"),
                HeaderName::from_static("anthropic-beta"),
                HeaderName::from_static("openai-organization"),
                HeaderName::from_static("openai-project"),
                HeaderName::from_static("idempotency-key"),
                HeaderName::from_static("x-stainless-helper-method"),
            ])),
    ))
}

fn duplicate_stream_for_disconnect_watch(
    stream: tokio::net::TcpStream,
) -> io::Result<(tokio::net::TcpStream, tokio::net::TcpStream)> {
    let std_stream = stream.into_std()?;
    std_stream.set_nonblocking(true)?;
    let watcher_stream = std_stream.try_clone()?;
    watcher_stream.set_nonblocking(true)?;
    Ok((
        tokio::net::TcpStream::from_std(std_stream)?,
        tokio::net::TcpStream::from_std(watcher_stream)?,
    ))
}

async fn watch_downstream_disconnect(
    stream: tokio::net::TcpStream,
    cancel_handle: DownstreamCancellationHandle,
) {
    let mut buf = [0u8; 1];

    loop {
        match stream.peek(&mut buf).await {
            Ok(0) => {
                cancel_handle.cancel();
                return;
            }
            Ok(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                cancel_handle.cancel();
                return;
            }
        }
    }
}

async fn with_request_downstream_cancellation(
    Extension(connection_cancellation): Extension<DownstreamCancellation>,
    request: Request,
    next: middleware::Next,
) -> Response {
    let (request_cancel_handle, request_cancellation) = connection_cancellation.child_channel();
    let (parts, body) = request.into_parts();
    let mut request = Request::from_parts(
        parts,
        wrap_body_with_cancellation(body, request_cancel_handle.clone()),
    );
    request.extensions_mut().insert(request_cancellation);

    let handler_guard = request_cancel_handle.drop_guard();
    let response = next.run(request).await;
    let _ = handler_guard.disarm();

    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        wrap_body_with_cancellation(body, request_cancel_handle),
    )
}

#[cfg(test)]
use admin::{authorize_admin_request, extract_bearer_token, handle_admin_namespace_state};
#[cfg(test)]
use errors::{
    append_compatibility_warning_headers, classify_post_translation_non_stream_status,
    error_response, normalize_upstream_error, normalized_non_stream_upstream_error,
    streaming_error_response, NormalizedUpstreamError,
};
#[cfg(test)]
use headers::extract_forwardable_headers;
#[cfg(test)]
use proxy::{
    classify_request_boundary, handle_request_core, handle_request_core_with_auth_context,
    resolve_requested_model_or_error, RequestBoundaryDecision, TestRequestCoreRequest,
};
#[cfg(test)]
use responses_resources::{
    resolve_native_responses_stateful_route_or_error, responses_stateful_request_controls,
};
#[cfg(test)]
use state::{RuntimeNamespaceState, RuntimeState, UpstreamState, DEFAULT_NAMESPACE};
