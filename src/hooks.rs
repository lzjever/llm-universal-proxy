use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
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

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TerminationReason {
    Completed,
    Incomplete,
    Failed,
    ClientDisconnected,
    StreamError,
}

#[derive(Debug, Clone, Copy)]
struct LegacyFinalizationState {
    completed: bool,
    cancelled_by_client: bool,
    partial: bool,
    termination_reason: TerminationReason,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum UsageHookStatus {
    Success,
    Incomplete,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TransportOutcome {
    CompletedEof,
    ClientDisconnected,
    StreamError,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ProtocolTerminalKind {
    Success,
    Incomplete,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct ProtocolTerminal {
    kind: ProtocolTerminalKind,
    event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    incomplete_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

#[derive(Debug, Clone)]
struct StreamObservation {
    transport_outcome: TransportOutcome,
    protocol_terminal: Option<ProtocolTerminal>,
}

impl StreamObservation {
    fn non_stream_completed() -> Self {
        Self {
            transport_outcome: TransportOutcome::CompletedEof,
            protocol_terminal: None,
        }
    }

    fn project_legacy(&self) -> LegacyFinalizationState {
        if let Some(terminal) = &self.protocol_terminal {
            return match terminal.kind {
                ProtocolTerminalKind::Success => LegacyFinalizationState {
                    completed: true,
                    cancelled_by_client: false,
                    partial: false,
                    termination_reason: TerminationReason::Completed,
                },
                ProtocolTerminalKind::Incomplete => LegacyFinalizationState {
                    completed: false,
                    cancelled_by_client: false,
                    partial: true,
                    termination_reason: TerminationReason::Incomplete,
                },
                ProtocolTerminalKind::Failed => LegacyFinalizationState {
                    completed: false,
                    cancelled_by_client: false,
                    partial: false,
                    termination_reason: TerminationReason::Failed,
                },
            };
        }

        match self.transport_outcome {
            TransportOutcome::CompletedEof => LegacyFinalizationState {
                completed: true,
                cancelled_by_client: false,
                partial: false,
                termination_reason: TerminationReason::Completed,
            },
            TransportOutcome::ClientDisconnected => LegacyFinalizationState {
                completed: false,
                cancelled_by_client: true,
                partial: true,
                termination_reason: TerminationReason::ClientDisconnected,
            },
            TransportOutcome::StreamError => LegacyFinalizationState {
                completed: false,
                cancelled_by_client: false,
                partial: true,
                termination_reason: TerminationReason::StreamError,
            },
        }
    }

    fn project_usage_status(&self, http_status: u16) -> UsageHookStatus {
        if let Some(terminal) = &self.protocol_terminal {
            return match terminal.kind {
                ProtocolTerminalKind::Success => {
                    if (200..300).contains(&http_status) {
                        UsageHookStatus::Success
                    } else {
                        UsageHookStatus::Error
                    }
                }
                ProtocolTerminalKind::Incomplete => UsageHookStatus::Incomplete,
                ProtocolTerminalKind::Failed => UsageHookStatus::Error,
            };
        }

        match self.transport_outcome {
            TransportOutcome::CompletedEof => {
                if (200..300).contains(&http_status) {
                    UsageHookStatus::Success
                } else {
                    UsageHookStatus::Error
                }
            }
            TransportOutcome::ClientDisconnected => UsageHookStatus::Cancelled,
            TransportOutcome::StreamError => UsageHookStatus::Error,
        }
    }
}

fn openai_protocol_terminal(event: &Value) -> Option<ProtocolTerminal> {
    if let Some(error) = event.get("error") {
        return Some(ProtocolTerminal {
            kind: ProtocolTerminalKind::Failed,
            event_type: "chat.completion.chunk".to_string(),
            finish_reason: None,
            incomplete_reason: None,
            error: Some(error.clone()),
        });
    }
    let choice = event
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first());
    let finish_reason = choice
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)?;
    let (kind, incomplete_reason) = match finish_reason {
        "length" | "content_filter" | "pause_turn" => (
            ProtocolTerminalKind::Incomplete,
            Some(finish_reason.to_string()),
        ),
        "context_length_exceeded" | "tool_error" | "error" => (ProtocolTerminalKind::Failed, None),
        _ => (ProtocolTerminalKind::Success, None),
    };
    Some(ProtocolTerminal {
        kind,
        event_type: "chat.completion.chunk".to_string(),
        finish_reason: Some(finish_reason.to_string()),
        incomplete_reason,
        error: event.get("error").cloned(),
    })
}

fn responses_protocol_terminal(event: &Value) -> Option<ProtocolTerminal> {
    let event_type = event.get("type").and_then(Value::as_str)?;
    match event_type {
        "response.completed" => Some(ProtocolTerminal {
            kind: ProtocolTerminalKind::Success,
            event_type: event_type.to_string(),
            finish_reason: None,
            incomplete_reason: None,
            error: None,
        }),
        "response.incomplete" => Some(ProtocolTerminal {
            kind: ProtocolTerminalKind::Incomplete,
            event_type: event_type.to_string(),
            finish_reason: None,
            incomplete_reason: event
                .get("response")
                .and_then(|response| response.get("incomplete_details"))
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str)
                .map(str::to_string),
            error: None,
        }),
        "response.failed" => Some(ProtocolTerminal {
            kind: ProtocolTerminalKind::Failed,
            event_type: event_type.to_string(),
            finish_reason: None,
            incomplete_reason: None,
            error: event
                .get("response")
                .and_then(|response| response.get("error"))
                .cloned()
                .filter(|error| !error.is_null()),
        }),
        _ => None,
    }
}

fn anthropic_protocol_terminal(
    event: &Value,
    last_stop_reason: Option<&str>,
) -> Option<ProtocolTerminal> {
    match event.get("type").and_then(Value::as_str) {
        Some("error") => Some(ProtocolTerminal {
            kind: ProtocolTerminalKind::Failed,
            event_type: "error".to_string(),
            finish_reason: None,
            incomplete_reason: None,
            error: event.get("error").cloned(),
        }),
        Some("message_stop") => {
            let stop_reason = last_stop_reason.unwrap_or("");
            let (kind, incomplete_reason, finish_reason) = match stop_reason {
                "max_tokens" | "pause_turn" | "refusal" => (
                    ProtocolTerminalKind::Incomplete,
                    Some(stop_reason.to_string()),
                    Some(stop_reason.to_string()),
                ),
                "model_context_window_exceeded" => (
                    ProtocolTerminalKind::Failed,
                    None,
                    Some(stop_reason.to_string()),
                ),
                _ => (
                    ProtocolTerminalKind::Success,
                    None,
                    (!stop_reason.is_empty()).then_some(stop_reason.to_string()),
                ),
            };
            Some(ProtocolTerminal {
                kind,
                event_type: "message_stop".to_string(),
                finish_reason,
                incomplete_reason,
                error: None,
            })
        }
        _ => None,
    }
}

fn google_protocol_terminal(event: &Value) -> Option<ProtocolTerminal> {
    let candidate = event
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())?;
    let finish_reason = candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .map(str::to_string)?;
    let kind = match finish_reason.as_str() {
        "MAX_TOKENS" | "SAFETY" | "RECITATION" => ProtocolTerminalKind::Incomplete,
        "MALFORMED_FUNCTION_CALL"
        | "UNEXPECTED_TOOL_CALL"
        | "TOO_MANY_TOOL_CALLS"
        | "MISSING_THOUGHT_SIGNATURE" => ProtocolTerminalKind::Failed,
        _ => ProtocolTerminalKind::Success,
    };
    Some(ProtocolTerminal {
        kind,
        event_type: "candidate".to_string(),
        finish_reason: Some(finish_reason.clone()),
        incomplete_reason: matches!(kind, ProtocolTerminalKind::Incomplete)
            .then_some(finish_reason),
        error: None,
    })
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
pub struct HookSnapshot {
    pub pending_bytes: usize,
    pub max_pending_bytes: usize,
    pub failure_threshold: usize,
    pub exchange: CircuitSnapshot,
    pub usage: CircuitSnapshot,
}

#[derive(Debug, Clone)]
pub struct CircuitSnapshot {
    pub consecutive_failures: usize,
    pub open: bool,
    pub remaining_cooldown_secs: u64,
}

#[derive(Debug, Clone)]
struct HookSender {
    client: reqwest::Client,
    config: HookEndpointConfig,
    kind: HookKind,
    runtime: Arc<HookRuntime>,
}

#[derive(Debug)]
enum ExchangeResponseCapture {
    Immediate(Value),
    Spool(EventSpoolArtifact),
    Unavailable { reason: String },
}

#[derive(Debug)]
struct ExchangeHookPayload {
    ctx: HookRequestContext,
    status: u16,
    response_headers: Vec<HeaderEntry>,
    response_capture: ExchangeResponseCapture,
    observation: StreamObservation,
}

#[derive(Debug)]
enum ExchangeCaptureMode {
    Spooling(EventSpoolSink),
    Unavailable { reason: String },
}

#[derive(Debug)]
struct EventSpoolSink {
    format: UpstreamFormat,
    budget_bytes: usize,
    captured_bytes: usize,
    dropped_event_count: usize,
    overflow_reason: Option<String>,
    writer: Option<SpoolWriterHandle>,
}

#[derive(Debug)]
struct EventSpoolArtifact {
    format: UpstreamFormat,
    state: Option<EventSpoolArtifactState>,
}

#[derive(Debug)]
enum EventSpoolArtifactState {
    Complete {
        writer: SpoolWriterHandle,
    },
    Truncated {
        reason: String,
        capture_budget_bytes: usize,
        captured_bytes: usize,
        dropped_event_count: usize,
        writer: Option<SpoolWriterHandle>,
    },
}

#[derive(Debug)]
struct SpoolWriterHandle {
    sender: Option<mpsc::SyncSender<Vec<u8>>>,
    join: Option<std::thread::JoinHandle<()>>,
    path: Option<PathBuf>,
    error: Arc<Mutex<Option<String>>>,
}

#[derive(Clone)]
struct SpoolWriterOptions {
    queue_capacity: usize,
    #[cfg(test)]
    start_barrier: Option<Arc<std::sync::Barrier>>,
}

impl Default for SpoolWriterOptions {
    fn default() -> Self {
        Self {
            queue_capacity: 64,
            #[cfg(test)]
            start_barrier: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum HookKind {
    Exchange,
    Usage,
}

#[derive(Debug)]
struct HookRuntime {
    max_pending_bytes: usize,
    exchange_capture_budget_bytes: usize,
    pending_bytes: AtomicUsize,
    timeout: Duration,
    failure_threshold: usize,
    cooldown: Duration,
    breaker: Mutex<HookBreakerState>,
}

const DEFAULT_EXCHANGE_CAPTURE_BUDGET_BYTES: usize = 32 * 1024;

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
            exchange_capture_budget_bytes: config
                .max_pending_bytes
                .min(DEFAULT_EXCHANGE_CAPTURE_BUDGET_BYTES),
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

    pub fn snapshot(&self) -> HookSnapshot {
        let breaker = self.runtime.breaker.lock().unwrap();
        HookSnapshot {
            pending_bytes: self.runtime.pending_bytes.load(Ordering::Relaxed),
            max_pending_bytes: self.runtime.max_pending_bytes,
            failure_threshold: self.runtime.failure_threshold,
            exchange: CircuitSnapshot::from_state(&breaker.exchange),
            usage: CircuitSnapshot::from_state(&breaker.usage),
        }
    }

    pub fn emit_non_stream(
        &self,
        ctx: HookRequestContext,
        status: u16,
        response_headers: Vec<HeaderEntry>,
        response_body: Value,
    ) {
        let usage = NormalizedUsage::from_client_body(ctx.client_format, &response_body);
        let observation = StreamObservation::non_stream_completed();
        self.emit_exchange(
            &ctx,
            status,
            response_headers.clone(),
            ExchangeResponseCapture::Immediate(response_body.clone()),
            observation.clone(),
        );
        self.emit_usage(&ctx, status, usage, observation);
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
        let exchange_capture = if self.exchange.is_some() && self.runtime.can_capture_exchange() {
            match EventSpoolSink::new(
                ctx.client_format,
                self.runtime.exchange_capture_budget_bytes,
            ) {
                Ok(sink) => Some(ExchangeCaptureMode::Spooling(sink)),
                Err(reason) => Some(ExchangeCaptureMode::Unavailable { reason }),
            }
        } else {
            None
        };
        let capture_enabled = matches!(exchange_capture, Some(ExchangeCaptureMode::Spooling(_)));
        let observe_events = capture_enabled || self.usage.is_some();
        HookCaptureStream {
            inner,
            buffer: Vec::new(),
            observer: ClientSseAccumulator::new(ctx.client_format),
            dispatcher: self.clone(),
            ctx,
            status,
            response_headers,
            finalized: false,
            capture_enabled,
            exchange_capture,
            observe_events,
        }
    }

    fn emit_exchange(
        &self,
        ctx: &HookRequestContext,
        status: u16,
        response_headers: Vec<HeaderEntry>,
        response_capture: ExchangeResponseCapture,
        observation: StreamObservation,
    ) {
        let Some(sender) = self.exchange.clone() else {
            return;
        };
        if !self.runtime.can_attempt(HookKind::Exchange) {
            return;
        }
        sender.spawn_exchange_send(ExchangeHookPayload {
            ctx: ctx.clone(),
            status,
            response_headers,
            response_capture,
            observation,
        });
    }

    fn emit_usage(
        &self,
        ctx: &HookRequestContext,
        status: u16,
        usage: NormalizedUsage,
        observation: StreamObservation,
    ) {
        let Some(sender) = self.usage.clone() else {
            return;
        };
        if !self.runtime.can_attempt(HookKind::Usage) {
            return;
        }
        let state = observation.project_legacy();
        let usage_status = observation.project_usage_status(status);
        let payload = json!({
            "request_id": ctx.request_id,
            "timestamp_ms": ctx.timestamp_ms,
            "path": ctx.path,
            "stream": ctx.stream,
            "completed": state.completed,
            "cancelled_by_client": state.cancelled_by_client,
            "partial": state.partial,
            "termination_reason": state.termination_reason,
            "status": usage_status,
            "http_status": status,
            "client_model": ctx.client_model,
            "upstream_name": ctx.upstream_name,
            "upstream_model": ctx.upstream_model,
            "client_format": ctx.client_format,
            "upstream_format": ctx.upstream_format,
            "credential_source": ctx.credential_source,
            "credential_fingerprint": ctx.credential_fingerprint,
            "transport_outcome": observation.transport_outcome,
            "protocol_terminal": observation.protocol_terminal,
            "usage": usage,
        });
        sender.spawn_send(payload);
    }
}

impl CircuitSnapshot {
    fn from_state(state: &CircuitState) -> Self {
        let now = Instant::now();
        let remaining_cooldown_secs = state
            .open_until
            .and_then(|deadline| deadline.checked_duration_since(now))
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        Self {
            consecutive_failures: state.consecutive_failures,
            open: remaining_cooldown_secs > 0,
            remaining_cooldown_secs,
        }
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

    fn spawn_exchange_send(self, payload: ExchangeHookPayload) {
        tokio::spawn(async move {
            let runtime = self.runtime.clone();
            let built = tokio::task::spawn_blocking(move || payload.into_json_payload()).await;
            let Ok(Ok(payload)) = built else {
                warn!("hook exchange payload dropped: reason=payload_build_failed");
                return;
            };
            let Ok(serialized) = serde_json::to_vec(&payload) else {
                return;
            };
            let payload_len = serialized.len();
            if !runtime.try_reserve(payload_len) {
                warn!(
                    "hook payload dropped: kind={:?} reason=max_pending_bytes_exceeded size={}",
                    self.kind, payload_len
                );
                return;
            }
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

impl ExchangeHookPayload {
    fn into_json_payload(self) -> Result<Value, String> {
        let legacy = self.observation.project_legacy();
        let mut response_body = match self.response_capture {
            ExchangeResponseCapture::Immediate(body) => body,
            ExchangeResponseCapture::Spool(artifact) => artifact.replay_final_response_body()?,
            ExchangeResponseCapture::Unavailable { reason } => json!({
                "capture_unavailable": true,
                "reason": reason,
            }),
        };
        strip_internal_replay_metadata(&mut response_body);
        Ok(json!({
            "request_id": self.ctx.request_id,
            "timestamp_ms": self.ctx.timestamp_ms,
            "path": self.ctx.path,
            "method": self.ctx.method,
            "stream": self.ctx.stream,
            "client_format": self.ctx.client_format,
            "upstream_format": self.ctx.upstream_format,
            "client_model": self.ctx.client_model,
            "upstream_name": self.ctx.upstream_name,
            "upstream_model": self.ctx.upstream_model,
            "credential_source": self.ctx.credential_source,
            "credential_fingerprint": self.ctx.credential_fingerprint,
            "completed": legacy.completed,
            "cancelled_by_client": legacy.cancelled_by_client,
            "partial": legacy.partial,
            "termination_reason": legacy.termination_reason,
            "transport_outcome": self.observation.transport_outcome,
            "protocol_terminal": self.observation.protocol_terminal,
            "request": {
                "headers": self.ctx.client_request_headers,
                "body": self.ctx.client_request_body,
            },
            "response": {
                "status": self.status,
                "headers": self.response_headers,
                "body": response_body,
            }
        }))
    }
}

impl EventSpoolSink {
    fn new(format: UpstreamFormat, budget_bytes: usize) -> Result<Self, String> {
        Self::new_with_options(format, budget_bytes, SpoolWriterOptions::default())
    }

    fn new_with_options(
        format: UpstreamFormat,
        budget_bytes: usize,
        options: SpoolWriterOptions,
    ) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "llm-proxy-hook-spool-{}-{}.jsonl",
            format!("{format:?}").to_lowercase(),
            Uuid::new_v4()
        ));
        Ok(Self {
            format,
            budget_bytes,
            captured_bytes: 0,
            dropped_event_count: 0,
            overflow_reason: None,
            writer: Some(SpoolWriterHandle::new(path, options)?),
        })
    }

    fn on_event(&mut self, event: &Value) -> Result<(), String> {
        if self.overflow_reason.is_some() {
            self.dropped_event_count = self.dropped_event_count.saturating_add(1);
            return Ok(());
        }
        let mut serialized =
            serde_json::to_vec(event).map_err(|err| format!("serialize spool event: {err}"))?;
        serialized.push(b'\n');
        let serialized_len = serialized.len();
        if self.captured_bytes.saturating_add(serialized_len) > self.budget_bytes {
            self.mark_overflow("capture_budget_exceeded");
            return Ok(());
        }
        match self
            .writer
            .as_mut()
            .and_then(|writer| writer.sender.as_ref())
            .ok_or_else(|| "spool writer unavailable".to_string())?
            .try_send(serialized)
        {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                self.mark_overflow("capture_queue_overflow");
                return Ok(());
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.mark_overflow("capture_writer_disconnected");
                return Ok(());
            }
        }
        self.captured_bytes = self.captured_bytes.saturating_add(serialized_len);
        Ok(())
    }

    fn finish(mut self) -> Result<EventSpoolArtifact, String> {
        let state = if let Some(reason) = self.overflow_reason.take() {
            EventSpoolArtifactState::Truncated {
                reason,
                capture_budget_bytes: self.budget_bytes,
                captured_bytes: self.captured_bytes,
                dropped_event_count: self.dropped_event_count,
                writer: self.writer.take(),
            }
        } else {
            EventSpoolArtifactState::Complete {
                writer: self
                    .writer
                    .take()
                    .ok_or_else(|| "missing spool writer".to_string())?,
            }
        };
        Ok(EventSpoolArtifact {
            format: self.format,
            state: Some(state),
        })
    }

    fn mark_overflow(&mut self, reason: &str) {
        if self.overflow_reason.is_none() {
            self.overflow_reason = Some(reason.to_string());
            if let Some(writer) = self.writer.as_mut() {
                writer.close_sender();
            }
        }
        self.dropped_event_count = self.dropped_event_count.saturating_add(1);
    }
}

impl EventSpoolArtifact {
    fn replay_final_response_body(mut self) -> Result<Value, String> {
        match self.state.take() {
            Some(EventSpoolArtifactState::Complete { writer }) => {
                let path = writer.finish()?;
                let file = File::open(&path)
                    .map_err(|err| format!("open spool artifact for replay: {err}"))?;
                let reader = BufReader::new(file);
                let mut accumulator = ClientSseAccumulator::new(self.format);
                for line in reader.lines() {
                    let line = line.map_err(|err| format!("read spool artifact line: {err}"))?;
                    if line.trim().is_empty() {
                        continue;
                    }
                    let event: Value = serde_json::from_str(&line)
                        .map_err(|err| format!("parse spool event: {err}"))?;
                    accumulator.on_event(&event, true);
                }
                let _ = fs::remove_file(&path);
                Ok(accumulator.final_response_body())
            }
            Some(EventSpoolArtifactState::Truncated {
                reason,
                capture_budget_bytes,
                captured_bytes,
                dropped_event_count,
                writer,
            }) => Ok(json!({
                "capture_truncated": true,
                "reason": reason,
                "client_format": self.format,
                "capture_budget_bytes": capture_budget_bytes,
                "captured_bytes": captured_bytes,
                "dropped_event_count": dropped_event_count,
                "writer_completed": writer.map(|writer| writer.cleanup()).unwrap_or(true),
            })),
            None => Err("missing spool artifact state".to_string()),
        }
    }
}

impl Drop for EventSpoolArtifact {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            match state {
                EventSpoolArtifactState::Complete { writer } => {
                    let _ = writer.cleanup();
                }
                EventSpoolArtifactState::Truncated {
                    writer: Some(writer),
                    ..
                } => {
                    let _ = writer.cleanup();
                }
                EventSpoolArtifactState::Truncated { writer: None, .. } => {}
            }
        }
    }
}

impl SpoolWriterHandle {
    fn new(path: PathBuf, options: SpoolWriterOptions) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|err| format!("open spool file: {err}"))?;
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(options.queue_capacity);
        let error = Arc::new(Mutex::new(None));
        let worker_error = error.clone();
        #[cfg(test)]
        let start_barrier = options.start_barrier.clone();
        let join = std::thread::Builder::new()
            .name("hook-exchange-spool".to_string())
            .spawn(move || {
                let mut writer = BufWriter::new(file);
                #[cfg(test)]
                if let Some(barrier) = start_barrier {
                    barrier.wait();
                }
                while let Ok(line) = receiver.recv() {
                    if let Err(err) = writer.write_all(&line) {
                        *worker_error.lock().unwrap() = Some(format!("write spool event: {err}"));
                        break;
                    }
                }
                let _ = writer.flush();
            })
            .map_err(|err| format!("spawn spool writer: {err}"))?;
        Ok(Self {
            sender: Some(sender),
            join: Some(join),
            path: Some(path),
            error,
        })
    }

    fn close_sender(&mut self) {
        self.sender.take();
    }

    fn finish(mut self) -> Result<PathBuf, String> {
        self.close_sender();
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| "join spool writer thread".to_string())?;
        }
        if let Some(err) = self.error.lock().unwrap().clone() {
            return Err(err);
        }
        self.path
            .take()
            .ok_or_else(|| "missing spool path".to_string())
    }

    fn cleanup(mut self) -> bool {
        self.close_sender();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
        self.error.lock().unwrap().is_none()
    }
}

impl Drop for SpoolWriterHandle {
    fn drop(&mut self) {
        self.close_sender();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
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
    observer: ClientSseAccumulator,
    dispatcher: HookDispatcher,
    ctx: HookRequestContext,
    status: u16,
    response_headers: Vec<HeaderEntry>,
    finalized: bool,
    capture_enabled: bool,
    exchange_capture: Option<ExchangeCaptureMode>,
    observe_events: bool,
}

impl<S> HookCaptureStream<S> {
    fn finalize(&mut self, transport_outcome: TransportOutcome) {
        if self.finalized {
            return;
        }
        self.finalized = true;
        let observation = StreamObservation {
            transport_outcome,
            protocol_terminal: self.observer.protocol_terminal(),
        };
        let usage = self.observer.final_usage();
        if let Some(capture) = self.exchange_capture.take() {
            let response_capture = match capture {
                ExchangeCaptureMode::Spooling(sink) => match sink.finish() {
                    Ok(artifact) => ExchangeResponseCapture::Spool(artifact),
                    Err(reason) => ExchangeResponseCapture::Unavailable { reason },
                },
                ExchangeCaptureMode::Unavailable { reason } => {
                    ExchangeResponseCapture::Unavailable { reason }
                }
            };
            self.dispatcher.emit_exchange(
                &self.ctx,
                self.status,
                self.response_headers.clone(),
                response_capture,
                observation.clone(),
            );
        }
        self.dispatcher
            .emit_usage(&self.ctx, self.status, usage, observation);
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
                if this.observe_events {
                    this.buffer.extend_from_slice(&bytes);
                    while let Some(event) = take_one_sse_event(&mut this.buffer) {
                        this.observer.on_event(&event, false);
                        if let Some(ExchangeCaptureMode::Spooling(sink)) =
                            this.exchange_capture.as_mut()
                        {
                            if let Err(err) = sink.on_event(&event) {
                                this.exchange_capture =
                                    Some(ExchangeCaptureMode::Unavailable { reason: err });
                                this.capture_enabled = false;
                            }
                        }
                    }
                }
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(err))) => {
                this.finalize(TransportOutcome::StreamError);
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                this.finalize(TransportOutcome::CompletedEof);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Drop for HookCaptureStream<S> {
    fn drop(&mut self) {
        self.finalize(TransportOutcome::ClientDisconnected);
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

    fn on_event(&mut self, event: &Value, capture_exchange: bool) {
        match self {
            Self::OpenAiCompletion(acc) => acc.on_event(event, capture_exchange),
            Self::Responses(acc) => acc.on_event(event, capture_exchange),
            Self::Anthropic(acc) => acc.on_event(event, capture_exchange),
            Self::Google(acc) => acc.on_event(event, capture_exchange),
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

    fn protocol_terminal(&self) -> Option<ProtocolTerminal> {
        match self {
            Self::OpenAiCompletion(acc) => acc.protocol_terminal(),
            Self::Responses(acc) => acc.protocol_terminal(),
            Self::Anthropic(acc) => acc.protocol_terminal(),
            Self::Google(acc) => acc.protocol_terminal(),
        }
    }
}

#[derive(Debug, Default)]
struct OpenAiToolCallAccumulator {
    id: Option<String>,
    name: String,
    arguments: String,
}

const INTERNAL_NON_REPLAYABLE_TOOL_CALL_FIELD: &str = "_llmup_non_replayable_tool_call";
const INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON: &str = "incomplete_arguments";
const INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION: u64 = 1;
const INTERNAL_NON_REPLAYABLE_TOOL_CALL_SIGNATURE_FIELD: &str = "sig";
const INTERNAL_REPLAY_MARKER_KEY_ENV: &str = "LLMUP_INTERNAL_REPLAY_MARKER_KEY";

fn internal_replay_marker_key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Some(existing) = std::env::var(INTERNAL_REPLAY_MARKER_KEY_ENV)
            .ok()
            .filter(|value| !value.is_empty())
        {
            return existing;
        }
        let generated = Uuid::new_v4().to_string();
        std::env::set_var(INTERNAL_REPLAY_MARKER_KEY_ENV, &generated);
        generated
    })
}

fn raw_json_is_valid_object(raw: &str) -> bool {
    !raw.trim().is_empty()
        && serde_json::from_str::<Value>(raw).is_ok_and(|value| value.is_object())
}

fn non_replayable_tool_call_signature(name: &str, raw: &str) -> String {
    let payload = json!({
        "v": INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION,
        "reason": INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON,
        "name": name,
        "raw": raw
    });
    let encoded = serde_json::to_vec(&payload).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(internal_replay_marker_key().as_bytes());
    hasher.update([0]);
    hasher.update(encoded);
    hex::encode(hasher.finalize())
}

fn mark_tool_call_as_non_replayable(value: &mut Value) {
    let name = value
        .get("function")
        .and_then(|function| function.get("name"))
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let raw = value
        .get("function")
        .and_then(|function| function.get("arguments"))
        .or_else(|| value.get("arguments"))
        .or_else(|| value.get("input"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    obj.insert(
        INTERNAL_NON_REPLAYABLE_TOOL_CALL_FIELD.to_string(),
        json!({
            "reason": INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON,
            "v": INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION,
            INTERNAL_NON_REPLAYABLE_TOOL_CALL_SIGNATURE_FIELD: non_replayable_tool_call_signature(&name, &raw)
        }),
    );
}

fn openai_tool_call_has_invalid_structured_arguments(tool_call: &Value) -> bool {
    tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .and_then(Value::as_str)
        .map(|raw| !raw.trim().is_empty() && !raw_json_is_valid_object(raw))
        .unwrap_or(false)
}

fn responses_tool_call_has_invalid_structured_arguments(item: &Value) -> bool {
    item.get("arguments")
        .or_else(|| item.get("input"))
        .and_then(Value::as_str)
        .map(|raw| !raw.trim().is_empty() && !raw_json_is_valid_object(raw))
        .unwrap_or(false)
}

fn finish_reason_allows_partial_tool_calls(finish_reason: Option<&str>) -> bool {
    matches!(finish_reason, Some("length") | Some("pause_turn"))
}

fn responses_status_allows_partial_tool_calls(response: &Value) -> bool {
    response.get("status").and_then(Value::as_str) == Some("incomplete")
        && matches!(
            response
                .get("incomplete_details")
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str),
            Some("max_output_tokens") | Some("pause_turn")
        )
}

fn strip_internal_replay_metadata(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                strip_internal_replay_metadata(item);
            }
        }
        Value::Object(object) => {
            object.remove(INTERNAL_NON_REPLAYABLE_TOOL_CALL_FIELD);
            for item in object.values_mut() {
                strip_internal_replay_metadata(item);
            }
        }
        _ => {}
    }
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
    protocol_terminal: Option<ProtocolTerminal>,
}

impl OpenAiCompletionAccumulator {
    fn on_event(&mut self, event: &Value, capture_exchange: bool) {
        if event.get("_done").and_then(Value::as_bool) == Some(true) {
            self.protocol_terminal
                .get_or_insert_with(|| ProtocolTerminal {
                    kind: ProtocolTerminalKind::Success,
                    event_type: "done".to_string(),
                    finish_reason: Some("stop".to_string()),
                    incomplete_reason: None,
                    error: None,
                });
            return;
        }
        if let Some(terminal) = openai_protocol_terminal(event) {
            self.protocol_terminal = Some(terminal);
        }
        if capture_exchange {
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
        }
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
        if !capture_exchange {
            return;
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
            if finish_reason_allows_partial_tool_calls(self.finish_reason.as_deref()) {
                if let Some(tool_calls) =
                    message.get_mut("tool_calls").and_then(Value::as_array_mut)
                {
                    for tool_call in tool_calls {
                        if openai_tool_call_has_invalid_structured_arguments(tool_call) {
                            mark_tool_call_as_non_replayable(tool_call);
                        }
                    }
                }
            }
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

    fn protocol_terminal(&self) -> Option<ProtocolTerminal> {
        self.protocol_terminal.clone()
    }
}

#[derive(Debug, Default)]
struct ResponsesAccumulator {
    final_response: Option<Value>,
    last_usage: Option<NormalizedUsage>,
    protocol_terminal: Option<ProtocolTerminal>,
}

impl ResponsesAccumulator {
    fn on_event(&mut self, event: &Value, capture_exchange: bool) {
        if let Some(terminal) = responses_protocol_terminal(event) {
            self.protocol_terminal = Some(terminal);
        }
        if let Some(response) = event.get("response") {
            let terminal = matches!(
                event.get("type").and_then(Value::as_str),
                Some("response.completed" | "response.incomplete" | "response.failed")
            );
            if terminal {
                if capture_exchange {
                    self.final_response = Some(response.clone());
                }
                self.last_usage = Some(NormalizedUsage::from_client_body(
                    UpstreamFormat::OpenAiResponses,
                    response,
                ));
            }
        }
    }

    fn final_body(&self) -> Value {
        let mut response = self.final_response.clone().unwrap_or_else(|| json!({}));
        if responses_status_allows_partial_tool_calls(&response) {
            if let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) {
                for item in output {
                    if matches!(
                        item.get("type").and_then(Value::as_str),
                        Some("function_call") | Some("custom_tool_call")
                    ) && responses_tool_call_has_invalid_structured_arguments(item)
                    {
                        mark_tool_call_as_non_replayable(item);
                    }
                }
            }
        }
        response
    }

    fn final_usage(&self) -> NormalizedUsage {
        self.last_usage.clone().unwrap_or_default()
    }

    fn protocol_terminal(&self) -> Option<ProtocolTerminal> {
        self.protocol_terminal.clone()
    }
}

#[derive(Debug, Default)]
struct AnthropicAccumulator {
    message: Value,
    error: Option<Value>,
    usage: Option<NormalizedUsage>,
    last_stop_reason: Option<String>,
    protocol_terminal: Option<ProtocolTerminal>,
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

    fn on_event(&mut self, event: &Value, capture_exchange: bool) {
        match event.get("type").and_then(Value::as_str) {
            Some("error") => {
                if capture_exchange {
                    self.error = Some(event.clone());
                }
            }
            Some("message_start") => {
                if !capture_exchange {
                    return;
                }
                if let Some(message) = event.get("message") {
                    self.message = message.clone();
                    self.message["content"] = Value::Array(Vec::new());
                }
            }
            Some("content_block_start") => {
                if !capture_exchange {
                    return;
                }
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
                if !capture_exchange {
                    return;
                }
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
                    self.last_stop_reason = Some(stop_reason.to_string());
                    if !capture_exchange {
                        return;
                    }
                    self.message["stop_reason"] = json!(stop_reason);
                }
                if let Some(usage) = event.get("usage") {
                    if capture_exchange {
                        self.message["usage"] = usage.clone();
                    }
                    self.usage = Some(NormalizedUsage::from_client_body(
                        UpstreamFormat::Anthropic,
                        &json!({ "usage": usage }),
                    ));
                }
            }
            Some("message_stop") => {
                if !capture_exchange {
                    if let Some(terminal) =
                        anthropic_protocol_terminal(event, self.last_stop_reason.as_deref())
                    {
                        self.protocol_terminal = Some(terminal);
                    }
                    return;
                }
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
        if let Some(terminal) = anthropic_protocol_terminal(event, self.last_stop_reason.as_deref())
        {
            self.protocol_terminal = Some(terminal);
        }
    }

    fn final_body(&self) -> Value {
        self.error.clone().unwrap_or_else(|| self.message.clone())
    }

    fn final_usage(&self) -> NormalizedUsage {
        self.usage.clone().unwrap_or_default()
    }

    fn protocol_terminal(&self) -> Option<ProtocolTerminal> {
        self.protocol_terminal.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use futures_util::task::noop_waker_ref;
    use serde_json::json;
    use std::pin::Pin;
    use std::task::Context;
    use std::task::Poll;

    fn runtime(
        max_pending_bytes: usize,
        failure_threshold: usize,
        cooldown_ms: u64,
    ) -> HookRuntime {
        HookRuntime {
            max_pending_bytes,
            exchange_capture_budget_bytes: max_pending_bytes
                .min(DEFAULT_EXCHANGE_CAPTURE_BUDGET_BYTES),
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

    #[test]
    fn hook_capture_stream_drop_marks_cancelled_partial() {
        let runtime = Arc::new(runtime(1024, 3, 100));
        let dispatcher = HookDispatcher {
            exchange: None,
            usage: None,
            runtime,
        };
        let ctx = HookRequestContext {
            request_id: "req_1".to_string(),
            timestamp_ms: 1,
            path: "/openai/v1/responses".to_string(),
            method: "POST".to_string(),
            stream: true,
            client_format: UpstreamFormat::OpenAiCompletion,
            upstream_format: UpstreamFormat::OpenAiCompletion,
            client_model: "gpt-4".to_string(),
            upstream_name: "default".to_string(),
            upstream_model: "gpt-4".to_string(),
            credential_source: CredentialSource::Server,
            credential_fingerprint: Some("abc".to_string()),
            client_request_headers: vec![],
            client_request_body: json!({"model":"gpt-4"}),
        };
        let stream = HookCaptureStream {
            inner: stream::pending::<Result<Bytes, std::io::Error>>(),
            buffer: Vec::new(),
            observer: ClientSseAccumulator::new(UpstreamFormat::OpenAiCompletion),
            exchange_capture: None,
            dispatcher,
            ctx,
            status: 200,
            response_headers: json_response_headers(),
            finalized: false,
            capture_enabled: true,
            observe_events: true,
        };

        assert!(!stream.finalized);
        drop(stream);
    }

    #[test]
    fn hook_capture_stream_tracks_usage_even_when_exchange_capture_disabled() {
        let runtime = Arc::new(runtime(1, 3, 100));
        runtime.pending_bytes.store(1, Ordering::Relaxed);
        let dispatcher = HookDispatcher {
            exchange: None,
            usage: Some(HookSender {
                client: reqwest::Client::new(),
                config: HookEndpointConfig {
                    url: "http://127.0.0.1:9/usage".to_string(),
                    authorization: None,
                },
                kind: HookKind::Usage,
                runtime: runtime.clone(),
            }),
            runtime,
        };
        let ctx = HookRequestContext {
            request_id: "req_usage".to_string(),
            timestamp_ms: 1,
            path: "/openai/v1/responses".to_string(),
            method: "POST".to_string(),
            stream: true,
            client_format: UpstreamFormat::OpenAiResponses,
            upstream_format: UpstreamFormat::OpenAiResponses,
            client_model: "gpt-4.1".to_string(),
            upstream_name: "default".to_string(),
            upstream_model: "gpt-4.1".to_string(),
            credential_source: CredentialSource::Server,
            credential_fingerprint: Some("abc".to_string()),
            client_request_headers: vec![],
            client_request_body: json!({"model":"gpt-4.1","stream":true}),
        };
        let payload = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":4,\"total_tokens\":7}}}\n\n"
        );
        let inner = stream::iter(vec![Ok(Bytes::from_static(payload.as_bytes()))]);
        let mut stream = dispatcher.wrap_stream(inner, ctx, 200, sse_response_headers());

        assert!(!stream.capture_enabled);
        assert_eq!(stream.observer.final_usage().total_tokens, None);

        let waker = noop_waker_ref();
        let mut cx = Context::from_waker(waker);
        let first = Pin::new(&mut stream).poll_next(&mut cx);
        assert!(matches!(first, Poll::Ready(Some(Ok(_)))));
        assert_eq!(stream.observer.final_usage().input_tokens, Some(3));
        assert_eq!(stream.observer.final_usage().output_tokens, Some(4));
        assert_eq!(stream.observer.final_usage().total_tokens, Some(7));
    }

    #[test]
    fn success_terminal_projects_completed_legacy_state() {
        let legacy = StreamObservation {
            transport_outcome: TransportOutcome::ClientDisconnected,
            protocol_terminal: Some(ProtocolTerminal {
                kind: ProtocolTerminalKind::Success,
                event_type: "response.completed".to_string(),
                finish_reason: Some("stop".to_string()),
                incomplete_reason: None,
                error: None,
            }),
        }
        .project_legacy();

        assert!(legacy.completed);
        assert!(!legacy.cancelled_by_client);
        assert!(!legacy.partial);
        assert!(matches!(
            legacy.termination_reason,
            TerminationReason::Completed
        ));
    }

    #[test]
    fn failed_terminal_does_not_project_completed_legacy_state() {
        let legacy = StreamObservation {
            transport_outcome: TransportOutcome::ClientDisconnected,
            protocol_terminal: Some(ProtocolTerminal {
                kind: ProtocolTerminalKind::Failed,
                event_type: "response.failed".to_string(),
                finish_reason: None,
                incomplete_reason: None,
                error: Some(json!({"code":"context_length_exceeded"})),
            }),
        }
        .project_legacy();

        assert!(!legacy.completed);
        assert!(!legacy.cancelled_by_client);
        assert!(!legacy.partial);
        assert!(matches!(
            legacy.termination_reason,
            TerminationReason::Failed
        ));
    }

    #[test]
    fn incomplete_terminal_does_not_project_completed_legacy_state() {
        let legacy = StreamObservation {
            transport_outcome: TransportOutcome::ClientDisconnected,
            protocol_terminal: Some(ProtocolTerminal {
                kind: ProtocolTerminalKind::Incomplete,
                event_type: "response.incomplete".to_string(),
                finish_reason: None,
                incomplete_reason: Some("max_output_tokens".to_string()),
                error: None,
            }),
        }
        .project_legacy();

        assert!(!legacy.completed);
        assert!(!legacy.cancelled_by_client);
        assert!(legacy.partial);
        assert!(matches!(
            legacy.termination_reason,
            TerminationReason::Incomplete
        ));
    }

    #[test]
    fn usage_status_projects_protocol_terminal_over_http_success() {
        let failed = StreamObservation {
            transport_outcome: TransportOutcome::CompletedEof,
            protocol_terminal: Some(ProtocolTerminal {
                kind: ProtocolTerminalKind::Failed,
                event_type: "response.failed".to_string(),
                finish_reason: None,
                incomplete_reason: None,
                error: Some(json!({"code":"tool_error"})),
            }),
        };
        let incomplete = StreamObservation {
            transport_outcome: TransportOutcome::CompletedEof,
            protocol_terminal: Some(ProtocolTerminal {
                kind: ProtocolTerminalKind::Incomplete,
                event_type: "response.incomplete".to_string(),
                finish_reason: None,
                incomplete_reason: Some("max_output_tokens".to_string()),
                error: None,
            }),
        };

        assert_eq!(failed.project_usage_status(200), UsageHookStatus::Error);
        assert_eq!(
            incomplete.project_usage_status(200),
            UsageHookStatus::Incomplete
        );
    }

    #[test]
    fn event_spool_capture_replays_full_openai_response_body() {
        let mut sink = EventSpoolSink::new(UpstreamFormat::OpenAiCompletion, 4096).expect("spool");
        sink.on_event(&json!({
            "id": "chatcmpl_1",
            "created": 7,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "delta": { "role": "assistant", "content": "hello " }
            }]
        }))
        .expect("write first");
        sink.on_event(&json!({
            "id": "chatcmpl_1",
            "created": 7,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "delta": { "content": "world" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5 }
        }))
        .expect("write second");

        let artifact = sink.finish().expect("artifact");
        let body = artifact.replay_final_response_body().expect("replay");

        assert_eq!(body["choices"][0]["message"]["content"], "hello world");
        assert_eq!(body["usage"]["total_tokens"], 5);
    }

    #[test]
    fn event_spool_capture_marks_incomplete_openai_replayable_tool_calls() {
        let mut sink = EventSpoolSink::new(UpstreamFormat::OpenAiCompletion, 4096).expect("spool");
        sink.on_event(&json!({
            "id": "chatcmpl_1",
            "created": 7,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "lookup_weather",
                            "arguments": "{\"city\":\"Tok"
                        }
                    }]
                }
            }]
        }))
        .expect("write first");
        sink.on_event(&json!({
            "id": "chatcmpl_1",
            "created": 7,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "length"
            }]
        }))
        .expect("write second");

        let artifact = sink.finish().expect("artifact");
        let body = artifact.replay_final_response_body().expect("replay");
        let tool_calls = body["choices"][0]["message"]["tool_calls"]
            .as_array()
            .expect("tool calls");

        assert_eq!(
            tool_calls[0]["_llmup_non_replayable_tool_call"]["reason"],
            "incomplete_arguments"
        );
        assert!(tool_calls[0]["_llmup_non_replayable_tool_call"]["sig"].is_string());
    }

    #[test]
    fn event_spool_capture_marks_incomplete_responses_replayable_tool_calls() {
        let mut sink = EventSpoolSink::new(UpstreamFormat::OpenAiResponses, 4096).expect("spool");
        sink.on_event(&json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_1",
                "object": "response",
                "status": "incomplete",
                "incomplete_details": { "reason": "max_output_tokens" },
                "output": [{
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup_weather",
                    "arguments": "{\"city\":\"Tok"
                }]
            }
        }))
        .expect("write");

        let artifact = sink.finish().expect("artifact");
        let body = artifact.replay_final_response_body().expect("replay");
        let output = body["output"].as_array().expect("output");

        assert_eq!(
            output[0]["_llmup_non_replayable_tool_call"]["reason"],
            "incomplete_arguments"
        );
        assert!(output[0]["_llmup_non_replayable_tool_call"]["sig"].is_string());
    }

    #[test]
    fn exchange_hook_payload_strips_internal_non_replayable_tool_call_metadata() {
        let mut sink = EventSpoolSink::new(UpstreamFormat::OpenAiCompletion, 4096).expect("spool");
        sink.on_event(&json!({
            "id": "chatcmpl_1",
            "created": 7,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "lookup_weather",
                            "arguments": "{\"city\":\"Tok"
                        }
                    }]
                }
            }]
        }))
        .expect("write first");
        sink.on_event(&json!({
            "id": "chatcmpl_1",
            "created": 7,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "length"
            }]
        }))
        .expect("write second");

        let payload = ExchangeHookPayload {
            ctx: HookRequestContext {
                request_id: "req_exchange".to_string(),
                timestamp_ms: 7,
                path: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
                stream: true,
                client_format: UpstreamFormat::OpenAiCompletion,
                upstream_format: UpstreamFormat::OpenAiCompletion,
                client_model: "gpt-4.1".to_string(),
                upstream_name: "default".to_string(),
                upstream_model: "gpt-4.1".to_string(),
                credential_source: CredentialSource::Server,
                credential_fingerprint: Some("fp".to_string()),
                client_request_headers: vec![],
                client_request_body: json!({"model":"gpt-4.1","stream":true}),
            },
            status: 200,
            response_headers: json_response_headers(),
            response_capture: ExchangeResponseCapture::Spool(sink.finish().expect("artifact")),
            observation: StreamObservation::non_stream_completed(),
        }
        .into_json_payload()
        .expect("exchange payload");

        assert!(
            payload["response"]["body"]["choices"][0]["message"]["tool_calls"][0]
                .get("_llmup_non_replayable_tool_call")
                .is_none(),
            "payload = {payload:?}"
        );
    }

    #[test]
    fn event_spool_capture_reports_truncation_when_budget_exceeded() {
        let mut sink = EventSpoolSink::new(UpstreamFormat::OpenAiCompletion, 64).expect("spool");
        sink.on_event(&json!({
            "id": "chatcmpl_1",
            "created": 7,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "delta": { "role": "assistant", "content": "hello" }
            }]
        }))
        .expect("write first");
        sink.on_event(&json!({
            "id": "chatcmpl_1",
            "created": 7,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "delta": { "content": "world" },
                "finish_reason": "stop"
            }]
        }))
        .expect("truncate");

        let artifact = sink.finish().expect("artifact");
        let body = artifact.replay_final_response_body().expect("replay");

        assert_eq!(body["capture_truncated"], true);
        assert_eq!(body["reason"], "capture_budget_exceeded");
        assert_eq!(body["capture_budget_bytes"], 64);
        assert!(body["captured_bytes"].as_u64().unwrap_or(0) <= 64);
        assert!(body["dropped_event_count"].as_u64().unwrap_or(0) >= 1);
    }

    #[test]
    fn event_spool_capture_reports_queue_overflow_without_replay() {
        let start_barrier = Arc::new(std::sync::Barrier::new(2));
        let mut sink = EventSpoolSink::new_with_options(
            UpstreamFormat::OpenAiCompletion,
            4096,
            SpoolWriterOptions {
                queue_capacity: 1,
                start_barrier: Some(start_barrier.clone()),
            },
        )
        .expect("spool");

        sink.on_event(&json!({
            "id": "chatcmpl_1",
            "created": 7,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "delta": { "role": "assistant", "content": "hello " }
            }]
        }))
        .expect("queue first");
        sink.on_event(&json!({
            "id": "chatcmpl_1",
            "created": 7,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "delta": { "content": "world" },
                "finish_reason": "stop"
            }]
        }))
        .expect("overflow to truncation");
        start_barrier.wait();

        let artifact = sink.finish().expect("artifact");
        let body = artifact.replay_final_response_body().expect("replay");

        assert_eq!(body["capture_truncated"], true);
        assert_eq!(body["reason"], "capture_queue_overflow");
        assert!(body["dropped_event_count"].as_u64().unwrap_or(0) >= 1);
        assert!(body.get("choices").is_none());
    }
}

#[derive(Debug, Default)]
struct GoogleAccumulator {
    response: Value,
    usage: Option<NormalizedUsage>,
    protocol_terminal: Option<ProtocolTerminal>,
}

impl GoogleAccumulator {
    fn on_event(&mut self, event: &Value, capture_exchange: bool) {
        if capture_exchange && self.response.is_null() {
            self.response = json!({
                "candidates": [{
                    "content": { "parts": [], "role": "model" }
                }]
            });
        }
        if let Some(candidates) = event.get("candidates").and_then(Value::as_array) {
            if let Some(candidate) = candidates.first() {
                if capture_exchange {
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
                if candidate.get("finishReason").is_some() {
                    self.protocol_terminal = google_protocol_terminal(event);
                }
            }
        }
        if capture_exchange {
            if let Some(model) = event.get("modelVersion") {
                self.response["modelVersion"] = model.clone();
            }
        }
        if let Some(usage) = event.get("usageMetadata") {
            if capture_exchange {
                self.response["usageMetadata"] = usage.clone();
            }
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

    fn protocol_terminal(&self) -> Option<ProtocolTerminal> {
        self.protocol_terminal.clone()
    }
}
