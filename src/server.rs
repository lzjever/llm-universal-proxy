//! HTTP server: single POST endpoint, format detection, proxy to upstream with optional translation.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Response, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use reqwest::Client;
use serde_json::Value;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use crate::config::Config;
use crate::detect::detect_request_format;
use crate::discovery::UpstreamCapability;
use crate::translate::{translate_request, translate_response};
use crate::streaming::{needs_stream_translation, TranslateSseStream};
use crate::upstream;
use futures_util::StreamExt;

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::from_env();
    run_with_config(config).await
}

pub async fn run_with_config(config: Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive("llm_universal_proxy=info".parse()?))
        .init();

    let listen = config.listen.parse::<std::net::SocketAddr>().map_err(|e| format!("listen addr: {}", e))?;
    info!("listening on {}", listen);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    run_with_listener(config, listener).await
}

/// Run the proxy on an already-bound listener. Used by integration tests to bind to port 0 and get the port.
pub async fn run_with_listener(
    config: Config,
    listener: tokio::net::TcpListener,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let capability = resolve_capability(&config).await;
    let client = upstream::build_client(&config);
    let state = Arc::new(AppState {
        config: config.clone(),
        capability,
        client,
    });
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([axum::http::Method::GET, axum::http::Method::POST, axum::http::Method::OPTIONS])
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/v1/responses", post(handle_responses))
        .layer(cors)
        .with_state(state);

    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    config: Config,
    capability: UpstreamCapability,
    client: Client,
}

impl AppState {
    fn upstream_format_for_request(&self, client_format: crate::formats::UpstreamFormat) -> crate::formats::UpstreamFormat {
        self.capability.upstream_format_for_request(client_format)
    }
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

async fn resolve_capability(config: &Config) -> UpstreamCapability {
    if let Some(f) = config.fixed_upstream_format {
        UpstreamCapability::fixed(f)
    } else {
        let supported = crate::discovery::discover_supported_formats(
            &config.upstream_url,
            config.upstream_timeout,
        )
        .await;
        if supported.is_empty() {
            UpstreamCapability::fixed(crate::formats::UpstreamFormat::OpenAiCompletion)
        } else {
            UpstreamCapability::from_supported(supported)
        }
    }
}

async fn handle_chat_completions(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_chat_inner(state, "/v1/chat/completions", body).await
}

async fn handle_responses(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_chat_inner(state, "/v1/responses", body).await
}

async fn handle_chat_inner(
    state: Arc<AppState>,
    path: &str,
    mut body: Value,
) -> impl IntoResponse {
    let client_format = detect_request_format(path, &body);
    let upstream_format = state.upstream_format_for_request(client_format);

    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(true);
    let model = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
    if client_format != upstream_format {
        if let Err(e) = translate_request(
            client_format,
            upstream_format,
            &model,
            &mut body,
            stream,
        ) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": { "message": e } })),
            )
                .into_response();
        }
    }

    let url = upstream::upstream_url(
        &state.config,
        upstream_format,
        if upstream_format == crate::formats::UpstreamFormat::Google {
            Some(model.as_str())
        } else {
            None
        },
    );
    let res = match upstream::call_upstream(&state.client, &url, &body, stream).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": { "message": e.to_string() } })),
            )
                .into_response();
        }
    };

    if stream {
        let status = res.status();
        let upstream_stream = res.bytes_stream();
        let body = if needs_stream_translation(upstream_format, client_format) {
            let translated = TranslateSseStream::new(
                upstream_stream,
                upstream_format,
                client_format,
            );
            Body::from_stream(translated.map(|r| {
                r.map(axum::body::Bytes::from).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
            }))
        } else {
            let pass = upstream_stream.map(|r| {
                r.map(axum::body::Bytes::from).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
            });
            Body::from_stream(pass)
        };
        return Response::builder()
            .status(status)
            .header("Content-Type", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .header("Connection", "keep-alive")
            .body(body)
            .unwrap()
            .into_response();
    }

    let status = res.status();
    let bytes = match res.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": { "message": e.to_string() } })),
            )
                .into_response();
        }
    };
    if !status.is_success() {
        return (
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(serde_json::json!({ "error": { "message": String::from_utf8_lossy(&bytes) } })),
        )
            .into_response();
    }
    let upstream_body: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": { "message": "upstream returned invalid JSON" } })),
            )
                .into_response();
        }
    };
    let out = match translate_response(upstream_format, client_format, &upstream_body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": { "message": e } })),
            )
                .into_response();
        }
    };
    (StatusCode::OK, Json(out)).into_response()
}
