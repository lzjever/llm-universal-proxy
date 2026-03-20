//! HTTP server: single POST endpoint, format detection, proxy to upstream with optional translation.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Response, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use reqwest::Client;
use serde_json::Value;
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error, info};

use crate::config::{Config, UpstreamConfig};
use crate::detect::detect_request_format;
use crate::discovery::UpstreamCapability;
use crate::streaming::{needs_stream_translation, TranslateSseStream};
use crate::translate::{translate_request, translate_response};
use crate::upstream;
use futures_util::StreamExt;

pub async fn run_with_config_path(
    path: impl AsRef<std::path::Path>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::from_yaml_path(path).map_err(std::io::Error::other)?;
    run_with_config(config).await
}

pub async fn run_with_config(
    config: Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    config
        .validate()
        .map_err(|e| format!("invalid config: {}", e))?;
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
    run_with_listener(config, listener).await
}

/// Run the proxy on an already-bound listener. Used by integration tests to bind to port 0 and get the port.
pub async fn run_with_listener(
    config: Config,
    listener: tokio::net::TcpListener,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let upstreams = resolve_upstreams(&config).await;
    let client = upstream::build_client(&config);
    let state = Arc::new(AppState {
        config: config.clone(),
        upstreams,
        client,
    });
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
    upstreams: BTreeMap<String, UpstreamState>,
    client: Client,
}

#[derive(Clone)]
struct UpstreamState {
    config: UpstreamConfig,
    capability: UpstreamCapability,
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

async fn resolve_upstreams(config: &Config) -> BTreeMap<String, UpstreamState> {
    let mut upstreams = BTreeMap::new();
    for upstream in &config.upstreams {
        let capability = if let Some(f) = upstream.fixed_upstream_format {
            UpstreamCapability::fixed(f)
        } else {
            let supported = crate::discovery::discover_supported_formats(
                &upstream.base_url,
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

async fn handle_chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_chat_inner(state, headers, "/v1/chat/completions", body).await
}

async fn handle_responses(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_chat_inner(state, headers, "/v1/responses", body).await
}

async fn handle_chat_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    path: &str,
    mut body: Value,
) -> impl IntoResponse {
    debug!("Request path: {}", path);
    debug!(
        "Request body: {}",
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
    );
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let resolved_model = match state.config.resolve_model(&requested_model) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": { "message": e } })),
            )
                .into_response();
        }
    };
    let upstream_state = match state.upstreams.get(&resolved_model.upstream_name) {
        Some(v) => v,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("resolved upstream `{}` is not configured", resolved_model.upstream_name)
                    }
                })),
            )
                .into_response();
        }
    };

    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "model".to_string(),
            Value::String(resolved_model.upstream_model.clone()),
        );
    }
    let client_format = detect_request_format(path, &body);
    debug!("Detected client format: {:?}", client_format);
    let upstream_format = upstream_state
        .capability
        .upstream_format_for_request(client_format);

    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if client_format != upstream_format {
        if let Err(e) = translate_request(client_format, upstream_format, &model, &mut body, stream)
        {
            error!("Translation failed: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": { "message": e } })),
            )
                .into_response();
        }
    }
    debug!(
        "Translated body for upstream: {}",
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
    );

    // Extract auth headers to forward
    let mut auth_headers = extract_forwardable_headers(&headers);

    // Check if client provided any auth headers
    let has_client_auth = auth_headers.iter().any(|(k, _)| {
        let k = k.to_lowercase();
        k == "authorization"
            || k == "x-api-key"
            || k == "api-key"
            || k == "openai-api-key"
            || k == "x-goog-api-key"
    });

    // If client didn't provide auth, use configured upstream API key as fallback
    if !has_client_auth {
        if let Some(ref api_key) = upstream_state.config.fallback_api_key {
            debug!("No client auth provided, using upstream API key from config as fallback");
            // Add auth header in the format appropriate for the upstream
            let auth_header = auth_header_for_format(upstream_format, api_key);
            auth_headers.push(auth_header);
        }
    } else {
        debug!("Using client-provided auth headers");
        // Normalize client auth headers for upstream format
        normalize_auth_headers(&mut auth_headers, upstream_format);
    }
    apply_upstream_headers(
        &mut auth_headers,
        &upstream_state.config.upstream_headers,
        upstream_format,
    );

    let url = upstream::upstream_url(
        &state.config,
        &upstream_state.config,
        upstream_format,
        if upstream_format == crate::formats::UpstreamFormat::Google {
            Some(model.as_str())
        } else {
            None
        },
        stream,
    );
    debug!("Calling upstream URL: {}", url);
    let res = match upstream::call_upstream(&state.client, &url, &body, stream, &auth_headers).await
    {
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
        debug!("Upstream streaming response status: {}", status);
        if !status.is_success() {
            // For streaming requests with errors, read the body and return as error
            let error_body = res
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            error!(
                "Upstream returned error for streaming request: {} - {}",
                status, error_body
            );
            return (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                Json(serde_json::json!({ "error": { "message": error_body } })),
            )
                .into_response();
        }
        let upstream_stream = res.bytes_stream();
        let body = if needs_stream_translation(upstream_format, client_format) {
            let translated =
                TranslateSseStream::new(upstream_stream, upstream_format, client_format);
            Body::from_stream(translated.map(|r| r.map_err(std::io::Error::other)))
        } else {
            let pass = upstream_stream.map(|r| r.map_err(std::io::Error::other));
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
        error!("Upstream returned non-success status: {}", status);
        error!(
            "Upstream response body: {}",
            String::from_utf8_lossy(&bytes)
        );
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
                Json(
                    serde_json::json!({ "error": { "message": "upstream returned invalid JSON" } }),
                ),
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

/// Extract all headers that should be forwarded to upstream.
/// This forwards all headers except hop-by-hop headers and content-related ones.
fn extract_forwardable_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    // Headers that should NOT be forwarded
    const HOP_BY_HOP: &[&str] = &[
        "host",
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
        "transfer-encoding",
        "upgrade",
        "content-length",
        "content-type",
    ];

    let mut result = Vec::new();
    debug!("Extracting headers from request:");
    for (name, value) in headers.iter() {
        let name_str = name.as_str().to_lowercase();
        if !HOP_BY_HOP.contains(&name_str.as_str()) {
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
            debug!("Skipping hop-by-hop header: {}", name_str);
        }
    }
    debug!("Total headers to forward: {}", result.len());
    result
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
