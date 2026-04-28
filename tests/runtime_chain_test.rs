//! Runtime chain tests focused on incremental streaming, cancellation propagation,
//! live namespace isolation, and fatal translated stream rejection behavior.

mod common;

use axum::{
    body::Body,
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use common::proxy_helpers::proxy_config;
use common::runtime_proxy::{start_proxy, upstream_api_root};
use futures_util::{future::join_all, Stream, StreamExt};
use llm_universal_proxy::config::{
    Config, DebugTraceConfig, HookConfig, HookEndpointConfig, ProxyConfig, RuntimeConfigPayload,
    RuntimeHookConfig, RuntimeUpstreamConfig,
};
use llm_universal_proxy::formats::UpstreamFormat;
use reqwest::{
    header::{HeaderMap as ReqwestHeaderMap, HeaderValue},
    Client as ReqwestClient, IntoUrl, RequestBuilder as ReqwestRequestBuilder,
};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::fs::OpenOptions;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

static ADMIN_TOKEN_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
const TEST_PROVIDER_KEY: &str = "provider-secret";

#[derive(Clone)]
struct Client {
    inner: ReqwestClient,
}

impl Client {
    fn new() -> Self {
        let mut headers = ReqwestHeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {TEST_PROVIDER_KEY}")).unwrap(),
        );
        Self {
            inner: ReqwestClient::builder()
                .default_headers(headers)
                .build()
                .unwrap(),
        }
    }

    fn post<U: IntoUrl>(&self, url: U) -> ReqwestRequestBuilder {
        self.inner.post(url)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapturedLiveRequest {
    session: String,
    namespace_header: String,
}

#[derive(Clone, Default)]
struct CapturedLiveRequests {
    requests: Arc<Mutex<Vec<CapturedLiveRequest>>>,
}

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<String>,
}

impl ScopedEnvVar {
    fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::remove_var(key);
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

#[derive(Clone, Default)]
struct CapturedHookPayloads {
    payloads: Arc<Mutex<Vec<Value>>>,
}

impl CapturedHookPayloads {
    fn push(&self, payload: Value) {
        self.payloads.lock().unwrap().push(payload);
    }

    fn snapshot(&self) -> Vec<Value> {
        self.payloads.lock().unwrap().clone()
    }
}

struct ScheduledChunk {
    delay: Duration,
    bytes: Bytes,
}

struct ControlledSseStream {
    chunks: VecDeque<ScheduledChunk>,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    pending: Option<Bytes>,
    drop_notify: Option<Arc<Notify>>,
}

#[cfg(unix)]
struct SlowTraceSinkGuard {
    path: PathBuf,
    release: Option<std::sync::mpsc::Sender<()>>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[cfg(unix)]
struct BlockingTraceCapture {
    path: PathBuf,
    gate: Option<std::fs::File>,
    release: Option<std::sync::mpsc::Sender<()>>,
    ready: Option<std::sync::mpsc::Receiver<()>>,
    join: Option<std::thread::JoinHandle<Vec<u8>>>,
}

#[cfg(unix)]
impl SlowTraceSinkGuard {
    fn new(label: &str) -> Self {
        let path = unique_temp_path(label, "fifo");
        let status = Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo should be available for slow trace sink tests");
        assert!(status.success(), "mkfifo should succeed for {path:?}");

        let (tx, rx) = std::sync::mpsc::channel();
        let thread_path = path.clone();
        let join = std::thread::spawn(move || {
            let handle = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&thread_path)
                .expect("slow trace sink reader should open fifo");
            let _ = rx.recv();
            drop(handle);
        });

        std::thread::sleep(Duration::from_millis(25));

        Self {
            path,
            release: Some(tx),
            join: Some(join),
        }
    }

    fn path_string(&self) -> String {
        self.path.to_string_lossy().to_string()
    }
}

#[cfg(unix)]
impl Drop for SlowTraceSinkGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.release.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
impl BlockingTraceCapture {
    fn new(label: &str) -> Self {
        let path = unique_temp_path(label, "fifo");
        let status = Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo should be available for blocking trace capture tests");
        assert!(status.success(), "mkfifo should succeed for {path:?}");

        let gate = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("blocking trace capture gate should open fifo");
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread_path = path.clone();
        let join = std::thread::spawn(move || {
            let _ = release_rx.recv();
            let mut reader = OpenOptions::new()
                .read(true)
                .open(&thread_path)
                .expect("blocking trace capture reader should open fifo");
            let _ = ready_tx.send(());
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut reader, &mut bytes)
                .expect("blocking trace capture reader should drain fifo");
            bytes
        });

        Self {
            path,
            gate: Some(gate),
            release: Some(release_tx),
            ready: Some(ready_rx),
            join: Some(join),
        }
    }

    fn path_string(&self) -> String {
        self.path.to_string_lossy().to_string()
    }

    fn begin_drain(&mut self) {
        if let Some(tx) = self.release.take() {
            tx.send(())
                .expect("blocking trace capture release should reach reader thread");
        }
        if let Some(rx) = self.ready.take() {
            rx.recv_timeout(Duration::from_secs(1))
                .expect("blocking trace capture reader should become ready");
        }
        drop(self.gate.take());
    }

    fn collect(mut self) -> Vec<Value> {
        self.begin_drain();
        let bytes = self
            .join
            .take()
            .expect("blocking trace capture join handle missing")
            .join()
            .expect("blocking trace capture thread should join");
        let _ = std::fs::remove_file(&self.path);
        String::from_utf8(bytes)
            .expect("blocking trace capture output should be valid utf-8")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<Value>(line)
                    .expect("blocking trace capture line should be valid json")
            })
            .collect()
    }
}

#[cfg(unix)]
impl Drop for BlockingTraceCapture {
    fn drop(&mut self) {
        if let Some(tx) = self.release.take() {
            let _ = tx.send(());
        }
        drop(self.gate.take());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl ControlledSseStream {
    fn new(chunks: Vec<(Duration, impl Into<Bytes>)>) -> Self {
        Self {
            chunks: chunks
                .into_iter()
                .map(|(delay, bytes)| ScheduledChunk {
                    delay,
                    bytes: bytes.into(),
                })
                .collect(),
            sleep: None,
            pending: None,
            drop_notify: None,
        }
    }

    fn with_drop_notify(mut self, drop_notify: Arc<Notify>) -> Self {
        self.drop_notify = Some(drop_notify);
        self
    }
}

impl Stream for ControlledSseStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(sleep) = self.sleep.as_mut() {
                if sleep.as_mut().poll(cx).is_pending() {
                    return Poll::Pending;
                }
                self.sleep = None;
                if let Some(bytes) = self.pending.take() {
                    return Poll::Ready(Some(Ok(bytes)));
                }
            }

            let Some(chunk) = self.chunks.pop_front() else {
                return Poll::Ready(None);
            };

            if chunk.delay.is_zero() {
                return Poll::Ready(Some(Ok(chunk.bytes)));
            }

            self.pending = Some(chunk.bytes);
            self.sleep = Some(Box::pin(tokio::time::sleep(chunk.delay)));
        }
    }
}

impl Drop for ControlledSseStream {
    fn drop(&mut self) {
        if let Some(notify) = &self.drop_notify {
            notify.notify_waiters();
        }
    }
}

fn unique_temp_path(label: &str, suffix: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "llm-universal-proxy-{label}-{}-{stamp}.{suffix}",
        std::process::id()
    ))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn read_complete_http_request(
    stream: &mut tokio::net::TcpStream,
) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut expected_len = None;

    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(buffer);
        }
        buffer.extend_from_slice(&chunk[..read]);

        if expected_len.is_none() {
            if let Some(headers_end) = find_bytes(&buffer, b"\r\n\r\n") {
                let body_start = headers_end + 4;
                let header_bytes = &buffer[..body_start];
                let header_text = String::from_utf8_lossy(header_bytes);
                let content_length = header_text
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                expected_len = Some(body_start + content_length);
            }
        }

        if let Some(expected_len) = expected_len {
            if buffer.len() >= expected_len {
                return Ok(buffer);
            }
        }
    }
}

async fn open_raw_http_request(
    base: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> TcpStream {
    let url = reqwest::Url::parse(base).expect("proxy base URL should parse");
    let host = url
        .host_str()
        .expect("proxy base URL should include a host");
    let port = url
        .port_or_known_default()
        .expect("proxy base URL should include a port");
    let address = format!("{host}:{port}");
    let mut stream = TcpStream::connect(address)
        .await
        .expect("raw downstream client should connect");

    let request_body = body.map(|value| serde_json::to_vec(value).unwrap());
    let content_length = request_body.as_ref().map_or(0, Vec::len);
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: keep-alive\r\nAuthorization: Bearer {TEST_PROVIDER_KEY}\r\n"
    );
    if request_body.is_some() {
        request.push_str("Content-Type: application/json\r\n");
    }
    request.push_str(&format!("Content-Length: {content_length}\r\n\r\n"));
    stream
        .write_all(request.as_bytes())
        .await
        .expect("raw downstream client should write request headers");
    if let Some(request_body) = request_body {
        stream
            .write_all(&request_body)
            .await
            .expect("raw downstream client should write request body");
    }
    stream
}

async fn wait_for_payloads(captured: &CapturedHookPayloads, count: usize) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = captured.snapshot();
            if snapshot.len() >= count {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected hook payloads to arrive")
}

async fn wait_for_debug_trace_response(path: &Path) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(contents) = tokio::fs::read_to_string(path).await {
                let mut response = None;
                for line in contents.lines() {
                    if let Ok(value) = serde_json::from_str::<Value>(line) {
                        if value.get("phase").and_then(Value::as_str) == Some("response") {
                            response = Some(value);
                        }
                    }
                }
                if let Some(value) = response {
                    return value;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected debug trace response line")
}

async fn spawn_hook_capture_mock() -> (
    String,
    tokio::task::JoinHandle<()>,
    CapturedHookPayloads,
    CapturedHookPayloads,
) {
    async fn exchange_handler(
        State((captured, _)): State<(CapturedHookPayloads, CapturedHookPayloads)>,
        Json(body): Json<Value>,
    ) -> Response {
        captured.push(body);
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    }

    async fn usage_handler(
        State((_, captured)): State<(CapturedHookPayloads, CapturedHookPayloads)>,
        Json(body): Json<Value>,
    ) -> Response {
        captured.push(body);
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let exchange = CapturedHookPayloads::default();
    let usage = CapturedHookPayloads::default();
    let app = Router::new()
        .route("/exchange", post(exchange_handler))
        .route("/usage", post(usage_handler))
        .with_state((exchange.clone(), usage.clone()));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, exchange, usage)
}

async fn spawn_incremental_anthropic_stream_mock() -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(Json(body): Json<Value>) -> Response {
        let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        if !stream {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "msg_incremental",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-3",
                    "content": [{ "type": "text", "text": "Hi" }],
                    "stop_reason": "end_turn",
                    "stop_sequence": null,
                    "usage": { "input_tokens": 1, "output_tokens": 1 }
                })),
            )
                .into_response();
        }

        let stream = ControlledSseStream::new(vec![
            (
                Duration::ZERO,
                concat!(
                    "event: message_start\n",
                    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_incremental\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-3\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\n"
                ),
            ),
            (
                Duration::from_millis(350),
                concat!(
                    "event: content_block_start\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                    "event: content_block_delta\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
                    "event: content_block_stop\n",
                    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                    "event: message_delta\n",
                    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}\n\n",
                    "event: message_stop\n",
                    "data: {\"type\":\"message_stop\"}\n\n"
                ),
            ),
        ]);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/messages", post(handler))
        .route("/messages", post(handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn spawn_disconnect_observing_openai_mock(
) -> (String, tokio::task::JoinHandle<()>, Arc<Notify>) {
    let drop_notify = Arc::new(Notify::new());

    async fn handler(State(drop_notify): State<Arc<Notify>>, Json(body): Json<Value>) -> Response {
        let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        if !stream {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "chatcmpl_disconnect",
                    "object": "chat.completion",
                    "created": 1,
                    "model": "gpt-4",
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

        let stream = ControlledSseStream::new(vec![
            (
                Duration::ZERO,
                "data: {\"id\":\"chatcmpl_disconnect\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"gpt-4\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            ),
            (
                Duration::from_secs(30),
                "data: [DONE]\n\n",
            ),
        ])
        .with_drop_notify(drop_notify);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/chat/completions", post(handler))
        .route("/chat/completions", post(handler))
        .with_state(drop_notify.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, drop_notify)
}

async fn spawn_pending_openai_send_mock() -> (
    String,
    tokio::task::JoinHandle<()>,
    Arc<Notify>,
    Arc<Notify>,
) {
    let request_started = Arc::new(Notify::new());
    let abort_notify = Arc::new(Notify::new());
    let request_started_task = request_started.clone();
    let abort_notify_task = abort_notify.clone();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_complete_http_request(&mut stream)
            .await
            .expect("pending upstream mock should receive a full request");
        request_started_task.notify_waiters();

        let mut buf = [0u8; 1];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    abort_notify_task.notify_waiters();
                    break;
                }
                Ok(_) => {}
            }
        }
    });

    (base, handle, request_started, abort_notify)
}

async fn spawn_pending_openai_response_resource_body_mock() -> (
    String,
    tokio::task::JoinHandle<()>,
    Arc<Notify>,
    Arc<Notify>,
) {
    let response_started = Arc::new(Notify::new());
    let drop_notify = Arc::new(Notify::new());

    async fn handler(
        State((response_started, drop_notify)): State<(Arc<Notify>, Arc<Notify>)>,
    ) -> Response {
        response_started.notify_waiters();

        let stream = ControlledSseStream::new(vec![
            (
                Duration::ZERO,
                Bytes::from_static(
                    br#"{"id":"resp_pending","object":"response","status":"completed","output":["#,
                ),
            ),
            (Duration::from_secs(30), Bytes::from_static(b"]}")),
        ])
        .with_drop_notify(drop_notify);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(Body::from_stream(stream))
            .unwrap()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/responses/:response_id", get(handler))
        .route("/responses/:response_id", get(handler))
        .with_state((response_started.clone(), drop_notify.clone()));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    (base, handle, response_started, drop_notify)
}

async fn spawn_namespaced_openai_echo_mock(
    tag: &'static str,
) -> (String, tokio::task::JoinHandle<()>, CapturedLiveRequests) {
    async fn handler(
        State((tag, captured)): State<(&'static str, CapturedLiveRequests)>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        let session = body["messages"][0]["content"]
            .as_str()
            .unwrap_or("missing-session")
            .to_string();
        let namespace_header = headers
            .get("x-namespace-tag")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();

        captured.requests.lock().unwrap().push(CapturedLiveRequest {
            session: session.clone(),
            namespace_header: namespace_header.clone(),
        });

        (
            StatusCode::OK,
            Json(json!({
                "id": format!("chatcmpl_{tag}_{session}"),
                "object": "chat.completion",
                "created": 1,
                "model": body.get("model").unwrap_or(&json!("gpt-4")),
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": format!("ns={tag};session={session};header={namespace_header}")
                    },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
            })),
        )
            .into_response()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let captured = CapturedLiveRequests::default();
    let app = Router::new()
        .route("/v1/chat/completions", post(handler))
        .route("/chat/completions", post(handler))
        .with_state((tag, captured.clone()));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, captured)
}

async fn spawn_prompt_fatal_anthropic_unsupported_block_mock(
) -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(Json(body): Json<Value>) -> Response {
        let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        if !stream {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "msg_thinking",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-3",
                    "content": [
                        { "type": "redacted_thinking" },
                        { "type": "text", "text": "Hi" }
                    ],
                    "stop_reason": "end_turn",
                    "stop_sequence": null,
                    "usage": { "input_tokens": 1, "output_tokens": 2 }
                })),
            )
                .into_response();
        }

        let stream = ControlledSseStream::new(vec![
            (
                Duration::ZERO,
                concat!(
                    "event: message_start\n",
                    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_thinking\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-3\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\n"
                ),
            ),
            (
                Duration::ZERO,
                concat!(
                    "event: content_block_start\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"redacted_thinking\"}}\n\n"
                ),
            ),
        ]);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/messages", post(handler))
        .route("/messages", post(handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn spawn_failed_responses_stream_mock() -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(Json(body): Json<Value>) -> Response {
        let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        if !stream {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "resp_failed",
                    "object": "response",
                    "status": "failed",
                    "error": { "type": "server_error", "message": "boom" },
                    "usage": { "input_tokens": 1, "output_tokens": 0, "total_tokens": 1 },
                    "output": []
                })),
            )
                .into_response();
        }

        let stream = ControlledSseStream::new(vec![
            (
                Duration::ZERO,
                concat!(
                    "event: response.failed\n",
                    "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_failed\",\"object\":\"response\",\"status\":\"failed\",\"error\":{\"type\":\"server_error\",\"message\":\"boom\"},\"incomplete_details\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0,\"total_tokens\":1},\"output\":[]}}\n\n"
                ),
            ),
            (Duration::from_secs(30), "data: [DONE]\n\n"),
        ]);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/responses", post(handler))
        .route("/responses", post(handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn spawn_large_openai_stream_mock(
    text_len: usize,
    terminal_delay: Duration,
) -> (String, tokio::task::JoinHandle<()>, Arc<Notify>) {
    let drop_notify = Arc::new(Notify::new());
    let text = "x".repeat(text_len);

    async fn handler(
        State((text, drop_notify, terminal_delay_ms)): State<(Arc<String>, Arc<Notify>, u64)>,
        Json(body): Json<Value>,
    ) -> Response {
        let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        if !stream {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "chatcmpl_large",
                    "object": "chat.completion",
                    "created": 1,
                    "model": "gpt-4",
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": &*text },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 1,
                        "completion_tokens": text.chars().count(),
                        "total_tokens": text.chars().count() + 1
                    }
                })),
            )
                .into_response();
        }

        let first = format!(
            "data: {{\"id\":\"chatcmpl_large\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"gpt-4\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":{}}},\"finish_reason\":null}}]}}\n\n",
            serde_json::to_string(&*text).unwrap()
        );
        let stream = ControlledSseStream::new(vec![
            (Duration::ZERO, Bytes::from(first)),
            (
                Duration::from_millis(terminal_delay_ms),
                Bytes::from_static(b"data: [DONE]\n\n"),
            ),
        ])
        .with_drop_notify(drop_notify);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let text = Arc::new(text);
    let app = Router::new()
        .route("/v1/chat/completions", post(handler))
        .route("/chat/completions", post(handler))
        .with_state((
            text.clone(),
            drop_notify.clone(),
            terminal_delay.as_millis() as u64,
        ));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, drop_notify)
}

fn runtime_namespace_config(
    upstream_base: &str,
    format: UpstreamFormat,
    namespace_tag: &str,
) -> RuntimeConfigPayload {
    RuntimeConfigPayload {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout_secs: 30,
        compatibility_mode: llm_universal_proxy::config::CompatibilityMode::Balanced,
        proxy: Some(ProxyConfig::Direct),
        upstreams: vec![RuntimeUpstreamConfig {
            name: "default".to_string(),
            api_root: upstream_api_root(upstream_base, format),
            fixed_upstream_format: Some(format),
            provider_key_env: None,
            upstream_headers: vec![("x-namespace-tag".to_string(), namespace_tag.to_string())],
            proxy: None,
            limits: None,
            surface_defaults: None,
        }],
        model_aliases: std::collections::BTreeMap::new(),
        hooks: RuntimeHookConfig::default(),
        debug_trace: DebugTraceConfig::default(),
        resource_limits: Default::default(),
    }
}

async fn apply_namespace_config(
    client: &Client,
    proxy_base: &str,
    namespace: &str,
    config: RuntimeConfigPayload,
) -> Value {
    let response = client
        .post(format!("{proxy_base}/admin/namespaces/{namespace}/config"))
        .json(&json!({ "config": config }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

#[tokio::test]
async fn translated_stream_delivers_first_chunk_incrementally() {
    let _ = common::mock_upstream::spawn_openai_completion_mock;
    let (mock_base, _mock) = spawn_incremental_anthropic_stream_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let started = Instant::now();
    let mut stream = response.bytes_stream();

    let first = tokio::time::timeout(Duration::from_millis(300), stream.next())
        .await
        .expect("first translated chunk should arrive before delayed upstream content")
        .expect("first stream item")
        .expect("first bytes");
    assert!(
        String::from_utf8_lossy(&first).contains("data:"),
        "first bytes should already contain SSE output: {}",
        String::from_utf8_lossy(&first)
    );
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "first chunk arrived too late: {:?}",
        started.elapsed()
    );

    assert!(
        tokio::time::timeout(Duration::from_millis(150), stream.next())
            .await
            .is_err(),
        "second chunk should still be waiting on delayed upstream content"
    );

    let second = tokio::time::timeout(Duration::from_millis(700), stream.next())
        .await
        .expect("second translated chunk should arrive after upstream delay")
        .expect("second stream item")
        .expect("second bytes");
    assert!(
        String::from_utf8_lossy(&second).contains("Hi"),
        "second chunk should contain translated content: {}",
        String::from_utf8_lossy(&second)
    );
}

#[tokio::test]
async fn downstream_disconnect_stops_upstream_stream_promptly() {
    let (mock_base, _mock, drop_notify) = spawn_disconnect_observing_openai_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut stream = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_millis(300), stream.next())
        .await
        .expect("first chunk")
        .expect("first item")
        .expect("first bytes");
    assert!(
        !first.is_empty(),
        "proxy should deliver at least one upstream chunk before disconnect"
    );

    drop(stream);

    tokio::time::timeout(Duration::from_secs(1), drop_notify.notified())
        .await
        .expect("upstream body should be dropped soon after downstream disconnect");
}

#[tokio::test]
async fn downstream_disconnect_aborts_upstream_before_response_headers() {
    let (mock_base, _mock, request_started, abort_notify) = spawn_pending_openai_send_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;
    let request_body = json!({
        "model": "gpt-4",
        "messages": [{ "role": "user", "content": "Hi" }],
        "stream": true
    });
    let stream = open_raw_http_request(
        &proxy_base,
        "POST",
        "/openai/v1/chat/completions",
        Some(&request_body),
    )
    .await;

    tokio::time::timeout(Duration::from_secs(1), request_started.notified())
        .await
        .expect("proxy should send the upstream request before client disconnects");

    drop(stream);

    tokio::time::timeout(Duration::from_secs(2), abort_notify.notified())
        .await
        .expect("upstream request should be aborted while still waiting for first response bytes");
}

#[tokio::test]
async fn downstream_disconnect_aborts_pending_responses_resource_body_read() {
    let (mock_base, _mock, response_started, drop_notify) =
        spawn_pending_openai_response_resource_body_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;
    let stream = open_raw_http_request(
        &proxy_base,
        "GET",
        "/openai/v1/responses/resp_pending",
        None,
    )
    .await;

    tokio::time::timeout(Duration::from_secs(1), response_started.notified())
        .await
        .expect("resource route should start the upstream response before disconnect");
    tokio::time::sleep(Duration::from_millis(50)).await;

    drop(stream);

    tokio::time::timeout(Duration::from_secs(2), drop_notify.notified())
        .await
        .expect("resource route should abort upstream body reads after downstream disconnect");
}

#[tokio::test]
async fn concurrent_live_requests_keep_namespaces_and_sessions_isolated() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");

    let (alpha_base, _alpha_mock, alpha_captured) =
        spawn_namespaced_openai_echo_mock("alpha").await;
    let (beta_base, _beta_mock, beta_captured) = spawn_namespaced_openai_echo_mock("beta").await;
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;

    let client = Client::new();
    let _ = apply_namespace_config(
        &client,
        &proxy_base,
        "alpha",
        runtime_namespace_config(&alpha_base, UpstreamFormat::OpenAiCompletion, "alpha"),
    )
    .await;
    let _ = apply_namespace_config(
        &client,
        &proxy_base,
        "beta",
        runtime_namespace_config(&beta_base, UpstreamFormat::OpenAiCompletion, "beta"),
    )
    .await;

    let total = 8usize;
    let counter = Arc::new(AtomicUsize::new(0));
    let mut futures = Vec::new();
    for namespace in ["alpha", "beta"] {
        for _ in 0..total {
            let client = client.clone();
            let proxy_base = proxy_base.clone();
            let namespace = namespace.to_string();
            let id = counter.fetch_add(1, Ordering::Relaxed);
            let session = format!("session-{namespace}-{id}");
            futures.push(async move {
                let response = client
                    .post(format!(
                        "{proxy_base}/namespaces/{namespace}/openai/v1/chat/completions"
                    ))
                    .json(&json!({
                        "model": "gpt-4",
                        "messages": [{ "role": "user", "content": session }],
                        "stream": false
                    }))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let body: Value = response.json().await.unwrap();
                (namespace, body)
            });
        }
    }

    let results = join_all(futures).await;
    for (namespace, body) in &results {
        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("");
        assert!(
            content.contains(&format!("ns={namespace}")),
            "response should stay bound to its namespace: {content}"
        );
        assert!(
            content.contains(&format!("header={namespace}")),
            "upstream header injection should stay namespaced: {content}"
        );
        assert!(
            content.contains(&format!("session=session-{namespace}-")),
            "response should echo the right session payload: {content}"
        );
    }

    let alpha_requests = alpha_captured.requests.lock().unwrap().clone();
    let beta_requests = beta_captured.requests.lock().unwrap().clone();
    assert_eq!(alpha_requests.len(), total);
    assert_eq!(beta_requests.len(), total);
    assert!(alpha_requests
        .iter()
        .all(|request| request.namespace_header == "alpha"));
    assert!(beta_requests
        .iter()
        .all(|request| request.namespace_header == "beta"));
    assert!(alpha_requests
        .iter()
        .all(|request| request.session.starts_with("session-alpha-")));
    assert!(beta_requests
        .iter()
        .all(|request| request.session.starts_with("session-beta-")));
}

#[tokio::test]
async fn fatal_translated_stream_rejection_returns_prompt_failure_and_finishes() {
    let (mock_base, _mock) = spawn_prompt_fatal_anthropic_unsupported_block_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Hi",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let started = Instant::now();
    let body = tokio::time::timeout(Duration::from_millis(300), response.text())
        .await
        .expect("fatal rejection stream should finish promptly")
        .unwrap();

    assert!(
        started.elapsed() < Duration::from_millis(300),
        "fatal rejection stream took too long: {:?}",
        started.elapsed()
    );
    assert!(body.contains("event: response.failed"), "body = {body}");
    assert!(body.contains("redacted_thinking"), "body = {body}");
    assert!(!body.contains("response.completed"), "body = {body}");
}

#[tokio::test]
async fn failed_terminal_then_disconnect_is_not_recorded_completed_in_hooks_or_debug_trace() {
    let (mock_base, _mock) = spawn_failed_responses_stream_mock().await;
    let (hook_base, _hook_mock, exchange, usage) = spawn_hook_capture_mock().await;
    let trace_path = unique_temp_path("failed-terminal-trace", "jsonl");

    let mut config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    config.hooks = HookConfig {
        max_pending_bytes: 4 * 1024 * 1024,
        timeout: Duration::from_secs(5),
        failure_threshold: 3,
        cooldown: Duration::from_secs(1),
        exchange: Some(HookEndpointConfig {
            url: format!("{hook_base}/exchange"),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{hook_base}/usage"),
            authorization: None,
        }),
    };
    config.debug_trace = DebugTraceConfig {
        path: Some(trace_path.to_string_lossy().to_string()),
        max_text_chars: 8_192,
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "gpt-4.1",
            "input": "Hi",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut stream = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_millis(300), stream.next())
        .await
        .expect("failed terminal should arrive promptly")
        .expect("first stream item")
        .expect("first bytes");
    let first_text = String::from_utf8_lossy(&first);
    assert!(
        first_text.contains("response.failed"),
        "body = {first_text}"
    );
    drop(stream);

    let exchange_payloads = wait_for_payloads(&exchange, 1).await;
    let usage_payloads = wait_for_payloads(&usage, 1).await;
    let exchange_payload = exchange_payloads.last().unwrap();
    let usage_payload = usage_payloads.last().unwrap();
    let trace_payload = wait_for_debug_trace_response(&trace_path).await;

    assert_eq!(
        exchange_payload.get("completed").and_then(Value::as_bool),
        Some(false),
        "failure terminal followed by disconnect should not be normalized to completed in exchange hook: {exchange_payload}"
    );
    assert_ne!(
        exchange_payload
            .get("termination_reason")
            .and_then(Value::as_str),
        Some("completed"),
        "failure terminal followed by disconnect should preserve non-completed termination in exchange hook: {exchange_payload}"
    );
    assert_eq!(
        usage_payload.get("completed").and_then(Value::as_bool),
        Some(false),
        "failure terminal followed by disconnect should not be normalized to completed in usage hook: {usage_payload}"
    );
    assert_ne!(
        usage_payload
            .get("termination_reason")
            .and_then(Value::as_str),
        Some("completed"),
        "failure terminal followed by disconnect should preserve non-completed termination in usage hook: {usage_payload}"
    );
    assert_eq!(
        trace_payload
            .get("response")
            .and_then(|response| response.get("terminal_event"))
            .and_then(Value::as_str),
        Some("response.failed"),
        "debug trace should still show the failure terminal it observed: {trace_payload}"
    );
    assert_ne!(
        trace_payload.get("outcome").and_then(Value::as_str),
        Some("completed"),
        "failure terminal followed by disconnect should not be normalized to completed in debug trace: {trace_payload}"
    );

    let _ = std::fs::remove_file(&trace_path);
}

#[cfg(unix)]
#[tokio::test]
async fn slow_debug_trace_sink_does_not_delay_stream_teardown() {
    let slow_sink = SlowTraceSinkGuard::new("slow-debug-trace");
    let (mock_base, _mock, drop_notify) =
        spawn_large_openai_stream_mock(200_000, Duration::from_secs(30)).await;

    let mut config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    config.debug_trace = DebugTraceConfig {
        path: Some(slow_sink.path_string()),
        max_text_chars: 200_000,
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut stream = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("first chunk should arrive even with debug trace enabled")
        .expect("first stream item")
        .expect("first bytes");
    assert!(
        !first.is_empty(),
        "first streamed chunk should not be empty"
    );
    drop(stream);

    tokio::time::timeout(Duration::from_millis(300), drop_notify.notified())
        .await
        .expect("slow debug trace finalize should not delay upstream teardown");
}

#[cfg(unix)]
#[tokio::test]
async fn debug_trace_background_writer_does_not_silent_drop_under_blocked_sink() {
    let mut trace_capture = BlockingTraceCapture::new("blocked-debug-trace");
    let (mock_base, _mock, _drop_notify) =
        spawn_large_openai_stream_mock(8_192, Duration::ZERO).await;

    let mut config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    config.debug_trace = DebugTraceConfig {
        path: Some(trace_capture.path_string()),
        max_text_chars: 256,
    };
    let (proxy_base, proxy) = start_proxy(config).await;

    let client = Client::new();
    let request_count = 400usize;
    for idx in 0..request_count {
        let response = client
            .post(format!("{proxy_base}/openai/v1/chat/completions"))
            .json(&json!({
                "model": "gpt-4",
                "messages": [{ "role": "user", "content": format!("req-{idx}") }]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.bytes().await.unwrap();
    }

    trace_capture.begin_drain();
    proxy.abort();
    let _ = proxy.await;
    let trace_entries = trace_capture.collect();

    let request_entries: Vec<&Value> = trace_entries
        .iter()
        .filter(|entry| entry.get("phase").and_then(Value::as_str) == Some("request"))
        .collect();
    let response_entries: Vec<&Value> = trace_entries
        .iter()
        .filter(|entry| entry.get("phase").and_then(Value::as_str) == Some("response"))
        .collect();
    let request_ids: HashSet<String> = request_entries
        .iter()
        .filter_map(|entry| entry.get("request_id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect();
    let response_ids: HashSet<String> = response_entries
        .iter()
        .filter_map(|entry| entry.get("request_id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect();

    assert_eq!(
        request_entries.len(),
        request_count,
        "blocked debug trace sink should not drop request entries silently: {trace_entries:?}"
    );
    assert_eq!(
        response_entries.len(),
        request_count,
        "blocked debug trace sink should not drop response entries silently: {trace_entries:?}"
    );
    assert_eq!(
        request_ids.len(),
        request_count,
        "every request should keep a unique request-phase trace entry even while the writer is blocked"
    );
    assert_eq!(
        response_ids.len(),
        request_count,
        "every request should keep a unique response-phase trace entry even while the writer is blocked"
    );
    assert_eq!(
        request_ids, response_ids,
        "blocked debug trace writer should eventually flush both phases for every request without silent drop"
    );
}

#[tokio::test]
async fn exchange_capture_for_long_stream_is_bounded_by_capture_budget() {
    let (mock_base, _mock, _drop_notify) =
        spawn_large_openai_stream_mock(120_000, Duration::ZERO).await;
    let (hook_base, _hook_mock, exchange, _usage) = spawn_hook_capture_mock().await;
    let capture_budget_bytes = 16 * 1024usize;

    let mut config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    config.hooks = HookConfig {
        max_pending_bytes: capture_budget_bytes,
        timeout: Duration::from_secs(5),
        failure_threshold: 3,
        cooldown: Duration::from_secs(1),
        exchange: Some(HookEndpointConfig {
            url: format!("{hook_base}/exchange"),
            authorization: None,
        }),
        usage: None,
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.text().await.unwrap();
    assert!(body.contains("[DONE]"), "stream should complete: {body}");

    let exchange_payloads = wait_for_payloads(&exchange, 1).await;
    let exchange_payload = exchange_payloads.last().unwrap();
    let response_body = exchange_payload
        .get("response")
        .and_then(|response| response.get("body"))
        .expect("exchange payload should include response body");

    assert_eq!(
        response_body
            .get("capture_truncated")
            .and_then(Value::as_bool),
        Some(true),
        "exchange capture should report truncation instead of replaying an unbounded long stream body: {exchange_payload}"
    );
    assert_eq!(
        response_body.get("reason").and_then(Value::as_str),
        Some("capture_budget_exceeded"),
        "exchange capture should explain that truncation was caused by the configured capture budget: {exchange_payload}"
    );
    assert_eq!(
        response_body
            .get("capture_budget_bytes")
            .and_then(Value::as_u64),
        Some(capture_budget_bytes as u64),
        "exchange capture should surface the configured capture budget in its truncated payload: {exchange_payload}"
    );
    assert!(
        response_body
            .get("captured_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
            <= capture_budget_bytes as u64,
        "exchange capture should never spool more than its configured budget: {exchange_payload}"
    );
    assert!(
        response_body
            .get("dropped_event_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1,
        "exchange capture should report dropped events once the budget is exceeded: {exchange_payload}"
    );
    assert!(
        response_body.get("choices").is_none(),
        "truncated exchange capture should not replay a full synthesized response body once the capture budget is exceeded: {exchange_payload}"
    );
}
