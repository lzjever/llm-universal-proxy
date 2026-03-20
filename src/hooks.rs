use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::Stream;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::warn;
use uuid::Uuid;

use crate::config::{HookConfig, HookEndpointConfig};
use crate::formats::UpstreamFormat;
use crate::streaming::take_one_sse_event;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    Client,
    Server,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct NormalizedUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
}

impl NormalizedUsage {
    pub fn from_client_body(format: UpstreamFormat, body: &Value) -> Self {
        match format {
            UpstreamFormat::OpenAiCompletion => {
                let usage = body.get("usage").unwrap_or(&Value::Null);
                let input_tokens = usage.get("prompt_tokens").and_then(Value::as_u64);
                let output_tokens = usage.get("completion_tokens").and_then(Value::as_u64);
                let total_tokens =
                    usage
                        .get("total_tokens")
                        .and_then(Value::as_u64)
                        .or_else(|| match (input_tokens, output_tokens) {
                            (Some(i), Some(o)) => Some(i + o),
                            _ => None,
                        });
                let cached_input_tokens = usage
                    .get("prompt_tokens_details")
                    .and_then(|v| v.get("cached_tokens"))
                    .and_then(Value::as_u64)
                    .or_else(|| usage.get("cache_read_input_tokens").and_then(Value::as_u64));
                let reasoning_tokens = usage
                    .get("completion_tokens_details")
                    .and_then(|v| v.get("reasoning_tokens"))
                    .and_then(Value::as_u64);
                Self {
                    input_tokens,
                    output_tokens,
                    total_tokens,
                    cached_input_tokens,
                    reasoning_tokens,
                    cache_creation_input_tokens: usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64),
                    cache_read_input_tokens: usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64),
                }
            }
            UpstreamFormat::OpenAiResponses => {
                let usage = body.get("usage").unwrap_or(&Value::Null);
                let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
                let output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
                let total_tokens =
                    usage
                        .get("total_tokens")
                        .and_then(Value::as_u64)
                        .or_else(|| match (input_tokens, output_tokens) {
                            (Some(i), Some(o)) => Some(i + o),
                            _ => None,
                        });
                Self {
                    input_tokens,
                    output_tokens,
                    total_tokens,
                    cached_input_tokens: usage
                        .get("input_tokens_details")
                        .and_then(|v| v.get("cached_tokens"))
                        .and_then(Value::as_u64),
                    reasoning_tokens: usage
                        .get("output_tokens_details")
                        .and_then(|v| v.get("reasoning_tokens"))
                        .and_then(Value::as_u64),
                    cache_creation_input_tokens: usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64),
                    cache_read_input_tokens: usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64),
                }
            }
            UpstreamFormat::Anthropic => {
                let usage = body.get("usage").unwrap_or(&Value::Null);
                let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
                let output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
                let total_tokens = match (input_tokens, output_tokens) {
                    (Some(i), Some(o)) => Some(i + o),
                    _ => None,
                };
                Self {
                    input_tokens,
                    output_tokens,
                    total_tokens,
                    cached_input_tokens: usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64),
                    reasoning_tokens: None,
                    cache_creation_input_tokens: usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64),
                    cache_read_input_tokens: usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64),
                }
            }
            UpstreamFormat::Google => {
                let usage = body.get("usageMetadata").unwrap_or(&Value::Null);
                let input_tokens = usage.get("promptTokenCount").and_then(Value::as_u64);
                let output_tokens = usage.get("candidatesTokenCount").and_then(Value::as_u64);
                Self {
                    input_tokens,
                    output_tokens,
                    total_tokens: usage
                        .get("totalTokenCount")
                        .and_then(Value::as_u64)
                        .or_else(|| match (input_tokens, output_tokens) {
                            (Some(i), Some(o)) => Some(i + o),
                            _ => None,
                        }),
                    cached_input_tokens: usage
                        .get("cachedContentTokenCount")
                        .and_then(Value::as_u64),
                    reasoning_tokens: usage.get("thoughtsTokenCount").and_then(Value::as_u64),
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HookRequestContext {
    pub request_id: String,
    pub timestamp_ms: u128,
    pub path: String,
    pub method: String,
    pub stream: bool,
    pub client_model: String,
    pub upstream_name: String,
    pub upstream_model: String,
    pub client_format: UpstreamFormat,
    pub upstream_format: UpstreamFormat,
    pub credential_source: CredentialSource,
    pub credential_fingerprint: Option<String>,
    pub client_request_headers: Vec<HeaderEntry>,
    pub client_request_body: Value,
}

#[derive(Debug, Clone)]
pub struct HookDispatcher {
    exchange: Option<HookSender>,
    usage: Option<HookSender>,
    runtime: Arc<HookRuntime>,
}

#[derive(Debug, Clone)]
struct HookSender {
    client: reqwest::Client,
    config: HookEndpointConfig,
    kind: HookKind,
    runtime: Arc<HookRuntime>,
}

#[derive(Debug, Clone, Copy)]
enum HookKind {
    Exchange,
    Usage,
}

#[derive(Debug)]
struct HookRuntime {
    max_pending_bytes: usize,
    pending_bytes: AtomicUsize,
    timeout: Duration,
    failure_threshold: usize,
    cooldown: Duration,
    breaker: Mutex<HookBreakerState>,
}

#[derive(Debug, Default)]
struct HookBreakerState {
    exchange: CircuitState,
    usage: CircuitState,
}

#[derive(Debug, Default)]
struct CircuitState {
    consecutive_failures: usize,
    open_until: Option<Instant>,
}

impl HookRuntime {
    fn can_attempt(&self, kind: HookKind) -> bool {
        let mut breaker = self.breaker.lock().unwrap();
        let state = breaker.state_mut(kind);
        match state.open_until {
            Some(until) if Instant::now() < until => false,
            Some(_) => {
                state.open_until = None;
                state.consecutive_failures = 0;
                true
            }
            None => true,
        }
    }

    fn can_capture_exchange(&self) -> bool {
        self.can_attempt(HookKind::Exchange)
            && self.pending_bytes.load(Ordering::Relaxed) < self.max_pending_bytes
    }

    fn try_reserve(&self, bytes: usize) -> bool {
        loop {
            let current = self.pending_bytes.load(Ordering::Relaxed);
            let Some(next) = current.checked_add(bytes) else {
                return false;
            };
            if next > self.max_pending_bytes {
                return false;
            }
            if self
                .pending_bytes
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
        }
    }

    fn release(&self, bytes: usize) {
        self.pending_bytes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                Some(current.saturating_sub(bytes))
            })
            .ok();
    }

    fn record_success(&self, kind: HookKind) {
        let mut breaker = self.breaker.lock().unwrap();
        let state = breaker.state_mut(kind);
        state.consecutive_failures = 0;
        state.open_until = None;
    }

    fn record_failure(&self, kind: HookKind) {
        let mut breaker = self.breaker.lock().unwrap();
        let state = breaker.state_mut(kind);
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.consecutive_failures >= self.failure_threshold {
            state.open_until = Some(Instant::now() + self.cooldown);
            state.consecutive_failures = 0;
        }
    }
}

impl HookBreakerState {
    fn state_mut(&mut self, kind: HookKind) -> &mut CircuitState {
        match kind {
            HookKind::Exchange => &mut self.exchange,
            HookKind::Usage => &mut self.usage,
        }
    }
}

impl HookDispatcher {
    pub fn new(config: &HookConfig) -> Option<Self> {
        if !config.is_enabled() {
            return None;
        }
        let runtime = Arc::new(HookRuntime {
            max_pending_bytes: config.max_pending_bytes,
            pending_bytes: AtomicUsize::new(0),
            timeout: config.timeout,
            failure_threshold: config.failure_threshold,
            cooldown: config.cooldown,
            breaker: Mutex::new(HookBreakerState::default()),
        });
        Some(Self {
            exchange: config
                .exchange
                .clone()
                .map(|cfg| HookSender::new(cfg, HookKind::Exchange, runtime.clone())),
            usage: config
                .usage
                .clone()
                .map(|cfg| HookSender::new(cfg, HookKind::Usage, runtime.clone())),
            runtime,
        })
    }

    pub fn emit_non_stream(
        &self,
        ctx: HookRequestContext,
        status: u16,
        response_headers: Vec<HeaderEntry>,
        response_body: Value,
    ) {
        let usage = NormalizedUsage::from_client_body(ctx.client_format, &response_body);
        self.emit_exchange(
            &ctx,
            status,
            response_headers.clone(),
            response_body.clone(),
            true,
        );
        self.emit_usage(&ctx, status, usage, true);
    }

    pub fn wrap_stream<S>(
        &self,
        inner: S,
        ctx: HookRequestContext,
        status: u16,
        response_headers: Vec<HeaderEntry>,
    ) -> HookCaptureStream<S>
    where
        S: Stream<Item = Result<Bytes, std::io::Error>>,
    {
        let capture_enabled = self.runtime.can_capture_exchange();
        HookCaptureStream {
            inner,
            buffer: Vec::new(),
            accumulator: ClientSseAccumulator::new(ctx.client_format),
            dispatcher: self.clone(),
            ctx,
            status,
            response_headers,
            finalized: false,
            capture_enabled,
        }
    }

    fn emit_exchange(
        &self,
        ctx: &HookRequestContext,
        status: u16,
        response_headers: Vec<HeaderEntry>,
        response_body: Value,
        completed: bool,
    ) {
        let Some(sender) = self.exchange.clone() else {
            return;
        };
        if !self.runtime.can_attempt(HookKind::Exchange) {
            return;
        }
        let payload = json!({
            "request_id": ctx.request_id,
            "timestamp_ms": ctx.timestamp_ms,
            "path": ctx.path,
            "method": ctx.method,
            "stream": ctx.stream,
            "client_format": ctx.client_format,
            "upstream_format": ctx.upstream_format,
            "client_model": ctx.client_model,
            "upstream_name": ctx.upstream_name,
            "upstream_model": ctx.upstream_model,
            "credential_source": ctx.credential_source,
            "credential_fingerprint": ctx.credential_fingerprint,
            "completed": completed,
            "request": {
                "headers": ctx.client_request_headers,
                "body": ctx.client_request_body,
            },
            "response": {
                "status": status,
                "headers": response_headers,
                "body": response_body,
            }
        });
        sender.spawn_send(payload);
    }

    fn emit_usage(
        &self,
        ctx: &HookRequestContext,
        status: u16,
        usage: NormalizedUsage,
        completed: bool,
    ) {
        let Some(sender) = self.usage.clone() else {
            return;
        };
        if !self.runtime.can_attempt(HookKind::Usage) {
            return;
        }
        let payload = json!({
            "request_id": ctx.request_id,
            "timestamp_ms": ctx.timestamp_ms,
            "path": ctx.path,
            "stream": ctx.stream,
            "completed": completed,
            "status": if (200..300).contains(&status) { "success" } else { "error" },
            "http_status": status,
            "client_model": ctx.client_model,
            "upstream_name": ctx.upstream_name,
            "upstream_model": ctx.upstream_model,
            "client_format": ctx.client_format,
            "upstream_format": ctx.upstream_format,
            "credential_source": ctx.credential_source,
            "credential_fingerprint": ctx.credential_fingerprint,
            "usage": usage,
        });
        sender.spawn_send(payload);
    }
}

impl HookSender {
    fn new(config: HookEndpointConfig, kind: HookKind, runtime: Arc<HookRuntime>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(runtime.timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            config,
            kind,
            runtime,
        }
    }

    fn spawn_send(self, payload: Value) {
        let Ok(serialized) = serde_json::to_vec(&payload) else {
            return;
        };
        let payload_len = serialized.len();
        if !self.runtime.try_reserve(payload_len) {
            warn!(
                "hook payload dropped: kind={:?} reason=max_pending_bytes_exceeded size={}",
                self.kind, payload_len
            );
            return;
        }
        tokio::spawn(async move {
            self.send(serialized, payload_len).await;
        });
    }

    async fn send(self, payload: Vec<u8>, payload_len: usize) {
        let mut req = self
            .client
            .post(&self.config.url)
            .body(payload)
            .header("content-type", "application/json");
        if let Some(auth) = &self.config.authorization {
            req = req.header("authorization", auth);
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                self.runtime.record_success(self.kind);
            }
            Ok(resp) => {
                if resp.status().is_server_error() {
                    self.runtime.record_failure(self.kind);
                }
                warn!(
                    "hook delivery failed: kind={:?} url={} status={}",
                    self.kind,
                    self.config.url,
                    resp.status()
                );
            }
            Err(err) => {
                self.runtime.record_failure(self.kind);
                warn!(
                    "hook delivery error: kind={:?} url={} error={}",
                    self.kind, self.config.url, err
                );
            }
        }
        self.runtime.release(payload_len);
    }
}

pub fn fingerprint_credential(credential: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"llm-universal-proxy:v1:");
    hasher.update(credential.as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

pub fn now_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn new_request_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn capture_headers(headers: &axum::http::HeaderMap) -> Vec<HeaderEntry> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value.to_str().ok().map(|v| HeaderEntry {
                name: name.as_str().to_string(),
                value: v.to_string(),
            })
        })
        .collect()
}

pub fn json_response_headers() -> Vec<HeaderEntry> {
    vec![HeaderEntry {
        name: "content-type".to_string(),
        value: "application/json".to_string(),
    }]
}

pub fn sse_response_headers() -> Vec<HeaderEntry> {
    vec![
        HeaderEntry {
            name: "content-type".to_string(),
            value: "text/event-stream".to_string(),
        },
        HeaderEntry {
            name: "cache-control".to_string(),
            value: "no-cache".to_string(),
        },
        HeaderEntry {
            name: "connection".to_string(),
            value: "keep-alive".to_string(),
        },
    ]
}

pub struct HookCaptureStream<S> {
    inner: S,
    buffer: Vec<u8>,
    accumulator: ClientSseAccumulator,
    dispatcher: HookDispatcher,
    ctx: HookRequestContext,
    status: u16,
    response_headers: Vec<HeaderEntry>,
    finalized: bool,
    capture_enabled: bool,
}

impl<S> HookCaptureStream<S> {
    fn finalize(&mut self, completed: bool) {
        if self.finalized {
            return;
        }
        self.finalized = true;
        let usage = self.accumulator.final_usage();
        if self.capture_enabled {
            let response_body = self.accumulator.final_response_body();
            self.dispatcher.emit_exchange(
                &self.ctx,
                self.status,
                self.response_headers.clone(),
                response_body,
                completed,
            );
        }
        self.dispatcher
            .emit_usage(&self.ctx, self.status, usage, completed);
    }
}

impl<S> Stream for HookCaptureStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                if this.capture_enabled {
                    this.buffer.extend_from_slice(&bytes);
                    while let Some(event) = take_one_sse_event(&mut this.buffer) {
                        this.accumulator.on_event(&event);
                    }
                }
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(err))) => {
                this.finalize(false);
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                this.finalize(true);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug)]
enum ClientSseAccumulator {
    OpenAiCompletion(OpenAiCompletionAccumulator),
    Responses(ResponsesAccumulator),
    Anthropic(AnthropicAccumulator),
    Google(GoogleAccumulator),
}

impl ClientSseAccumulator {
    fn new(format: UpstreamFormat) -> Self {
        match format {
            UpstreamFormat::OpenAiCompletion => Self::OpenAiCompletion(Default::default()),
            UpstreamFormat::OpenAiResponses => Self::Responses(Default::default()),
            UpstreamFormat::Anthropic => Self::Anthropic(Default::default()),
            UpstreamFormat::Google => Self::Google(Default::default()),
        }
    }

    fn on_event(&mut self, event: &Value) {
        match self {
            Self::OpenAiCompletion(acc) => acc.on_event(event),
            Self::Responses(acc) => acc.on_event(event),
            Self::Anthropic(acc) => acc.on_event(event),
            Self::Google(acc) => acc.on_event(event),
        }
    }

    fn final_response_body(&self) -> Value {
        match self {
            Self::OpenAiCompletion(acc) => acc.final_body(),
            Self::Responses(acc) => acc.final_body(),
            Self::Anthropic(acc) => acc.final_body(),
            Self::Google(acc) => acc.final_body(),
        }
    }

    fn final_usage(&self) -> NormalizedUsage {
        match self {
            Self::OpenAiCompletion(acc) => acc.final_usage(),
            Self::Responses(acc) => acc.final_usage(),
            Self::Anthropic(acc) => acc.final_usage(),
            Self::Google(acc) => acc.final_usage(),
        }
    }
}

#[derive(Debug, Default)]
struct OpenAiToolCallAccumulator {
    id: Option<String>,
    name: String,
    arguments: String,
}

#[derive(Debug, Default)]
struct OpenAiCompletionAccumulator {
    id: Option<String>,
    created: Option<u64>,
    model: Option<String>,
    role: Option<String>,
    content: String,
    finish_reason: Option<String>,
    usage: Option<Value>,
    tool_calls: BTreeMap<usize, OpenAiToolCallAccumulator>,
}

impl OpenAiCompletionAccumulator {
    fn on_event(&mut self, event: &Value) {
        if event.get("_done").and_then(Value::as_bool) == Some(true) {
            return;
        }
        self.id = self.id.clone().or_else(|| {
            event
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        self.created = self
            .created
            .or_else(|| event.get("created").and_then(Value::as_u64));
        self.model = self.model.clone().or_else(|| {
            event
                .get("model")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        if let Some(usage) = event.get("usage") {
            self.usage = Some(usage.clone());
        }
        let Some(choice) = event
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };
        if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(finish_reason.to_string());
        }
        let delta = choice.get("delta").unwrap_or(&Value::Null);
        if let Some(role) = delta.get("role").and_then(Value::as_str) {
            self.role = Some(role.to_string());
        }
        if let Some(content) = delta.get("content").and_then(Value::as_str) {
            self.content.push_str(content);
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in tool_calls {
                let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let entry = self.tool_calls.entry(index).or_default();
                if let Some(id) = tc.get("id").and_then(Value::as_str) {
                    entry.id = Some(id.to_string());
                }
                if let Some(name) = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                {
                    entry.name = name.to_string();
                }
                if let Some(args) = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                {
                    entry.arguments.push_str(args);
                }
            }
        }
    }

    fn final_body(&self) -> Value {
        let mut message = json!({
            "role": self.role.clone().unwrap_or_else(|| "assistant".to_string()),
            "content": self.content,
        });
        if !self.tool_calls.is_empty() {
            let mut tool_calls = self.tool_calls.iter().collect::<Vec<_>>();
            tool_calls.sort_by_key(|(idx, _)| *idx);
            message["tool_calls"] = Value::Array(
                tool_calls
                    .into_iter()
                    .map(|(idx, tool)| {
                        json!({
                            "index": idx,
                            "id": tool.id,
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "arguments": tool.arguments
                            }
                        })
                    })
                    .collect(),
            );
        }
        let mut out = json!({
            "id": self.id,
            "object": "chat.completion",
            "created": self.created.unwrap_or(0),
            "model": self.model.clone().unwrap_or_default(),
            "choices": [{
                "index": 0,
                "message": message,
                "finish_reason": self.finish_reason.clone().unwrap_or_else(|| "stop".to_string())
            }]
        });
        if let Some(usage) = &self.usage {
            out["usage"] = usage.clone();
        }
        out
    }

    fn final_usage(&self) -> NormalizedUsage {
        self.usage
            .as_ref()
            .map(|usage| {
                NormalizedUsage::from_client_body(
                    UpstreamFormat::OpenAiCompletion,
                    &json!({ "usage": usage }),
                )
            })
            .unwrap_or_default()
    }
}

#[derive(Debug, Default)]
struct ResponsesAccumulator {
    final_response: Option<Value>,
    last_usage: Option<NormalizedUsage>,
}

impl ResponsesAccumulator {
    fn on_event(&mut self, event: &Value) {
        if let Some(response) = event.get("response") {
            if event.get("type").and_then(Value::as_str) == Some("response.completed") {
                self.final_response = Some(response.clone());
                self.last_usage = Some(NormalizedUsage::from_client_body(
                    UpstreamFormat::OpenAiResponses,
                    response,
                ));
            }
        }
    }

    fn final_body(&self) -> Value {
        self.final_response.clone().unwrap_or_else(|| json!({}))
    }

    fn final_usage(&self) -> NormalizedUsage {
        self.last_usage.clone().unwrap_or_default()
    }
}

#[derive(Debug, Default)]
struct AnthropicAccumulator {
    message: Value,
    usage: Option<NormalizedUsage>,
}

impl AnthropicAccumulator {
    fn ensure_content_array(&mut self) -> &mut Vec<Value> {
        if self.message.is_null() {
            self.message = json!({
                "id": null,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": null,
                "stop_reason": null,
                "stop_sequence": null
            });
        }
        self.message["content"].as_array_mut().unwrap()
    }

    fn on_event(&mut self, event: &Value) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(message) = event.get("message") {
                    self.message = message.clone();
                    self.message["content"] = Value::Array(Vec::new());
                }
            }
            Some("content_block_start") => {
                let Some(index) = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize)
                else {
                    return;
                };
                let Some(block) = event.get("content_block") else {
                    return;
                };
                let content = self.ensure_content_array();
                while content.len() <= index {
                    content.push(json!({}));
                }
                content[index] = block.clone();
            }
            Some("content_block_delta") => {
                let Some(index) = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize)
                else {
                    return;
                };
                let Some(delta) = event.get("delta") else {
                    return;
                };
                let content = self.ensure_content_array();
                while content.len() <= index {
                    content.push(json!({}));
                }
                let block = &mut content[index];
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let existing = block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let add = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        block["type"] = json!("text");
                        block["text"] = json!(format!("{}{}", existing, add));
                    }
                    Some("thinking_delta") => {
                        let existing = block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let add = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                        block["type"] = json!("thinking");
                        block["thinking"] = json!(format!("{}{}", existing, add));
                    }
                    Some("input_json_delta") => {
                        let existing = block
                            .get("input_json_raw")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let add = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        block["input_json_raw"] = json!(format!("{}{}", existing, add));
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(stop_reason) = event
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.message["stop_reason"] = json!(stop_reason);
                }
                if let Some(usage) = event.get("usage") {
                    self.message["usage"] = usage.clone();
                    self.usage = Some(NormalizedUsage::from_client_body(
                        UpstreamFormat::Anthropic,
                        &json!({ "usage": usage }),
                    ));
                }
            }
            Some("message_stop") => {
                if let Some(content) = self
                    .message
                    .get_mut("content")
                    .and_then(Value::as_array_mut)
                {
                    for block in content.iter_mut() {
                        if let Some(raw) = block.get("input_json_raw").and_then(Value::as_str) {
                            if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
                                block["input"] = parsed;
                            }
                        }
                        if let Some(obj) = block.as_object_mut() {
                            obj.remove("input_json_raw");
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn final_body(&self) -> Value {
        self.message.clone()
    }

    fn final_usage(&self) -> NormalizedUsage {
        self.usage.clone().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(
        max_pending_bytes: usize,
        failure_threshold: usize,
        cooldown_ms: u64,
    ) -> HookRuntime {
        HookRuntime {
            max_pending_bytes,
            pending_bytes: AtomicUsize::new(0),
            timeout: Duration::from_secs(30),
            failure_threshold,
            cooldown: Duration::from_millis(cooldown_ms),
            breaker: Mutex::new(HookBreakerState::default()),
        }
    }

    #[test]
    fn pending_budget_rejects_oversized_reservation() {
        let runtime = runtime(10, 3, 100);
        assert!(runtime.try_reserve(6));
        assert!(!runtime.try_reserve(5));
        runtime.release(6);
        assert!(runtime.try_reserve(10));
    }

    #[test]
    fn circuit_breaker_opens_and_recovers_after_cooldown() {
        let runtime = runtime(1024, 2, 5);
        assert!(runtime.can_attempt(HookKind::Usage));
        runtime.record_failure(HookKind::Usage);
        assert!(runtime.can_attempt(HookKind::Usage));
        runtime.record_failure(HookKind::Usage);
        assert!(!runtime.can_attempt(HookKind::Usage));
        std::thread::sleep(Duration::from_millis(10));
        assert!(runtime.can_attempt(HookKind::Usage));
        runtime.record_success(HookKind::Usage);
        assert!(runtime.can_attempt(HookKind::Usage));
    }
}

#[derive(Debug, Default)]
struct GoogleAccumulator {
    response: Value,
    usage: Option<NormalizedUsage>,
}

impl GoogleAccumulator {
    fn on_event(&mut self, event: &Value) {
        if self.response.is_null() {
            self.response = json!({
                "candidates": [{
                    "content": { "parts": [], "role": "model" }
                }]
            });
        }
        if let Some(candidates) = event.get("candidates").and_then(Value::as_array) {
            if let Some(candidate) = candidates.first() {
                if let Some(parts) = candidate
                    .get("content")
                    .and_then(|c| c.get("parts"))
                    .and_then(Value::as_array)
                {
                    let dest_parts = self.response["candidates"][0]["content"]["parts"]
                        .as_array_mut()
                        .unwrap();
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            if let Some(last) = dest_parts.last_mut() {
                                if last.get("text").is_some() {
                                    let existing =
                                        last.get("text").and_then(Value::as_str).unwrap_or("");
                                    last["text"] = json!(format!("{}{}", existing, text));
                                    continue;
                                }
                            }
                            dest_parts.push(json!({ "text": text }));
                        } else if part.get("functionCall").is_some() {
                            dest_parts.push(part.clone());
                        }
                    }
                }
                if let Some(reason) = candidate.get("finishReason") {
                    self.response["candidates"][0]["finishReason"] = reason.clone();
                }
            }
        }
        if let Some(model) = event.get("modelVersion") {
            self.response["modelVersion"] = model.clone();
        }
        if let Some(usage) = event.get("usageMetadata") {
            self.response["usageMetadata"] = usage.clone();
            self.usage = Some(NormalizedUsage::from_client_body(
                UpstreamFormat::Google,
                &json!({ "usageMetadata": usage }),
            ));
        }
    }

    fn final_body(&self) -> Value {
        self.response.clone()
    }

    fn final_usage(&self) -> NormalizedUsage {
        self.usage.clone().unwrap_or_default()
    }
}
