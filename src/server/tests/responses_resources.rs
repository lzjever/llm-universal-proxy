use super::*;
use crate::server::responses_resources::{
    handle_openai_responses_resource, handle_openai_responses_resource_with_auth_context,
    TestOpenAiResponsesResourceRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type RecordedStreamRequests = Arc<Mutex<Vec<(String, Option<String>)>>>;
type RecordedResourceRequests = Arc<Mutex<Vec<RecordedResourceRequest>>>;

#[derive(Debug, Clone)]
struct RecordedResourceRequest {
    method: String,
    uri: String,
    body: Option<Value>,
    authorization: Option<String>,
    helper_method: Option<String>,
    openai_organization: Option<String>,
    openai_project: Option<String>,
    idempotency_key: Option<String>,
    data_token: Option<String>,
    content_type: Option<String>,
}

#[derive(Clone)]
struct RecordedResponsesResourceStreamState {
    status: StatusCode,
    content_type: String,
    body: String,
    seen_requests: RecordedStreamRequests,
}

async fn spawn_raw_responses_resource_mock(
    status: StatusCode,
    body: impl Into<String>,
) -> (String, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct RawState {
        status: StatusCode,
        body: String,
    }

    async fn handle_resource(State(state): State<RawState>) -> Response<Body> {
        Response::builder()
            .status(state.status)
            .header("Content-Type", "application/json")
            .body(Body::from(state.body.clone()))
            .expect("raw resource response")
    }

    let app = Router::new()
        .route(
            "/responses/:id",
            axum::routing::get(handle_resource).delete(handle_resource),
        )
        .route("/responses/:id/cancel", post(handle_resource))
        .route("/responses/compact", post(handle_resource))
        .with_state(RawState {
            status,
            body: body.into(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind raw responses resource mock");
    let addr = listener.local_addr().expect("raw resource mock addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("raw resource mock server");
    });

    (format!("http://{addr}"), server)
}

async fn spawn_recorded_responses_resource_stream_mock(
    status: StatusCode,
    content_type: impl Into<String>,
    body: impl Into<String>,
) -> (String, RecordedStreamRequests, tokio::task::JoinHandle<()>) {
    async fn handle_resource(
        axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
        headers: HeaderMap,
        State(state): State<RecordedResponsesResourceStreamState>,
    ) -> Response<Body> {
        let accept = headers
            .get(axum::http::header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        state
            .seen_requests
            .lock()
            .await
            .push((uri.to_string(), accept));
        Response::builder()
            .status(state.status)
            .header("Content-Type", state.content_type.as_str())
            .body(Body::from(state.body.clone()))
            .expect("streaming resource response")
    }

    let seen_requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/responses/:id", axum::routing::get(handle_resource))
        .with_state(RecordedResponsesResourceStreamState {
            status,
            content_type: content_type.into(),
            body: body.into(),
            seen_requests: seen_requests.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind streaming responses resource mock");
    let addr = listener.local_addr().expect("stream resource mock addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("streaming responses resource mock server");
    });

    (format!("http://{addr}"), seen_requests, server)
}

async fn spawn_recording_responses_resource_mock() -> (
    String,
    RecordedResourceRequests,
    tokio::task::JoinHandle<()>,
) {
    #[derive(Clone)]
    struct RecordingState {
        requests: RecordedResourceRequests,
    }

    async fn handle_resource(
        State(state): State<RecordingState>,
        request: axum::extract::Request,
    ) -> Response<Body> {
        let (parts, body) = request.into_parts();
        let body_bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("record upstream request body");
        let body = if body_bytes.is_empty() {
            None
        } else {
            Some(serde_json::from_slice(&body_bytes).expect("json upstream request body"))
        };
        let header_value = |name: &str| {
            parts
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string)
        };
        state.requests.lock().await.push(RecordedResourceRequest {
            method: parts.method.to_string(),
            uri: parts
                .uri
                .path_and_query()
                .map(|path| path.as_str().to_string())
                .unwrap_or_else(|| parts.uri.path().to_string()),
            body,
            authorization: header_value("authorization"),
            helper_method: header_value("x-stainless-helper-method"),
            openai_organization: header_value("openai-organization"),
            openai_project: header_value("openai-project"),
            idempotency_key: header_value("idempotency-key"),
            data_token: header_value(data_auth::LEGACY_DATA_TOKEN_HEADER),
            content_type: header_value("content-type"),
        });

        if parts.method == axum::http::Method::DELETE {
            return Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .expect("delete resource response");
        }

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(Body::from(
                r#"{"id":"resource_ok","object":"list","data":[],"has_more":false}"#,
            ))
            .expect("recording resource response")
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/*path", axum::routing::any(handle_resource))
        .with_state(RecordingState {
            requests: requests.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind recording responses resource mock");
    let addr = listener.local_addr().expect("recording resource mock addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("recording responses resource mock server");
    });

    (format!("http://{addr}"), requests, server)
}

async fn call_raw_responses_resource(
    status: StatusCode,
    upstream_body: &'static str,
    method: reqwest::Method,
    resource_path: &str,
    request_body: Option<Value>,
) -> (Response<Body>, tokio::task::JoinHandle<()>) {
    let (mock_base, server) = spawn_raw_responses_resource_mock(status, upstream_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        method,
        resource_path.to_string(),
        request_body,
        None,
    )
    .await;

    (response, server)
}

async fn spawn_raw_tcp_responses_resource_upstream(
    raw_response: Vec<u8>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind raw tcp responses resource mock");
    let addr = listener.local_addr().expect("raw tcp resource mock addr");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept raw tcp request");
        let mut request = Vec::new();
        let mut buf = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buf).await.expect("read raw tcp request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buf[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        stream
            .write_all(&raw_response)
            .await
            .expect("write raw tcp response");
        let _ = stream.shutdown().await;
    });

    (format!("http://{addr}"), server)
}

fn raw_no_content_response(status: StatusCode, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let reason = status.canonical_reason().unwrap_or("No Content");
    let mut response = format!(
        "HTTP/1.1 {} {reason}\r\nConnection: close\r\n",
        status.as_u16()
    )
    .into_bytes();
    for (name, value) in headers {
        response.extend_from_slice(name.as_bytes());
        response.extend_from_slice(b": ");
        response.extend_from_slice(value.as_bytes());
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b"\r\n");
    response.extend_from_slice(body);
    response
}

async fn call_raw_tcp_responses_resource(
    raw_response: Vec<u8>,
) -> (Response<Body>, tokio::task::JoinHandle<()>) {
    let (mock_base, server) = spawn_raw_tcp_responses_resource_upstream(raw_response).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::DELETE,
        "responses/resp_no_content".to_string(),
        None,
        None,
    )
    .await;

    (response, server)
}

async fn response_body_text(response: Response<Body>) -> String {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body bytes");
    String::from_utf8(body.to_vec()).expect("response body utf8")
}

async fn responses_resource_redaction_state(api_root: &str) -> Arc<AppState> {
    app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "responses",
            api_root,
            crate::formats::UpstreamFormat::OpenAiResponses,
            None,
            Some(crate::config::SecretSourceConfig {
                inline: Some(PROVIDER_INLINE_REDACTION_SECRET.to_string()),
                env: None,
            }),
        )],
        data_auth::DataAccess::ProxyKey {
            key: PROXY_INLINE_REDACTION_SECRET.to_string(),
        },
    )
    .await
}

fn responses_resource_error_body_with_secrets() -> String {
    serde_json::json!({
        "error": {
            "message": format!(
                "responses resource upstream echoed provider {PROVIDER_INLINE_REDACTION_SECRET} and proxy {PROXY_INLINE_REDACTION_SECRET}"
            )
        }
    })
    .to_string()
}

fn assert_resource_response_redacted(body_text: &str, context: &str) {
    for secret in [
        PROVIDER_INLINE_REDACTION_SECRET,
        PROXY_INLINE_REDACTION_SECRET,
    ] {
        assert!(
            !body_text.contains(secret),
            "{context} leaked {secret}: {body_text}"
        );
    }
    assert!(
        body_text.contains("[REDACTED]"),
        "{context} should show redacted placeholder: {body_text}"
    );
}

async fn assert_resource_metrics_redacted(state: &Arc<AppState>, secrets: &[&str], context: &str) {
    let config = state
        .runtime
        .read()
        .await
        .namespaces
        .get(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .config
        .clone();
    let metrics_text = format!("{:?}", state.metrics.snapshot(&config));
    for secret in secrets {
        assert!(
            !metrics_text.contains(secret),
            "{context} metrics leaked {secret}: {metrics_text}"
        );
    }
    assert!(
        metrics_text.contains("[REDACTED]"),
        "{context} metrics should show redacted placeholder: {metrics_text}"
    );
}

async fn responses_resource_redaction_state_with(
    api_root: &str,
    provider_secret: Option<&str>,
    data_access: data_auth::DataAccess,
) -> Arc<AppState> {
    app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "responses",
            api_root,
            crate::formats::UpstreamFormat::OpenAiResponses,
            None,
            provider_secret.map(|secret| crate::config::SecretSourceConfig {
                inline: Some(secret.to_string()),
                env: None,
            }),
        )],
        data_access,
    )
    .await
}

fn responses_resource_success_body_with_text(text: String) -> String {
    serde_json::json!({
        "id": "resp_success_redaction",
        "object": "response",
        "status": "completed",
        "output": [{
            "type": "message",
            "id": "msg_success_redaction",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": text,
                "annotations": []
            }]
        }]
    })
    .to_string()
}

async fn send_raw_proxy_resource_request(
    proxy_addr: std::net::SocketAddr,
    method: &reqwest::Method,
    path: &str,
    label: &str,
    body: Option<&Value>,
) -> (StatusCode, String) {
    let mut stream = tokio::net::TcpStream::connect(proxy_addr)
        .await
        .expect("connect proxy");
    let body_bytes = body
        .map(|body| serde_json::to_vec(body).expect("serialize raw proxy body"))
        .unwrap_or_default();
    let mut request = format!(
        "{} {path} HTTP/1.1\r\nHost: {proxy_addr}\r\nopenai-api-key: client-secret\r\nx-stainless-helper-method: {label}\r\nOpenAI-Organization: org-route-preserve\r\nOpenAI-Project: proj-route-preserve\r\nIdempotency-Key: idem-{label}\r\nConnection: close\r\n",
        method.as_str()
    );
    if body.is_some() {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body_bytes.len()));
    }
    request.push_str("\r\n");

    stream
        .write_all(request.as_bytes())
        .await
        .expect("write raw proxy request head");
    if !body_bytes.is_empty() {
        stream
            .write_all(&body_bytes)
            .await
            .expect("write raw proxy request body");
    }

    let mut raw_response = Vec::new();
    stream
        .read_to_end(&mut raw_response)
        .await
        .expect("read raw proxy response");
    let response_text = String::from_utf8_lossy(&raw_response).to_string();
    let status = response_text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .and_then(|status| StatusCode::from_u16(status).ok())
        .expect("raw proxy response status");
    let body_text = response_text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, body_text)
}

async fn set_resource_limits(state: &Arc<AppState>, limits: crate::config::ResourceLimits) {
    let mut runtime = state.runtime.write().await;
    runtime
        .namespaces
        .get_mut(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .config
        .resource_limits = limits;
}

fn format_sse_event(event_type: &str, data: &Value) -> String {
    format!("event: {event_type}\ndata: {data}\n\n")
}

async fn assert_raw_no_content_framing_fails_closed(raw_response: Vec<u8>) -> String {
    let (response, server) = call_raw_tcp_responses_resource(raw_response).await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body_text = response_body_text(response).await;
    let body: Value = serde_json::from_str(&body_text).expect("OpenAI error envelope");
    assert_eq!(
        body["error"]["message"], "upstream returned invalid no-content response framing",
        "{body_text}"
    );

    server.abort();
    body_text
}

async fn assert_empty_success_body_fails_closed(
    method: reqwest::Method,
    resource_path: &str,
    request_body: Option<Value>,
) {
    let (response, server) = call_raw_responses_resource(
        StatusCode::OK,
        "",
        method.clone(),
        resource_path,
        request_body,
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "{method} {resource_path}"
    );
    let body_text = response_body_text(response).await;
    let body: Value = serde_json::from_str(&body_text).expect("OpenAI error envelope");
    assert_eq!(
        body["error"]["message"], "upstream returned empty response body",
        "{body_text}"
    );

    server.abort();
}

#[tokio::test]
async fn openai_responses_resource_post_body_limit_rejects_all_json_body_handlers_before_upstream()
{
    let (mock_base, recorded, upstream_server) = spawn_recording_responses_resource_mock().await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);
    set_resource_limits(
        &state,
        crate::config::ResourceLimits {
            max_request_body_bytes: 96,
            ..Default::default()
        },
    )
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy listener");
    let proxy_addr = listener.local_addr().expect("proxy addr");
    let proxy_server = tokio::spawn(async move { run_server(state, listener).await });
    let proxy_base = format!("http://{proxy_addr}");
    let client = reqwest::Client::new();

    let oversized_secret = "RESOURCE_REQUEST_SENTINEL_SHOULD_NOT_LEAK";
    let oversized_text = format!("{oversized_secret}{}", "x".repeat(512));
    let cases = [
        (
            "compact",
            "/openai/v1/responses/compact",
            serde_json::json!({ "input": oversized_text }),
        ),
        (
            "input_tokens",
            "/openai/v1/responses/input_tokens",
            serde_json::json!({ "input": oversized_text }),
        ),
        (
            "conversation create",
            "/openai/v1/conversations",
            serde_json::json!({ "metadata": { "note": oversized_text } }),
        ),
        (
            "conversation update",
            "/openai/v1/conversations/conv_body_limit",
            serde_json::json!({ "metadata": { "note": oversized_text } }),
        ),
        (
            "conversation item create",
            "/openai/v1/conversations/conv_body_limit/items",
            serde_json::json!({
                "items": [{
                    "type": "message",
                    "role": "user",
                    "content": oversized_text
                }]
            }),
        ),
    ];

    for (label, path, body) in cases {
        let response = client
            .post(format!("{proxy_base}{path}"))
            .header("openai-api-key", "client-secret")
            .json(&body)
            .send()
            .await
            .expect("proxy response");

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE, "{label}");
        let body_text = response.text().await.expect("error body");
        assert!(
            body_text.contains("request body exceeded"),
            "{label}: {body_text}"
        );
        assert!(
            !body_text.contains(oversized_secret),
            "{label}: {body_text}"
        );
    }

    assert!(recorded.lock().await.is_empty());
    proxy_server.abort();
    upstream_server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_non_stream_success_body_limit_fails_closed_without_payload_leak(
) {
    let upstream_sentinel = "RESOURCE_NON_STREAM_SENTINEL_SHOULD_NOT_LEAK";
    let raw_body = serde_json::json!({
        "id": "resp_large",
        "object": "response",
        "status": "completed",
        "output": [{
            "type": "message",
            "id": "msg_large",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": format!("{upstream_sentinel}{}", "x".repeat(1024)),
                "annotations": []
            }]
        }]
    })
    .to_string();
    let (mock_base, server) = spawn_raw_responses_resource_mock(StatusCode::OK, raw_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);
    set_resource_limits(
        &state,
        crate::config::ResourceLimits {
            max_non_stream_response_bytes: 256,
            ..Default::default()
        },
    )
    .await;

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_large".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body_text = response_body_text(response).await;
    assert!(
        body_text.contains("upstream response body exceeded"),
        "{body_text}"
    );
    assert!(!body_text.contains(upstream_sentinel), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_upstream_error_body_limit_fails_closed_without_payload_leak(
) {
    let upstream_sentinel = "RESOURCE_ERROR_SENTINEL_SHOULD_NOT_LEAK";
    let raw_body = format!("{upstream_sentinel}{}", "x".repeat(1024));
    let (mock_base, server) =
        spawn_raw_responses_resource_mock(StatusCode::BAD_REQUEST, raw_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);
    set_resource_limits(
        &state,
        crate::config::ResourceLimits {
            max_upstream_error_body_bytes: 64,
            ..Default::default()
        },
    )
    .await;

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_error".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body_text = response_body_text(response).await;
    assert!(
        body_text.contains("upstream error body exceeded"),
        "{body_text}"
    );
    assert!(!body_text.contains(upstream_sentinel), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_stream_true_uses_configured_sse_frame_limit() {
    let upstream_sentinel = "RESOURCE_STREAM_FRAME_SENTINEL_SHOULD_NOT_LEAK";
    let oversized_event = serde_json::json!({
        "type": "response.output_text.delta",
        "sequence_number": 0,
        "response_id": "resp_stream_frame_limit",
        "output_index": 0,
        "content_index": 0,
        "delta": format!("{upstream_sentinel}{}", "x".repeat(128))
    });
    let upstream_body = format_sse_event("response.output_text.delta", &oversized_event);
    let (mock_base, _seen_requests, server) = spawn_recorded_responses_resource_stream_mock(
        StatusCode::OK,
        "text/event-stream",
        upstream_body,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);
    set_resource_limits(
        &state,
        crate::config::ResourceLimits {
            max_sse_frame_bytes: 64,
            ..Default::default()
        },
    )
    .await;

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_stream_frame_limit".to_string(),
        None,
        Some("stream=true".to_string()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_body_text(response).await;
    assert!(
        body_text.contains("\"code\":\"upstream_sse_frame_too_large\""),
        "{body_text}"
    );
    assert!(!body_text.contains(upstream_sentinel), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_stream_true_uses_configured_event_limit() {
    let first_sentinel = "RESOURCE_STREAM_FIRST_EVENT_ALLOWED";
    let second_sentinel = "RESOURCE_STREAM_SECOND_EVENT_SHOULD_NOT_LEAK";
    let first = serde_json::json!({
        "type": "response.output_text.delta",
        "sequence_number": 0,
        "response_id": "resp_stream_event_limit",
        "output_index": 0,
        "content_index": 0,
        "delta": first_sentinel
    });
    let second = serde_json::json!({
        "type": "response.output_text.delta",
        "sequence_number": 1,
        "response_id": "resp_stream_event_limit",
        "output_index": 0,
        "content_index": 0,
        "delta": second_sentinel
    });
    let upstream_body = [
        format_sse_event("response.output_text.delta", &first),
        format_sse_event("response.output_text.delta", &second),
    ]
    .concat();
    let (mock_base, _seen_requests, server) = spawn_recorded_responses_resource_stream_mock(
        StatusCode::OK,
        "text/event-stream",
        upstream_body,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);
    set_resource_limits(
        &state,
        crate::config::ResourceLimits {
            stream_max_events: 1,
            ..Default::default()
        },
    )
    .await;

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_stream_event_limit".to_string(),
        None,
        Some("stream=true".to_string()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_body_text(response).await;
    assert!(body_text.contains(first_sentinel), "{body_text}");
    assert!(
        body_text.contains("\"code\":\"upstream_stream_event_limit_exceeded\""),
        "{body_text}"
    );
    assert!(!body_text.contains(second_sentinel), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn openai_responses_state_resource_routes_preserve_method_path_query_body_and_headers() {
    let (mock_base, recorded, upstream_server) = spawn_recording_responses_resource_mock().await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);
    {
        let mut runtime = state.runtime.write().await;
        let default_namespace = runtime
            .namespaces
            .get(DEFAULT_NAMESPACE)
            .expect("default namespace")
            .clone();
        runtime
            .namespaces
            .insert("tenant".to_string(), default_namespace);
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy listener");
    let proxy_addr = listener.local_addr().expect("proxy addr");
    let proxy_server = tokio::spawn(async move { run_server(state, listener).await });
    let proxy_base = format!("http://{proxy_addr}");
    let client = reqwest::Client::new();

    let post_body = serde_json::json!({
        "input": [{ "role": "user", "content": "hello" }],
        "metadata": { "source": "route-test" }
    });
    let cases = vec![
        (
            "default response input items",
            reqwest::Method::GET,
            "/openai/v1/responses/resp%2Fstate%3Fowned/input_items?after=item%2F1&limit=2",
            "/responses/resp%2Fstate%3Fowned/input_items?after=item%2F1&limit=2",
            None,
        ),
        (
            "default response input items dot segment",
            reqwest::Method::GET,
            "/openai/v1/responses/%2E%2E/input_items?after=%2E",
            "/responses/%2E%2E/input_items?after=%2E",
            None,
        ),
        (
            "default input tokens",
            reqwest::Method::POST,
            "/openai/v1/responses/input_tokens?trace=keep",
            "/responses/input_tokens?trace=keep",
            Some(post_body.clone()),
        ),
        (
            "default create conversation",
            reqwest::Method::POST,
            "/openai/v1/conversations?trace=keep",
            "/conversations?trace=keep",
            Some(serde_json::json!({ "metadata": { "tenant": "default" } })),
        ),
        (
            "default get conversation",
            reqwest::Method::GET,
            "/openai/v1/conversations/conv%2Fstate%3Fowned?include=items",
            "/conversations/conv%2Fstate%3Fowned?include=items",
            None,
        ),
        (
            "default update conversation",
            reqwest::Method::POST,
            "/openai/v1/conversations/conv%2Fstate%3Fowned?trace=keep",
            "/conversations/conv%2Fstate%3Fowned?trace=keep",
            Some(serde_json::json!({ "metadata": { "phase": "update" } })),
        ),
        (
            "default delete conversation",
            reqwest::Method::DELETE,
            "/openai/v1/conversations/conv%2Fstate%3Fowned?trace=keep",
            "/conversations/conv%2Fstate%3Fowned?trace=keep",
            None,
        ),
        (
            "default list conversation items",
            reqwest::Method::GET,
            "/openai/v1/conversations/conv%2Fstate%3Fowned/items?after=item%2F0",
            "/conversations/conv%2Fstate%3Fowned/items?after=item%2F0",
            None,
        ),
        (
            "default create conversation item",
            reqwest::Method::POST,
            "/openai/v1/conversations/conv%2Fstate%3Fowned/items?trace=keep",
            "/conversations/conv%2Fstate%3Fowned/items?trace=keep",
            Some(serde_json::json!({
                "items": [{ "type": "message", "role": "user", "content": "item" }]
            })),
        ),
        (
            "default get conversation item",
            reqwest::Method::GET,
            "/openai/v1/conversations/conv%2Fstate%3Fowned/items/item%2Fstate%3Fowned?include=content",
            "/conversations/conv%2Fstate%3Fowned/items/item%2Fstate%3Fowned?include=content",
            None,
        ),
        (
            "default get conversation item dot segments",
            reqwest::Method::GET,
            "/openai/v1/conversations/%2E/items/%2E%2E",
            "/conversations/%2E/items/%2E%2E",
            None,
        ),
        (
            "default delete conversation item",
            reqwest::Method::DELETE,
            "/openai/v1/conversations/conv%2Fstate%3Fowned/items/item%2Fstate%3Fowned?trace=keep",
            "/conversations/conv%2Fstate%3Fowned/items/item%2Fstate%3Fowned?trace=keep",
            None,
        ),
        (
            "namespaced response input items",
            reqwest::Method::GET,
            "/namespaces/tenant/openai/v1/responses/resp%2Ftenant%3Fowned/input_items?after=item%2F1",
            "/responses/resp%2Ftenant%3Fowned/input_items?after=item%2F1",
            None,
        ),
        (
            "namespaced response input items dot segment",
            reqwest::Method::GET,
            "/namespaces/tenant/openai/v1/responses/%2E%2E/input_items",
            "/responses/%2E%2E/input_items",
            None,
        ),
        (
            "namespaced input tokens",
            reqwest::Method::POST,
            "/namespaces/tenant/openai/v1/responses/input_tokens?trace=tenant",
            "/responses/input_tokens?trace=tenant",
            Some(post_body),
        ),
        (
            "namespaced create conversation",
            reqwest::Method::POST,
            "/namespaces/tenant/openai/v1/conversations?trace=tenant",
            "/conversations?trace=tenant",
            Some(serde_json::json!({ "metadata": { "tenant": "tenant" } })),
        ),
        (
            "namespaced get conversation",
            reqwest::Method::GET,
            "/namespaces/tenant/openai/v1/conversations/conv%2Ftenant%3Fowned?include=items",
            "/conversations/conv%2Ftenant%3Fowned?include=items",
            None,
        ),
        (
            "namespaced update conversation",
            reqwest::Method::POST,
            "/namespaces/tenant/openai/v1/conversations/conv%2Ftenant%3Fowned",
            "/conversations/conv%2Ftenant%3Fowned",
            Some(serde_json::json!({ "metadata": { "phase": "tenant-update" } })),
        ),
        (
            "namespaced delete conversation",
            reqwest::Method::DELETE,
            "/namespaces/tenant/openai/v1/conversations/conv%2Ftenant%3Fowned?trace=tenant",
            "/conversations/conv%2Ftenant%3Fowned?trace=tenant",
            None,
        ),
        (
            "namespaced list conversation items",
            reqwest::Method::GET,
            "/namespaces/tenant/openai/v1/conversations/conv%2Ftenant%3Fowned/items?after=item%2F0",
            "/conversations/conv%2Ftenant%3Fowned/items?after=item%2F0",
            None,
        ),
        (
            "namespaced create conversation item",
            reqwest::Method::POST,
            "/namespaces/tenant/openai/v1/conversations/conv%2Ftenant%3Fowned/items",
            "/conversations/conv%2Ftenant%3Fowned/items",
            Some(serde_json::json!({
                "items": [{ "type": "message", "role": "user", "content": "tenant item" }]
            })),
        ),
        (
            "namespaced get conversation item",
            reqwest::Method::GET,
            "/namespaces/tenant/openai/v1/conversations/conv%2Ftenant%3Fowned/items/item%2Ftenant%3Fowned?include=content",
            "/conversations/conv%2Ftenant%3Fowned/items/item%2Ftenant%3Fowned?include=content",
            None,
        ),
        (
            "namespaced get conversation item dot segments",
            reqwest::Method::GET,
            "/namespaces/tenant/openai/v1/conversations/%2E/items/%2E%2E",
            "/conversations/%2E/items/%2E%2E",
            None,
        ),
        (
            "namespaced delete conversation item",
            reqwest::Method::DELETE,
            "/namespaces/tenant/openai/v1/conversations/conv%2Ftenant%3Fowned/items/item%2Ftenant%3Fowned?trace=tenant",
            "/conversations/conv%2Ftenant%3Fowned/items/item%2Ftenant%3Fowned?trace=tenant",
            None,
        ),
    ];

    for (label, method, proxy_path, _upstream_path, body) in &cases {
        let (status, body_text) = if proxy_path.contains("%2E") || proxy_path.contains("%2e") {
            send_raw_proxy_resource_request(proxy_addr, method, proxy_path, label, body.as_ref())
                .await
        } else {
            let mut request = client
                .request(method.clone(), format!("{proxy_base}{proxy_path}"))
                .header("openai-api-key", "client-secret")
                .header("x-stainless-helper-method", *label)
                .header("OpenAI-Organization", "org-route-preserve")
                .header("OpenAI-Project", "proj-route-preserve")
                .header("Idempotency-Key", format!("idem-{label}"));
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request.send().await.expect("proxy response");
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            (status, body_text)
        };
        assert!(
            status.is_success(),
            "{label}: status={status} body={body_text}"
        );
    }

    let requests = recorded.lock().await;
    assert_eq!(requests.len(), cases.len(), "requests = {requests:?}");
    for (recorded, (label, method, _proxy_path, upstream_path, body)) in
        requests.iter().zip(cases.iter())
    {
        assert_eq!(recorded.method.as_str(), method.as_str(), "{label}");
        assert_eq!(recorded.uri.as_str(), *upstream_path, "{label}");
        assert_eq!(&recorded.body, body, "{label}");
        assert_eq!(
            recorded.authorization.as_deref(),
            Some("Bearer client-secret"),
            "{label}"
        );
        assert_eq!(recorded.helper_method.as_deref(), Some(*label), "{label}");
        assert_eq!(
            recorded.openai_organization.as_deref(),
            Some("org-route-preserve"),
            "{label}"
        );
        assert_eq!(
            recorded.openai_project.as_deref(),
            Some("proj-route-preserve"),
            "{label}"
        );
        let expected_idempotency_key = format!("idem-{label}");
        assert_eq!(
            recorded.idempotency_key.as_deref(),
            Some(expected_idempotency_key.as_str()),
            "{label}"
        );
        assert_eq!(recorded.data_token.as_deref(), None, "{label}");
        if body.is_some() {
            assert_eq!(
                recorded.content_type.as_deref(),
                Some("application/json"),
                "{label}"
            );
        } else {
            assert_eq!(recorded.content_type.as_deref(), None, "{label}");
        }
    }

    proxy_server.abort();
    upstream_server.abort();
}

#[tokio::test]
async fn openai_responses_resource_uses_request_snapshot_after_auth_runtime_race() {
    let client_key = "old-responses-client-key";
    let new_server_key = "new-responses-server-key";
    let (mock_base, recorded, upstream_server) = spawn_recording_responses_resource_mock().await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "primary",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiResponses,
            None,
            None,
        )],
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let old_runtime = state.runtime.read().await.clone();
    let auth_context = request_auth_context_for_runtime(
        old_runtime,
        data_auth::DataAccess::ClientProviderKey,
        data_auth::RequestAuthorization::ClientProviderKey {
            provider_key: client_key.to_string(),
        },
    );
    let replacement_config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![redaction_upstream_config(
            "primary",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiResponses,
            None,
            Some(crate::config::SecretSourceConfig {
                inline: Some(new_server_key.to_string()),
                env: None,
            }),
        )],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        data_auth: None,
    };
    replace_runtime_and_data_auth(
        &state,
        replacement_config,
        data_auth::DataAccess::ProxyKey {
            key: "new-responses-proxy-key".to_string(),
        },
    )
    .await;

    let response = handle_openai_responses_resource_with_auth_context(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        TestOpenAiResponsesResourceRequest {
            method: reqwest::Method::GET,
            resource_path: "responses/resp_snapshot".to_string(),
            body: None,
            query: None,
        },
        auth_context,
    )
    .await;

    assert!(
        response.status().is_success(),
        "Responses resource snapshot response status={}",
        response.status()
    );
    let requests = recorded.lock().await;
    assert_eq!(requests.len(), 1, "requests = {requests:?}");
    let expected_auth = format!("Bearer {client_key}");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some(expected_auth.as_str())
    );
    upstream_server.abort();
}

#[tokio::test]
async fn openai_responses_state_resource_routes_fail_closed_without_unique_available_native_upstream(
) {
    let cases = [
        (
            "no native upstream",
            runtime_namespace_state_for_tests(&[(
                "anthropic",
                crate::formats::UpstreamFormat::Anthropic,
                true,
            )]),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "multiple native upstreams",
            runtime_namespace_state_for_tests(&[
                (
                    "responses-a",
                    crate::formats::UpstreamFormat::OpenAiResponses,
                    true,
                ),
                (
                    "responses-b",
                    crate::formats::UpstreamFormat::OpenAiResponses,
                    true,
                ),
            ]),
            StatusCode::BAD_REQUEST,
        ),
        (
            "unavailable native upstream",
            runtime_namespace_state_for_tests(&[(
                "responses",
                crate::formats::UpstreamFormat::OpenAiResponses,
                false,
            )]),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ];

    for (label, namespace_state, expected_status) in cases {
        let state = Arc::new(AppState {
            runtime: Arc::new(RwLock::new(RuntimeState {
                namespaces: BTreeMap::from([(DEFAULT_NAMESPACE.to_string(), namespace_state)]),
            })),
            admin_update_lock: Arc::new(Mutex::new(())),
            metrics: crate::telemetry::RuntimeMetrics::new(&crate::config::Config::default()),
            admin_access: AdminAccess::LoopbackOnly,
            data_auth_policy: test_data_auth_policy_for_tests(),
        });

        let response = handle_openai_responses_resource(
            state,
            DEFAULT_NAMESPACE.to_string(),
            HeaderMap::new(),
            reqwest::Method::POST,
            "responses/input_tokens".to_string(),
            Some(serde_json::json!({ "input": "count me" })),
            None,
        )
        .await;

        assert_eq!(response.status(), expected_status, "{label}");
    }
}

#[test]
fn responses_stateful_request_controls_detect_provider_owned_fields() {
    let controls = responses_stateful_request_controls(&serde_json::json!({
        "previous_response_id": "resp_1",
        "conversation": { "id": "conv_1" },
        "background": true,
        "store": true,
        "prompt": { "id": "pmpt_1" }
    }));

    assert_eq!(
        controls,
        vec![
            "previous_response_id",
            "conversation",
            "background",
            "store",
            "prompt"
        ]
    );
}

#[test]
fn resolve_native_responses_stateful_route_or_error_prefers_unique_available_native_upstream() {
    let namespace_state = runtime_namespace_state_for_tests(&[
        (
            "responses",
            crate::formats::UpstreamFormat::OpenAiResponses,
            true,
        ),
        ("anthropic", crate::formats::UpstreamFormat::Anthropic, true),
    ]);

    let resolved = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "previous_response_id": "resp_1" }),
    )
    .expect("resolver should succeed")
    .expect("native route should be selected");

    assert_eq!(resolved.upstream_name, "responses");
    assert!(resolved.upstream_model.is_empty());
}

#[test]
fn resolve_native_responses_stateful_route_or_error_rejects_multiple_native_upstreams() {
    let namespace_state = runtime_namespace_state_for_tests(&[
        ("a", crate::formats::UpstreamFormat::OpenAiResponses, true),
        ("b", crate::formats::UpstreamFormat::OpenAiResponses, true),
    ]);

    let error = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "background": true }),
    )
    .expect_err("multiple native upstreams should fail");

    assert!(error.contains("multiple configured native OpenAI Responses upstreams"));
}

#[test]
fn resolve_native_responses_stateful_route_or_error_rejects_multiple_configured_native_upstreams_even_if_only_one_is_available(
) {
    let namespace_state = runtime_namespace_state_for_tests(&[
        ("a", crate::formats::UpstreamFormat::OpenAiResponses, true),
        ("b", crate::formats::UpstreamFormat::OpenAiResponses, false),
    ]);

    let error = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "previous_response_id": "resp_1" }),
    )
    .expect_err("configured ownership should stay ambiguous");

    assert!(error.contains("multiple"));
    assert!(error.contains("configured"));
    assert!(error.contains("previous_response_id"));
}

#[test]
fn resolve_native_responses_stateful_route_or_error_rejects_multi_upstream_auto_discovery_without_explicit_owner_pin(
) {
    let pinned = test_upstream_config("responses", crate::formats::UpstreamFormat::OpenAiResponses);
    let auto = test_upstream_config_with_fixed_format("auto", None);
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![pinned.clone(), auto.clone()],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        data_auth: None,
    };
    let (responses_client, responses_streaming_client, responses_resolved_proxy) =
        crate::upstream::build_upstream_clients(
            &config,
            pinned.proxy.as_ref(),
            config.proxy.as_ref(),
        )
        .expect("build responses upstream clients");
    let responses_no_auto_decompression_client =
        crate::upstream::build_no_auto_decompression_client(
            config.upstream_timeout,
            &responses_resolved_proxy,
        )
        .expect("build responses no-auto-decompression upstream client");
    let (auto_client, auto_streaming_client, auto_resolved_proxy) =
        crate::upstream::build_upstream_clients(
            &config,
            auto.proxy.as_ref(),
            config.proxy.as_ref(),
        )
        .expect("build auto upstream clients");
    let auto_no_auto_decompression_client = crate::upstream::build_no_auto_decompression_client(
        config.upstream_timeout,
        &auto_resolved_proxy,
    )
    .expect("build auto no-auto-decompression upstream client");
    let upstreams = std::collections::BTreeMap::from([
        (
            "responses".to_string(),
            UpstreamState {
                config: pinned,
                provider_key: None,
                capability: Some(crate::discovery::UpstreamCapability::fixed(
                    crate::formats::UpstreamFormat::OpenAiResponses,
                )),
                availability: crate::discovery::UpstreamAvailability::available(),
                client: responses_client,
                streaming_client: responses_streaming_client,
                no_auto_decompression_client: responses_no_auto_decompression_client,
                resolved_proxy: responses_resolved_proxy,
            },
        ),
        (
            "auto".to_string(),
            UpstreamState {
                config: auto,
                provider_key: None,
                capability: Some(crate::discovery::UpstreamCapability::fixed(
                    crate::formats::UpstreamFormat::Anthropic,
                )),
                availability: crate::discovery::UpstreamAvailability::available(),
                client: auto_client,
                streaming_client: auto_streaming_client,
                no_auto_decompression_client: auto_no_auto_decompression_client,
                resolved_proxy: auto_resolved_proxy,
            },
        ),
    ]);
    let namespace_state = RuntimeNamespaceState {
        revision: "test-revision".to_string(),
        config,
        upstreams,
        hooks: None,
        debug_trace: None,
    };

    let error = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "background": true }),
    )
    .expect_err("auto-discovery should block provenance-free routing");

    assert!(error.contains("auto-discovery"));
    assert!(error.contains("fixed_upstream_format"));
}

#[tokio::test]
async fn handle_openai_responses_resource_uses_upstream_state_client() {
    #[derive(Clone)]
    struct ResourceState {
        bodies: Arc<Mutex<Vec<Value>>>,
    }

    async fn handle_compact(
        State(state): State<ResourceState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.bodies.lock().await.push(body);
        Json(serde_json::json!({
            "id": "resp_compact_1",
            "object": "response",
            "status": "completed"
        }))
    }

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/responses/compact", post(handle_compact))
        .with_state(ResourceState {
            bodies: bodies.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind responses resource mock");
    let addr = listener.local_addr().expect("responses resource mock addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("responses resource mock server");
    });

    let upstream = crate::config::UpstreamConfig {
        name: "responses".to_string(),
        api_root: format!("http://{addr}"),
        fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
        provider_key_env: None,
        provider_key: None,
        upstream_headers: Vec::new(),
        proxy: None,
        limits: None,
        surface_defaults: None,
    };
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(5),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![upstream.clone()],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        data_auth: None,
    };
    let (client, streaming_client, resolved_proxy) = crate::upstream::build_upstream_clients(
        &config,
        upstream.proxy.as_ref(),
        config.proxy.as_ref(),
    )
    .expect("build responses runtime clients");
    let no_auto_decompression_client = crate::upstream::build_no_auto_decompression_client(
        config.upstream_timeout,
        &resolved_proxy,
    )
    .expect("build responses no-auto-decompression runtime client");
    let namespace_state = RuntimeNamespaceState {
        revision: "test-revision".to_string(),
        config: config.clone(),
        upstreams: std::collections::BTreeMap::from([(
            upstream.name.clone(),
            UpstreamState {
                config: upstream,
                provider_key: None,
                capability: Some(crate::discovery::UpstreamCapability::fixed(
                    crate::formats::UpstreamFormat::OpenAiResponses,
                )),
                availability: crate::discovery::UpstreamAvailability::available(),
                client,
                streaming_client,
                no_auto_decompression_client,
                resolved_proxy,
            },
        )]),
        hooks: None,
        debug_trace: None,
    };
    let state = Arc::new(AppState {
        runtime: Arc::new(RwLock::new(RuntimeState {
            namespaces: std::collections::BTreeMap::from([(
                DEFAULT_NAMESPACE.to_string(),
                namespace_state,
            )]),
        })),
        admin_update_lock: Arc::new(Mutex::new(())),
        metrics: crate::telemetry::RuntimeMetrics::new(&crate::config::Config::default()),
        admin_access: AdminAccess::LoopbackOnly,
        data_auth_policy: test_data_auth_policy_for_tests(),
    });

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::POST,
        "responses/compact".to_string(),
        Some(serde_json::json!({ "reasoning": { "effort": "medium" } })),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let recorded = bodies.lock().await;
    assert_eq!(recorded.len(), 1, "bodies = {recorded:?}");
    assert_eq!(recorded[0]["reasoning"]["effort"], "medium");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_non_stream_error_redacts_runtime_secrets() {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::BAD_REQUEST,
        responses_resource_error_body_with_secrets(),
    )
    .await;
    let state = responses_resource_redaction_state(&mock_base).await;

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_redaction_error".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_text = response_body_text(response).await;
    assert_resource_response_redacted(&body_text, "Responses resource non-stream error");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_public_metadata_errors_redact_active_credentials() {
    let provider_secret = PROVIDER_INLINE_REDACTION_SECRET;
    let proxy_secret = PROXY_INLINE_REDACTION_SECRET;
    let secrets = [provider_secret, proxy_secret];
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::OK,
        responses_resource_success_body_with_text("ok".to_string()),
    )
    .await;
    let state = responses_resource_redaction_state_with(
        &mock_base,
        Some(provider_secret),
        data_auth::DataAccess::ProxyKey {
            key: proxy_secret.to_string(),
        },
    )
    .await;

    let response = handle_openai_responses_resource(
        state.clone(),
        format!("tenant-{proxy_secret}"),
        HeaderMap::new(),
        reqwest::Method::GET,
        format!("responses/resp-{provider_secret}-{proxy_secret}"),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body_text = response_body_text(response).await;
    assert_resource_response_redacted(&body_text, "Responses resource namespace error");
    assert_resource_metrics_redacted(&state, &secrets, "Responses resource namespace error").await;
    server.abort();

    let upstream_name = format!("responses-{provider_secret}-{proxy_secret}");
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::OK,
        responses_resource_success_body_with_text("ok".to_string()),
    )
    .await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            &upstream_name,
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiResponses,
            None,
            Some(crate::config::SecretSourceConfig {
                inline: Some(provider_secret.to_string()),
                env: None,
            }),
        )],
        data_auth::DataAccess::ProxyKey {
            key: proxy_secret.to_string(),
        },
    )
    .await;
    state
        .runtime
        .write()
        .await
        .namespaces
        .get_mut(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .upstreams
        .get_mut(&upstream_name)
        .expect("redaction upstream")
        .availability = crate::discovery::UpstreamAvailability::unavailable(format!(
        "outage-{provider_secret}-{proxy_secret}"
    ));

    let response = handle_openai_responses_resource(
        state.clone(),
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        format!("responses/resp-upstream-{provider_secret}-{proxy_secret}"),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_text = response_body_text(response).await;
    assert_resource_response_redacted(&body_text, "Responses resource upstream error");
    assert_resource_metrics_redacted(&state, &secrets, "Responses resource upstream error").await;
    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_stream_error_redacts_runtime_secrets() {
    let (mock_base, _seen_requests, server) = spawn_recorded_responses_resource_stream_mock(
        StatusCode::UNAUTHORIZED,
        "application/json",
        responses_resource_error_body_with_secrets(),
    )
    .await;
    let state = responses_resource_redaction_state(&mock_base).await;

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_redaction_stream".to_string(),
        None,
        Some("stream=true".to_string()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_body_text(response).await;
    assert!(body_text.contains("response.failed"), "{body_text}");
    assert_resource_response_redacted(&body_text, "Responses resource stream error");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_streaming_error_metrics_redact_resource_path_credentials()
{
    let provider_secret = PROVIDER_INLINE_REDACTION_SECRET;
    let proxy_secret = PROXY_INLINE_REDACTION_SECRET;
    let secrets = [provider_secret, proxy_secret];
    let (mock_base, _seen_requests, server) = spawn_recorded_responses_resource_stream_mock(
        StatusCode::UNAUTHORIZED,
        "application/json",
        responses_resource_error_body_with_secrets(),
    )
    .await;
    let state = responses_resource_redaction_state(&mock_base).await;

    let response = handle_openai_responses_resource(
        state.clone(),
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        format!("responses/resp-stream-{provider_secret}-{proxy_secret}"),
        None,
        Some("stream=true".to_string()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_body_text(response).await;
    assert!(body_text.contains("response.failed"), "{body_text}");
    assert_resource_response_redacted(&body_text, "Responses resource streaming error");
    assert_resource_metrics_redacted(&state, &secrets, "Responses resource streaming error").await;

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_non_stream_success_redacts_known_credentials() {
    let provider_secret = "rrs";
    let proxy_secret = "rrp";
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::OK,
        responses_resource_success_body_with_text(format!(
            "resource echoed provider {provider_secret} and proxy {proxy_secret}"
        )),
    )
    .await;
    let state = responses_resource_redaction_state_with(
        &mock_base,
        Some(provider_secret),
        data_auth::DataAccess::ProxyKey {
            key: proxy_secret.to_string(),
        },
    )
    .await;

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_success_redaction".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_body_text(response).await;
    let server_secrets = [provider_secret, proxy_secret];
    for secret in server_secrets {
        assert!(
            !body_text.contains(secret),
            "Responses resource success JSON leaked {secret}: {body_text}"
        );
    }
    assert!(
        body_text.contains("[REDACTED]"),
        "Responses resource success JSON should show redaction placeholder: {body_text}"
    );
    server.abort();

    let client_secret = "rrc";
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::OK,
        responses_resource_success_body_with_text(format!(
            "resource echoed client credential {client_secret}"
        )),
    )
    .await;
    let state = responses_resource_redaction_state_with(
        &mock_base,
        None,
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {client_secret}")).expect("client credential"),
    );

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        headers,
        reqwest::Method::GET,
        "responses/resp_client_success_redaction".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_body_text(response).await;
    assert!(
        !body_text.contains(client_secret),
        "Responses resource client success JSON leaked {client_secret}: {body_text}"
    );
    assert!(
        body_text.contains("[REDACTED]"),
        "Responses resource client success JSON should show redaction placeholder: {body_text}"
    );
    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_stream_success_redacts_known_credentials() {
    let provider_secret = "rss";
    let proxy_secret = "rsp";
    let stream_event = serde_json::json!({
        "type": "response.output_text.delta",
        "sequence_number": 0,
        "response_id": "resp_stream_success_redaction",
        "output_index": 0,
        "content_index": 0,
        "delta": format!("resource stream echoed {provider_secret} and {proxy_secret}")
    });
    let (mock_base, _seen_requests, server) = spawn_recorded_responses_resource_stream_mock(
        StatusCode::OK,
        "text/event-stream",
        format_sse_event("response.output_text.delta", &stream_event),
    )
    .await;
    let state = responses_resource_redaction_state_with(
        &mock_base,
        Some(provider_secret),
        data_auth::DataAccess::ProxyKey {
            key: proxy_secret.to_string(),
        },
    )
    .await;

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_stream_success_redaction".to_string(),
        None,
        Some("stream=true".to_string()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_body_text(response).await;
    let server_secrets = [provider_secret, proxy_secret];
    for secret in server_secrets {
        assert!(
            !body_text.contains(secret),
            "Responses resource success SSE leaked {secret}: {body_text}"
        );
    }
    assert!(
        body_text.contains("[REDACTED]"),
        "Responses resource success SSE should show redaction placeholder: {body_text}"
    );
    server.abort();

    let client_secret = "rsc";
    let stream_event = serde_json::json!({
        "type": "response.output_text.delta",
        "sequence_number": 0,
        "response_id": "resp_client_stream_success_redaction",
        "output_index": 0,
        "content_index": 0,
        "delta": format!("resource stream echoed client credential {client_secret}")
    });
    let (mock_base, _seen_requests, server) = spawn_recorded_responses_resource_stream_mock(
        StatusCode::OK,
        "text/event-stream",
        format_sse_event("response.output_text.delta", &stream_event),
    )
    .await;
    let state = responses_resource_redaction_state_with(
        &mock_base,
        None,
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {client_secret}")).expect("client credential"),
    );

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        headers,
        reqwest::Method::GET,
        "responses/resp_client_stream_success_redaction".to_string(),
        None,
        Some("stream=true".to_string()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_body_text(response).await;
    assert!(
        !body_text.contains(client_secret),
        "Responses resource client success SSE leaked {client_secret}: {body_text}"
    );
    assert!(
        body_text.contains("[REDACTED]"),
        "Responses resource client success SSE should show redaction placeholder: {body_text}"
    );
    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_invalid_json_success_fails_closed() {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::OK,
        "not json __llmup_custom__secret _llmup_tool_bridge_context",
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_bad_json".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    assert!(!body_text.contains("__llmup_custom__"), "{body_text}");
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "{body_text}"
    );
    assert!(!body_text.contains("secret"), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_non_json_error_is_normalized_and_sanitized() {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::BAD_REQUEST,
        "upstream failed with __llmup_custom__secret and _llmup_tool_bridge_context",
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_error".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    let body: Value = serde_json::from_str(&body_text).expect("normalized json error");
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(!body_text.contains("__llmup_custom__"), "{body_text}");
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "{body_text}"
    );
    assert!(!body_text.contains("secret"), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_json_error_without_message_is_sanitized() {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::BAD_GATEWAY,
        r#"{"error":{"type":"server_error","debug":"__llmup_custom__secret"},"_llmup_tool_bridge_context":{"entries":{}}}"#,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_error".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    let body: Value = serde_json::from_str(&body_text).expect("normalized json error");
    assert_eq!(
        body["error"]["message"],
        crate::internal_artifacts::GENERIC_UPSTREAM_ERROR_MESSAGE
    );
    assert!(!body_text.contains("__llmup_custom__"), "{body_text}");
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "{body_text}"
    );
    assert!(!body_text.contains("secret"), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_success_canonicalizes_validated_json() {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::OK,
        r#"{"id":"resp_duplicate","object":"response","status":"completed","output":[],"metadata":{"note":"__llmup_custom__secret"},"metadata":{}}"#,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_duplicate".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    let body: Value = serde_json::from_str(&body_text).expect("canonical json body");
    assert_eq!(body["id"], "resp_duplicate");
    assert_eq!(body["metadata"], serde_json::json!({}));
    assert!(!body_text.contains("__llmup_custom__"), "{body_text}");
    assert!(!body_text.contains("secret"), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_success_preserves_regular_text_metadata_and_schema_descriptions(
) {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::OK,
        r#"{"id":"resp_plain_reserved_text","object":"response","status":"completed","output":[{"type":"message","id":"msg_plain_reserved_text","role":"assistant","content":[{"type":"output_text","text":"resource output mentions __llmup_custom__apply_patch as plain text","annotations":[]}]}],"metadata":{"note":"user metadata mentions __llmup_custom__apply_patch"},"tools":[{"type":"function","name":"describe_patch_token","description":"schema docs mention __llmup_custom__apply_patch literally","parameters":{"type":"object","properties":{"literal":{"type":"string","description":"schema docs mention __llmup_custom__apply_patch literally"}}}}]}"#,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_plain_reserved_text".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    let body: Value = serde_json::from_str(&body_text).expect("canonical json body");
    assert_eq!(
        body["output"][0]["content"][0]["text"],
        "resource output mentions __llmup_custom__apply_patch as plain text"
    );
    assert_eq!(
        body["metadata"]["note"],
        "user metadata mentions __llmup_custom__apply_patch"
    );
    assert_eq!(
        body["tools"][0]["parameters"]["properties"]["literal"]["description"],
        "schema docs mention __llmup_custom__apply_patch literally"
    );
    assert!(body_text.contains("__llmup_custom__apply_patch"));

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_stream_true_forwards_guarded_sse() {
    let upstream_body = r#"event: response.created
data: {"type":"response.created","sequence_number":0,"response":{"id":"resp_stream","object":"response","created_at":0,"status":"in_progress","output":[],"metadata":{}}}

event: response.completed
data: {"type":"response.completed","sequence_number":1,"response":{"id":"resp_stream","object":"response","created_at":0,"status":"completed","output":[],"metadata":{}}}

"#;
    let (mock_base, seen_requests, server) = spawn_recorded_responses_resource_stream_mock(
        StatusCode::OK,
        "text/event-stream",
        upstream_body,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_stream".to_string(),
        None,
        Some("stream=true&starting_after=7&include_obfuscation=false".to_string()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body_text = response_body_text(response).await;
    assert!(body_text.contains("event: response.created"), "{body_text}");
    assert!(
        body_text.contains("event: response.completed"),
        "{body_text}"
    );
    assert!(
        body_text.contains("\"status\":\"completed\""),
        "{body_text}"
    );

    let recorded = seen_requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert_eq!(
        recorded[0].0,
        "/responses/resp_stream?stream=true&starting_after=7&include_obfuscation=false"
    );
    assert_eq!(recorded[0].1.as_deref(), Some("text/event-stream"));

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_stream_true_rejects_public_boundary_artifacts() {
    let upstream_body = r#"event: response.output_item.added
data: {"type":"response.output_item.added","sequence_number":0,"output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"__llmup_custom__apply_patch","arguments":"{}"}}

"#;
    let (mock_base, _seen_requests, server) = spawn_recorded_responses_resource_stream_mock(
        StatusCode::OK,
        "text/event-stream",
        upstream_body,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_stream".to_string(),
        None,
        Some("stream=true".to_string()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_body_text(response).await;
    assert!(body_text.contains("response.failed"), "{body_text}");
    assert!(
        body_text.contains("\"code\":\"reserved_openai_custom_bridge_prefix\""),
        "{body_text}"
    );
    assert!(!body_text.contains("__llmup_custom__"), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_stream_true_fails_closed_on_non_sse_success() {
    let (mock_base, _seen_requests, server) = spawn_recorded_responses_resource_stream_mock(
        StatusCode::OK,
        "application/json",
        r#"{"id":"resp_json","object":"response","status":"completed"}"#,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_stream".to_string(),
        None,
        Some("stream=true".to_string()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body_text = response_body_text(response).await;
    assert!(body_text.contains("response.failed"), "{body_text}");
    assert!(
        body_text.contains("upstream returned non-SSE response for streamed Responses resource"),
        "{body_text}"
    );
    assert!(!body_text.contains("resp_json"), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_200_empty_body_fails_closed_for_lifecycle_methods() {
    for (method, resource_path, request_body) in [
        (reqwest::Method::GET, "responses/resp_empty_get", None),
        (reqwest::Method::DELETE, "responses/resp_empty_delete", None),
        (
            reqwest::Method::POST,
            "responses/resp_empty_cancel/cancel",
            None,
        ),
        (
            reqwest::Method::POST,
            "responses/compact",
            Some(serde_json::json!({
                "input": "compact this response"
            })),
        ),
    ] {
        assert_empty_success_body_fails_closed(method, resource_path, request_body).await;
    }
}

#[tokio::test]
async fn handle_openai_responses_resource_201_empty_body_fails_closed() {
    let (response, server) = call_raw_responses_resource(
        StatusCode::CREATED,
        "",
        reqwest::Method::GET,
        "responses/resp_empty_created",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body_text = response_body_text(response).await;
    let body: Value = serde_json::from_str(&body_text).expect("OpenAI error envelope");
    assert_eq!(
        body["error"]["message"], "upstream returned empty response body",
        "{body_text}"
    );

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_204_content_length_payload_fails_closed() {
    let body_text = assert_raw_no_content_framing_fails_closed(raw_no_content_response(
        StatusCode::NO_CONTENT,
        &[("Content-Length", "5")],
        b"hello",
    ))
    .await;

    assert!(!body_text.contains("hello"), "{body_text}");
}

#[tokio::test]
async fn handle_openai_responses_resource_204_transfer_encoding_chunked_fails_closed() {
    assert_raw_no_content_framing_fails_closed(raw_no_content_response(
        StatusCode::NO_CONTENT,
        &[("Transfer-Encoding", "chunked")],
        b"5\r\nhello\r\n0\r\n\r\n",
    ))
    .await;
}

#[tokio::test]
async fn handle_openai_responses_resource_204_invalid_or_conflicting_content_length_fails_closed() {
    assert_raw_no_content_framing_fails_closed(raw_no_content_response(
        StatusCode::NO_CONTENT,
        &[("Content-Length", "0, 5")],
        b"",
    ))
    .await;
}

#[tokio::test]
async fn handle_openai_responses_resource_204_signed_zero_content_length_fails_closed() {
    for content_length in ["+0", "-0", "+000"] {
        assert_raw_no_content_framing_fails_closed(raw_no_content_response(
            StatusCode::NO_CONTENT,
            &[("Content-Length", content_length)],
            b"",
        ))
        .await;
    }
}

#[tokio::test]
async fn handle_openai_responses_resource_204_zero_content_length_stays_empty() {
    let (response, server) = call_raw_tcp_responses_resource(raw_no_content_response(
        StatusCode::NO_CONTENT,
        &[("Content-Length", "0")],
        b"",
    ))
    .await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("empty body bytes");
    assert!(body.is_empty(), "body = {body:?}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_205_transfer_encoding_chunked_zero_chunk_fails_closed() {
    assert_raw_no_content_framing_fails_closed(raw_no_content_response(
        StatusCode::RESET_CONTENT,
        &[("Transfer-Encoding", "chunked")],
        b"0\r\n\r\n",
    ))
    .await;
}

#[tokio::test]
async fn handle_openai_responses_resource_204_205_gzip_empty_non_zero_content_length_fails_closed()
{
    let gzip_empty_body =
        b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00";
    for status in [StatusCode::NO_CONTENT, StatusCode::RESET_CONTENT] {
        assert_raw_no_content_framing_fails_closed(raw_no_content_response(
            status,
            &[
                ("Content-Encoding", "gzip"),
                ("Content-Length", &gzip_empty_body.len().to_string()),
            ],
            gzip_empty_body,
        ))
        .await;
    }
}

#[tokio::test]
async fn handle_openai_responses_resource_empty_success_body_stays_empty() {
    let (response, server) = call_raw_responses_resource(
        StatusCode::NO_CONTENT,
        "",
        reqwest::Method::DELETE,
        "responses/resp_empty",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .is_none(),
        "204 empty response should not force JSON content-type"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("empty body bytes");
    assert!(body.is_empty(), "body = {body:?}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_205_empty_success_body_stays_empty() {
    let (response, server) = call_raw_responses_resource(
        StatusCode::RESET_CONTENT,
        "",
        reqwest::Method::DELETE,
        "responses/resp_empty_reset",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::RESET_CONTENT);
    assert!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .is_none(),
        "205 empty response should not force JSON content-type"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("empty body bytes");
    assert!(body.is_empty(), "body = {body:?}");

    server.abort();
}
