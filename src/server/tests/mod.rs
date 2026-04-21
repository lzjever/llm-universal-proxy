pub(super) use std::collections::BTreeMap;
pub(super) use std::sync::Arc;

pub(super) use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
pub(super) use reqwest::Client;
pub(super) use serde_json::Value;
pub(super) use tokio::sync::{Mutex, RwLock};

pub(super) use super::*;

mod admin;
mod errors;
mod headers;
mod proxy;
mod responses_resources;
mod state;

pub(super) struct ScopedEnvVar {
    key: &'static str,
    previous: Option<String>,
}

impl ScopedEnvVar {
    pub(super) fn set(key: &'static str, value: impl AsRef<str>) -> Self {
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

pub(super) fn test_upstream_config(
    name: &str,
    format: crate::formats::UpstreamFormat,
) -> crate::config::UpstreamConfig {
    test_upstream_config_with_fixed_format(name, Some(format))
}

pub(super) fn test_upstream_config_with_fixed_format(
    name: &str,
    fixed_upstream_format: Option<crate::formats::UpstreamFormat>,
) -> crate::config::UpstreamConfig {
    crate::config::UpstreamConfig {
        name: name.to_string(),
        api_root: format!("https://{name}.example/v1"),
        fixed_upstream_format,
        fallback_credential_env: None,
        fallback_credential_actual: None,
        fallback_api_key: None,
        auth_policy: crate::config::AuthPolicy::ClientOrFallback,
        upstream_headers: Vec::new(),
        limits: None,
        surface_defaults: None,
    }
}

pub(super) fn runtime_namespace_state_for_tests(
    upstreams: &[(&str, crate::formats::UpstreamFormat, bool)],
) -> RuntimeNamespaceState {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        upstreams: upstreams
            .iter()
            .map(|(name, format, _)| test_upstream_config(name, *format))
            .collect(),
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let upstream_states = upstreams
        .iter()
        .map(|(name, format, available)| {
            (
                (*name).to_string(),
                UpstreamState {
                    config: test_upstream_config(name, *format),
                    capability: Some(crate::discovery::UpstreamCapability::fixed(*format)),
                    availability: if *available {
                        crate::discovery::UpstreamAvailability::available()
                    } else {
                        crate::discovery::UpstreamAvailability::unavailable("test outage")
                    },
                },
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    RuntimeNamespaceState {
        revision: "test-revision".to_string(),
        config,
        upstreams: upstream_states,
        client: Client::new(),
        streaming_client: Client::new(),
        hooks: None,
        debug_trace: None,
    }
}

pub(super) async fn spawn_openai_completion_mock(
    response_body: Value,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        response_body: Value,
    }

    async fn handle_chat_completions(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.requests.lock().await.push(body);
        Json(state.response_body)
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(MockState {
            requests: requests.clone(),
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock upstream");
    let addr = listener.local_addr().expect("mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock server");
    });

    (format!("http://{addr}"), requests, server)
}

pub(super) async fn spawn_anthropic_messages_mock(
    response_body: Value,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        response_body: Value,
    }

    async fn handle_messages(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.requests.lock().await.push(body);
        Json(state.response_body)
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/messages", post(handle_messages))
        .route("/messages", post(handle_messages))
        .with_state(MockState {
            requests: requests.clone(),
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind anthropic mock upstream");
    let addr = listener.local_addr().expect("anthropic mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("anthropic mock server");
    });

    (format!("http://{addr}"), requests, server)
}

pub(super) fn app_state_for_single_upstream(
    api_root: String,
    upstream_format: crate::formats::UpstreamFormat,
) -> Arc<AppState> {
    app_state_for_single_upstream_with_timeout(
        api_root,
        upstream_format,
        std::time::Duration::from_secs(30),
    )
}

pub(super) fn app_state_for_single_upstream_with_timeout(
    api_root: String,
    upstream_format: crate::formats::UpstreamFormat,
    upstream_timeout: std::time::Duration,
) -> Arc<AppState> {
    let upstream = crate::config::UpstreamConfig {
        name: "primary".to_string(),
        api_root,
        fixed_upstream_format: Some(upstream_format),
        fallback_credential_env: None,
        fallback_credential_actual: None,
        fallback_api_key: None,
        auth_policy: crate::config::AuthPolicy::ClientOrFallback,
        upstream_headers: Vec::new(),
        limits: None,
        surface_defaults: None,
    };
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout,
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        upstreams: vec![upstream.clone()],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let runtime = RuntimeState {
        namespaces: BTreeMap::from([(
            DEFAULT_NAMESPACE.to_string(),
            RuntimeNamespaceState {
                revision: "test-revision".to_string(),
                config: config.clone(),
                upstreams: BTreeMap::from([(
                    upstream.name.clone(),
                    UpstreamState {
                        config: upstream,
                        capability: Some(crate::discovery::UpstreamCapability::fixed(
                            upstream_format,
                        )),
                        availability: crate::discovery::UpstreamAvailability::available(),
                    },
                )]),
                client: crate::upstream::build_client(&config),
                streaming_client: crate::upstream::build_streaming_client(&config),
                hooks: None,
                debug_trace: None,
            },
        )]),
    };

    Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        metrics: crate::telemetry::RuntimeMetrics::new(&crate::config::Config::default()),
        admin_access: AdminAccess::LoopbackOnly,
    })
}

pub(super) async fn spawn_delayed_openai_completion_stream_mock(
    tail_delay: std::time::Duration,
) -> (String, tokio::task::JoinHandle<()>) {
    use bytes::Bytes;
    use futures_util::stream;

    #[derive(Clone)]
    struct SlowMockState {
        tail_delay: std::time::Duration,
    }

    async fn handle_chat_completions(
        State(state): State<SlowMockState>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        let stream_enabled = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        if !stream_enabled {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": "chatcmpl-slow",
                    "object": "chat.completion",
                    "created": 1,
                    "model": body.get("model").cloned().unwrap_or_else(|| serde_json::json!("mock")),
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": "Hi" },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })),
            )
                .into_response();
        }

        let pieces = vec![
            Ok::<Bytes, std::io::Error>(Bytes::from_static(
                br#"data: {"id":"chatcmpl-slow","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}"#,
            )),
            Ok(Bytes::from_static(b"\n\n")),
            Ok(Bytes::from_static(
                br#"data: {"id":"chatcmpl-slow","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}]}"#,
            )),
            Ok(Bytes::from_static(b"\n\n")),
            Ok(Bytes::from_static(
                br#"data: {"id":"chatcmpl-slow","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            )),
            Ok(Bytes::from_static(b"\n\n")),
            Ok(Bytes::from_static(b"data: [DONE]\n\n")),
        ];
        let delay = state.tail_delay;
        let body_stream =
            stream::unfold(pieces.into_iter().enumerate(), move |mut iter| async move {
                if let Some((idx, chunk)) = iter.next() {
                    if idx >= 2 {
                        tokio::time::sleep(delay).await;
                    }
                    Some((chunk, iter))
                } else {
                    None
                }
            });
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(body_stream))
            .expect("streaming response")
    }

    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(SlowMockState { tail_delay });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed mock upstream");
    let addr = listener.local_addr().expect("delayed mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("delayed mock server");
    });

    (format!("http://{addr}"), server)
}
