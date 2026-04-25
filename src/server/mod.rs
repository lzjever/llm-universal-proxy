//! HTTP server: single POST endpoint, format detection, proxy to upstream with optional translation.

mod admin;
mod errors;
mod headers;
mod models;
mod proxy;
mod public_boundary;
mod responses_resources;
mod state;
#[cfg(test)]
mod tests;
mod tracked_body;

use std::sync::Arc;
use std::{convert::Infallible, io, time::Duration};

use axum::{
    body::Body,
    extract::Request,
    middleware,
    response::Response,
    routing::{get, post},
    Extension, Router,
};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
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
        .route("/admin/state", get(admin::handle_admin_state))
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

    let data_router = Router::new()
        .route("/health", get(proxy::health))
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
            "/openai/v1/responses/:response_id",
            get(responses_resources::handle_openai_response_get)
                .delete(responses_resources::handle_openai_response_delete),
        )
        .route(
            "/openai/v1/responses/:response_id/cancel",
            post(responses_resources::handle_openai_response_cancel),
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
        .route("/google/v1beta/models", get(models::handle_google_models))
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
            "/namespaces/:namespace/openai/v1/responses/:response_id",
            get(responses_resources::handle_openai_response_get_namespaced)
                .delete(responses_resources::handle_openai_response_delete_namespaced),
        )
        .route(
            "/namespaces/:namespace/openai/v1/responses/:response_id/cancel",
            post(responses_resources::handle_openai_response_cancel_namespaced),
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
        .route(
            "/namespaces/:namespace/google/v1beta/models",
            get(models::handle_google_models_namespaced),
        )
        .route(
            "/google/v1beta/models/:id",
            get(models::handle_google_model).post(proxy::handle_google_model_action),
        )
        .route(
            "/namespaces/:namespace/google/v1beta/models/:id",
            get(models::handle_google_model_namespaced)
                .post(proxy::handle_google_model_action_namespaced),
        )
        .layer(cors);

    let app = Router::new()
        .merge(admin_router)
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
    classify_request_boundary, handle_request_core, resolve_requested_model_or_error,
    RequestBoundaryDecision,
};
#[cfg(test)]
use responses_resources::{
    resolve_native_responses_stateful_route_or_error, responses_stateful_request_controls,
};
#[cfg(test)]
use state::{RuntimeNamespaceState, RuntimeState, UpstreamState, DEFAULT_NAMESPACE};
