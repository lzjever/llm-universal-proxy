use std::fs::{create_dir_all, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::Stream;
use serde::Serialize;
use serde_json::{json, Value};
use tracing::warn;

use crate::config::DebugTraceConfig;
use crate::formats::UpstreamFormat;
use crate::streaming::take_one_sse_event;

#[derive(Clone)]
pub struct DebugTraceRecorder {
    writer: Arc<TraceWriter>,
    max_text_chars: usize,
}

struct TraceWriter {
    sender: mpsc::SyncSender<Value>,
    dropped_entries: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct TraceWriterOptions {
    queue_capacity: usize,
    #[cfg(test)]
    start_barrier: Option<Arc<std::sync::Barrier>>,
}

impl Default for TraceWriterOptions {
    fn default() -> Self {
        Self {
            queue_capacity: 1024,
            #[cfg(test)]
            start_barrier: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DebugTraceContext {
    pub request_id: String,
    pub timestamp_ms: u128,
    pub path: String,
    pub stream: bool,
    pub client_model: String,
    pub upstream_name: String,
    pub upstream_model: String,
    pub client_format: UpstreamFormat,
    pub upstream_format: UpstreamFormat,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TracePhase {
    Request,
    Response,
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

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TraceOutcome {
    Completed,
    Incomplete,
    Failed,
    ClientDisconnected,
    StreamError,
}

fn project_trace_outcome(
    transport_outcome: TransportOutcome,
    summary: &ResponseSummary,
) -> TraceOutcome {
    if let Some(terminal) = &summary.protocol_terminal {
        return match terminal.kind {
            ProtocolTerminalKind::Success => TraceOutcome::Completed,
            ProtocolTerminalKind::Incomplete => TraceOutcome::Incomplete,
            ProtocolTerminalKind::Failed => TraceOutcome::Failed,
        };
    }
    match transport_outcome {
        TransportOutcome::CompletedEof => TraceOutcome::Completed,
        TransportOutcome::ClientDisconnected => TraceOutcome::ClientDisconnected,
        TransportOutcome::StreamError => TraceOutcome::StreamError,
    }
}

#[derive(Default)]
struct BoundedTextSummary {
    head: String,
    stored_chars: usize,
    total_chars: usize,
    max_chars: usize,
}

impl BoundedTextSummary {
    fn new(max_chars: usize) -> Self {
        Self {
            head: String::new(),
            stored_chars: 0,
            total_chars: 0,
            max_chars,
        }
    }

    fn push_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let incoming_chars = text.chars().count();
        self.total_chars += incoming_chars;
        if self.stored_chars >= self.max_chars {
            return;
        }
        let remaining = self.max_chars.saturating_sub(self.stored_chars);
        for ch in text.chars().take(remaining) {
            self.head.push(ch);
            self.stored_chars += 1;
        }
    }

    fn render(&self) -> String {
        if self.total_chars <= self.max_chars {
            self.head.clone()
        } else {
            format!("{}…[{} chars]", self.head, self.total_chars)
        }
    }
}

#[derive(Default)]
struct BoundedToolCallSummary {
    items: Vec<Value>,
    stored_chars: usize,
    omitted_items: usize,
    max_chars: usize,
}

impl BoundedToolCallSummary {
    fn new(max_chars: usize) -> Self {
        Self {
            items: Vec::new(),
            stored_chars: 0,
            omitted_items: 0,
            max_chars,
        }
    }

    fn push(&mut self, value: Value) {
        let value = truncate_value_strings(&value, self.max_chars);
        let serialized_len = serde_json::to_string(&value)
            .map(|text| text.chars().count())
            .unwrap_or(self.max_chars.saturating_add(1));
        let budget = self.max_chars.max(1);
        if self.stored_chars.saturating_add(serialized_len) > budget {
            self.omitted_items = self.omitted_items.saturating_add(1);
            return;
        }
        self.stored_chars += serialized_len;
        self.items.push(value);
    }

    fn render(&self) -> Vec<Value> {
        let mut out = self.items.clone();
        if self.omitted_items > 0 {
            out.push(json!({
                "type": "truncated",
                "omitted_items": self.omitted_items,
            }));
        }
        out
    }
}

struct ResponseSummary {
    text: BoundedTextSummary,
    reasoning: BoundedTextSummary,
    tool_calls: BoundedToolCallSummary,
    protocol_terminal: Option<ProtocolTerminal>,
    terminal_event: Option<String>,
    finish_reason: Option<String>,
    error: Option<Value>,
}

impl ResponseSummary {
    fn new(max_text_chars: usize) -> Self {
        Self {
            text: BoundedTextSummary::new(max_text_chars),
            reasoning: BoundedTextSummary::new(max_text_chars),
            tool_calls: BoundedToolCallSummary::new(max_text_chars),
            protocol_terminal: None,
            terminal_event: None,
            finish_reason: None,
            error: None,
        }
    }

    fn text_for_record(&self) -> String {
        self.text.render()
    }

    fn reasoning_for_record(&self) -> String {
        self.reasoning.render()
    }

    fn tool_calls_for_record(&self) -> Vec<Value> {
        self.tool_calls.render()
    }

    fn terminal_event_for_record(&self) -> Option<&str> {
        self.terminal_event.as_deref().or_else(|| {
            self.protocol_terminal
                .as_ref()
                .map(|terminal| terminal.event_type.as_str())
        })
    }

    fn finish_reason_for_record(&self) -> Option<&str> {
        self.finish_reason.as_deref().or_else(|| {
            self.protocol_terminal.as_ref().and_then(|terminal| {
                terminal
                    .finish_reason
                    .as_deref()
                    .or(terminal.incomplete_reason.as_deref())
            })
        })
    }

    fn error_for_record(&self) -> Option<&Value> {
        self.error.as_ref().or_else(|| {
            self.protocol_terminal
                .as_ref()
                .and_then(|terminal| terminal.error.as_ref())
        })
    }
}

pub struct DebugTraceStream<S> {
    inner: S,
    recorder: DebugTraceRecorder,
    ctx: DebugTraceContext,
    status: u16,
    buffer: Vec<u8>,
    summary: ResponseSummary,
    finalized: bool,
}

impl DebugTraceRecorder {
    pub fn new(config: &DebugTraceConfig) -> Option<Self> {
        Self::new_with_options(config, TraceWriterOptions::default())
    }

    fn new_with_options(config: &DebugTraceConfig, options: TraceWriterOptions) -> Option<Self> {
        let path = config.path.as_ref()?;
        let parent = Path::new(path).parent();
        if let Some(parent) = parent {
            if !parent.as_os_str().is_empty() && create_dir_all(parent).is_err() {
                return None;
            }
        }
        let (sender, receiver) = mpsc::sync_channel(options.queue_capacity);
        let dropped_entries = Arc::new(AtomicUsize::new(0));
        let worker_dropped_entries = dropped_entries.clone();
        let path = path.to_string();
        #[cfg(test)]
        let start_barrier = options.start_barrier.clone();
        std::thread::Builder::new()
            .name("debug-trace-writer".to_string())
            .spawn(move || {
                #[cfg(test)]
                if let Some(barrier) = start_barrier {
                    barrier.wait();
                }
                'outer: while let Ok(first) = receiver.recv() {
                    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
                        continue;
                    };
                    let mut writer = BufWriter::new(file);
                    let mut pending = Some(first);
                    loop {
                        if let Some(value) = pending.take() {
                            flush_trace_overflow_summary(&mut writer, &worker_dropped_entries);
                            let Ok(line) = serde_json::to_vec(&value) else {
                                continue;
                            };
                            let _ = writer.write_all(&line);
                            let _ = writer.write_all(b"\n");
                        }
                        while let Ok(value) = receiver.try_recv() {
                            flush_trace_overflow_summary(&mut writer, &worker_dropped_entries);
                            let Ok(line) = serde_json::to_vec(&value) else {
                                continue;
                            };
                            let _ = writer.write_all(&line);
                            let _ = writer.write_all(b"\n");
                        }
                        match receiver.recv_timeout(std::time::Duration::from_millis(25)) {
                            Ok(value) => {
                                pending = Some(value);
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                flush_trace_overflow_summary(&mut writer, &worker_dropped_entries);
                                let _ = writer.flush();
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                flush_trace_overflow_summary(&mut writer, &worker_dropped_entries);
                                let _ = writer.flush();
                                break 'outer;
                            }
                        }
                    }
                }
            })
            .ok()?;
        Some(Self {
            writer: Arc::new(TraceWriter {
                sender,
                dropped_entries,
            }),
            max_text_chars: config.max_text_chars,
        })
    }

    pub fn record_request(&self, ctx: &DebugTraceContext, body: &Value) {
        self.write_entry(json!({
            "timestamp_ms": ctx.timestamp_ms,
            "request_id": ctx.request_id,
            "phase": TracePhase::Request,
            "path": ctx.path,
            "stream": ctx.stream,
            "client_format": ctx.client_format,
            "upstream_format": ctx.upstream_format,
            "client_model": ctx.client_model,
            "upstream_name": ctx.upstream_name,
            "upstream_model": ctx.upstream_model,
            "request": {
                "new_items": extract_request_delta(ctx.client_format, body, self.max_text_chars)
            }
        }));
    }

    pub fn record_request_with_upstream(
        &self,
        ctx: &DebugTraceContext,
        original_body: &Value,
        upstream_body: &Value,
    ) {
        self.write_entry(json!({
            "timestamp_ms": ctx.timestamp_ms,
            "request_id": ctx.request_id,
            "phase": TracePhase::Request,
            "path": ctx.path,
            "stream": ctx.stream,
            "client_format": ctx.client_format,
            "upstream_format": ctx.upstream_format,
            "client_model": ctx.client_model,
            "upstream_name": ctx.upstream_name,
            "upstream_model": ctx.upstream_model,
            "request": {
                "new_items": extract_request_delta(ctx.client_format, original_body, self.max_text_chars),
                "client_summary": summarize_request_body(ctx.client_format, original_body, self.max_text_chars),
                "upstream_summary": summarize_request_body(ctx.upstream_format, upstream_body, self.max_text_chars)
            }
        }));
    }

    pub fn record_non_stream_response(&self, ctx: &DebugTraceContext, status: u16, body: &Value) {
        self.write_entry(json!({
            "timestamp_ms": ctx.timestamp_ms,
            "request_id": ctx.request_id,
            "phase": TracePhase::Response,
            "path": ctx.path,
            "stream": false,
            "client_format": ctx.client_format,
            "upstream_format": ctx.upstream_format,
            "client_model": ctx.client_model,
            "upstream_name": ctx.upstream_name,
            "upstream_model": ctx.upstream_model,
            "response": summarize_non_stream_response(ctx.client_format, body, self.max_text_chars),
            "http_status": status,
            "outcome": TraceOutcome::Completed,
            "transport_outcome": TransportOutcome::CompletedEof,
        }));
    }

    pub fn record_stream_start(&self, _ctx: &DebugTraceContext) {}

    pub fn wrap_stream<S>(
        &self,
        inner: S,
        ctx: DebugTraceContext,
        status: u16,
    ) -> DebugTraceStream<S>
    where
        S: Stream<Item = Result<Bytes, std::io::Error>>,
    {
        DebugTraceStream {
            inner,
            recorder: self.clone(),
            ctx,
            status,
            buffer: Vec::new(),
            summary: ResponseSummary::new(self.max_text_chars),
            finalized: false,
        }
    }

    fn record_stream_result(
        &self,
        ctx: &DebugTraceContext,
        status: u16,
        summary: &ResponseSummary,
        transport_outcome: TransportOutcome,
    ) {
        let outcome = project_trace_outcome(transport_outcome, summary);
        self.write_entry(json!({
            "timestamp_ms": ctx.timestamp_ms,
            "request_id": ctx.request_id,
            "phase": TracePhase::Response,
            "path": ctx.path,
            "stream": true,
            "client_format": ctx.client_format,
            "upstream_format": ctx.upstream_format,
            "client_model": ctx.client_model,
            "upstream_name": ctx.upstream_name,
            "upstream_model": ctx.upstream_model,
            "http_status": status,
            "outcome": outcome,
            "transport_outcome": transport_outcome,
            "protocol_terminal": summary.protocol_terminal,
            "response": {
                "terminal_event": summary.terminal_event_for_record(),
                "finish_reason": summary.finish_reason_for_record(),
                "text": summary.text_for_record(),
                "reasoning": summary.reasoning_for_record(),
                "tool_calls": summary.tool_calls_for_record(),
                "error": summary.error_for_record(),
            }
        }));
    }

    fn write_entry(&self, value: Value) {
        match self.writer.sender.try_send(value) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                self.writer.dropped_entries.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                warn!("debug trace entry dropped: writer_disconnected");
            }
        }
    }
}

fn flush_trace_overflow_summary(
    writer: &mut BufWriter<std::fs::File>,
    dropped_entries: &AtomicUsize,
) {
    let dropped = dropped_entries.swap(0, Ordering::Relaxed);
    if dropped == 0 {
        return;
    }
    let value = json!({
        "timestamp_ms": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        "phase": TracePhase::Response,
        "kind": "debug_trace_overflow",
        "overflow": {
            "dropped_entries": dropped,
        }
    });
    let Ok(line) = serde_json::to_vec(&value) else {
        return;
    };
    let _ = writer.write_all(&line);
    let _ = writer.write_all(b"\n");
}

impl<S> Stream for DebugTraceStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match std::pin::Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                this.buffer.extend_from_slice(&bytes);
                while let Some(event) = take_one_sse_event(&mut this.buffer) {
                    accumulate_event(this.ctx.client_format, &event, &mut this.summary);
                }
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(err))) => {
                if !this.finalized {
                    this.recorder.record_stream_result(
                        &this.ctx,
                        this.status,
                        &this.summary,
                        TransportOutcome::StreamError,
                    );
                    this.finalized = true;
                }
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                while let Some(event) = take_one_sse_event(&mut this.buffer) {
                    accumulate_event(this.ctx.client_format, &event, &mut this.summary);
                }
                if !this.finalized {
                    this.recorder.record_stream_result(
                        &this.ctx,
                        this.status,
                        &this.summary,
                        TransportOutcome::CompletedEof,
                    );
                    this.finalized = true;
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Drop for DebugTraceStream<S> {
    fn drop(&mut self) {
        if !self.finalized {
            self.recorder.record_stream_result(
                &self.ctx,
                self.status,
                &self.summary,
                TransportOutcome::ClientDisconnected,
            );
            self.finalized = true;
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
        .and_then(|arr| arr.first())?;
    let finish_reason = choice.get("finish_reason").and_then(Value::as_str)?;
    let kind = match finish_reason {
        "length" | "content_filter" | "pause_turn" => ProtocolTerminalKind::Incomplete,
        "context_length_exceeded" | "tool_error" | "error" => ProtocolTerminalKind::Failed,
        _ => ProtocolTerminalKind::Success,
    };
    Some(ProtocolTerminal {
        kind,
        event_type: "chat.completion.chunk".to_string(),
        finish_reason: Some(finish_reason.to_string()),
        incomplete_reason: matches!(kind, ProtocolTerminalKind::Incomplete)
            .then_some(finish_reason.to_string()),
        error: None,
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
            let kind = match stop_reason {
                "max_tokens" | "pause_turn" | "refusal" => ProtocolTerminalKind::Incomplete,
                "model_context_window_exceeded" => ProtocolTerminalKind::Failed,
                _ => ProtocolTerminalKind::Success,
            };
            Some(ProtocolTerminal {
                kind,
                event_type: "message_stop".to_string(),
                finish_reason: (!stop_reason.is_empty()).then_some(stop_reason.to_string()),
                incomplete_reason: matches!(kind, ProtocolTerminalKind::Incomplete)
                    .then_some(stop_reason.to_string()),
                error: None,
            })
        }
        _ => None,
    }
}

fn extract_request_delta(format: UpstreamFormat, body: &Value, max_text_chars: usize) -> Value {
    match format {
        UpstreamFormat::OpenAiCompletion => {
            let messages = body
                .get("messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let tail = trailing_client_messages(&messages, |msg| {
                matches!(
                    msg.get("role").and_then(Value::as_str),
                    Some("assistant") | Some("system") | Some("developer")
                )
            });
            Value::Array(
                tail.iter()
                    .map(|msg| summarize_chat_message(msg, max_text_chars))
                    .collect(),
            )
        }
        UpstreamFormat::OpenAiResponses => {
            if let Some(input) = body.get("input").and_then(Value::as_str) {
                return Value::Array(vec![json!({
                    "type": "message",
                    "role": "user",
                    "text": truncate_text(input, max_text_chars)
                })]);
            }
            let items = body
                .get("input")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let tail = trailing_client_messages(&items, |item| {
                let ty = item.get("type").and_then(Value::as_str);
                matches!(ty, Some("reasoning") | Some("function_call"))
                    || matches!(item.get("role").and_then(Value::as_str), Some("assistant"))
            });
            Value::Array(
                tail.iter()
                    .map(|item| summarize_responses_item(item, max_text_chars))
                    .collect(),
            )
        }
        UpstreamFormat::Anthropic => {
            let messages = body
                .get("messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let tail = trailing_client_messages(&messages, |msg| {
                msg.get("role").and_then(Value::as_str) == Some("assistant")
            });
            Value::Array(
                tail.iter()
                    .map(|msg| summarize_claude_message(msg, max_text_chars))
                    .collect(),
            )
        }
        UpstreamFormat::Google => body.clone(),
    }
}

fn summarize_request_body(format: UpstreamFormat, body: &Value, max_text_chars: usize) -> Value {
    match format {
        UpstreamFormat::OpenAiResponses => json!({
            "model": body.get("model"),
            "stream": body.get("stream"),
            "max_output_tokens": body.get("max_output_tokens"),
            "tool_choice": body.get("tool_choice"),
            "parallel_tool_calls": body.get("parallel_tool_calls"),
            "text": body.get("text"),
            "include": body.get("include"),
            "reasoning": body.get("reasoning"),
            "tool_names": body.get("tools").and_then(Value::as_array).map(|tools| tool_names_from_responses(tools)),
            "input_tail": extract_request_delta(format, body, max_text_chars),
        }),
        UpstreamFormat::OpenAiCompletion => json!({
            "model": body.get("model"),
            "stream": body.get("stream"),
            "max_tokens": body.get("max_tokens"),
            "temperature": body.get("temperature"),
            "top_p": body.get("top_p"),
            "stop": body.get("stop"),
            "tool_choice": body.get("tool_choice"),
            "parallel_tool_calls": body.get("parallel_tool_calls"),
            "tool_names": body.get("tools").and_then(Value::as_array).map(|tools| tool_names_from_chat_tools(tools)),
            "message_roles": body.get("messages").and_then(Value::as_array).map(|messages| message_roles(messages)),
            "messages_tail": extract_request_delta(format, body, max_text_chars),
        }),
        UpstreamFormat::Anthropic => json!({
            "model": body.get("model"),
            "stream": body.get("stream"),
            "max_tokens": body.get("max_tokens"),
            "temperature": body.get("temperature"),
            "top_p": body.get("top_p"),
            "tool_choice": body.get("tool_choice"),
            "tool_names": body.get("tools").and_then(Value::as_array).map(|tools| tool_names_from_claude_tools(tools)),
            "message_roles": body.get("messages").and_then(Value::as_array).map(|messages| message_roles(messages)),
            "messages_tail": extract_request_delta(format, body, max_text_chars),
        }),
        UpstreamFormat::Google => json!({
            "model": body.get("model"),
            "contents_count": body.get("contents").and_then(Value::as_array).map(|a| a.len()),
        }),
    }
}

fn tool_names_from_responses(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn tool_names_from_chat_tools(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .filter_map(|tool| {
            tool.get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn tool_names_from_claude_tools(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn message_roles(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|msg| msg.get("role").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn trailing_client_messages<F>(items: &[Value], is_model_boundary: F) -> Vec<Value>
where
    F: Fn(&Value) -> bool,
{
    let mut out = Vec::new();
    for item in items.iter().rev() {
        if is_model_boundary(item) {
            break;
        }
        out.push(item.clone());
    }
    out.reverse();
    out
}

fn summarize_chat_message(msg: &Value, max_text_chars: usize) -> Value {
    json!({
        "role": msg.get("role").and_then(Value::as_str).unwrap_or("unknown"),
        "content": summarize_openai_content(msg.get("content"), max_text_chars),
        "tool_call_id": msg.get("tool_call_id"),
        "tool_calls": msg.get("tool_calls").and_then(Value::as_array).map(|arr| {
            arr.iter().map(|tc| json!({
                "id": tc.get("id"),
                "name": tc.get("function").and_then(|f| f.get("name")),
                "arguments": truncate_text(tc.get("function").and_then(|f| f.get("arguments")).and_then(Value::as_str).unwrap_or(""), max_text_chars),
            })).collect::<Vec<_>>()
        }).unwrap_or_default(),
    })
}

fn summarize_responses_item(item: &Value, max_text_chars: usize) -> Value {
    match item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
    {
        "message" => json!({
            "type": "message",
            "role": item.get("role").and_then(Value::as_str).unwrap_or("unknown"),
            "content": summarize_responses_content(item.get("content"), max_text_chars),
        }),
        "function_call_output" => json!({
            "type": "function_call_output",
            "call_id": item.get("call_id"),
            "output": truncate_text(
                &value_to_display_string(item.get("output").unwrap_or(&Value::Null)),
                max_text_chars
            ),
        }),
        other => json!({
            "type": other,
            "value": truncate_text(&value_to_display_string(item), max_text_chars),
        }),
    }
}

fn summarize_claude_message(msg: &Value, max_text_chars: usize) -> Value {
    json!({
        "role": msg.get("role").and_then(Value::as_str).unwrap_or("unknown"),
        "content": msg.get("content").and_then(Value::as_array).map(|arr| {
            arr.iter().map(|block| {
                match block.get("type").and_then(Value::as_str).unwrap_or("unknown") {
                    "text" => json!({"type":"text","text": truncate_text(block.get("text").and_then(Value::as_str).unwrap_or(""), max_text_chars)}),
                    "tool_result" => json!({"type":"tool_result","tool_use_id": block.get("tool_use_id"), "content": truncate_text(&value_to_display_string(block.get("content").unwrap_or(&Value::Null)), max_text_chars)}),
                    other => json!({"type": other, "value": truncate_text(&value_to_display_string(block), max_text_chars)}),
                }
            }).collect::<Vec<_>>()
        }).unwrap_or_default()
    })
}

fn summarize_openai_content(content: Option<&Value>, max_text_chars: usize) -> Value {
    match content {
        Some(Value::String(s)) => Value::String(truncate_text(s, max_text_chars)),
        Some(Value::Array(arr)) => Value::Array(
            arr.iter()
                .map(|part| {
                    let ty = part.get("type").and_then(Value::as_str).unwrap_or("unknown");
                    match ty {
                        "text" => json!({"type": "text", "text": truncate_text(part.get("text").and_then(Value::as_str).unwrap_or(""), max_text_chars)}),
                        "image_url" => json!({"type": "image_url"}),
                        _ => json!({"type": ty}),
                    }
                })
                .collect(),
        ),
        Some(other) => Value::String(truncate_text(&value_to_display_string(other), max_text_chars)),
        None => Value::Null,
    }
}

fn summarize_responses_content(content: Option<&Value>, max_text_chars: usize) -> Value {
    Value::Array(
        content
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .map(|part| {
                        let ty = part.get("type").and_then(Value::as_str).unwrap_or("unknown");
                        match ty {
                            "input_text" | "output_text" => json!({
                                "type": ty,
                                "text": truncate_text(part.get("text").and_then(Value::as_str).unwrap_or(""), max_text_chars)
                            }),
                            other => json!({"type": other}),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    )
}

fn summarize_non_stream_response(
    format: UpstreamFormat,
    body: &Value,
    max_text_chars: usize,
) -> Value {
    match format {
        UpstreamFormat::OpenAiCompletion => {
            let choice = body
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|arr| arr.first())
                .cloned()
                .unwrap_or(Value::Null);
            json!({
                "finish_reason": choice.get("finish_reason"),
                "message": summarize_chat_message(choice.get("message").unwrap_or(&Value::Null), max_text_chars),
                "error": body.get("error"),
            })
        }
        UpstreamFormat::OpenAiResponses => json!({
            "status": body.get("status"),
            "output": body.get("output").and_then(Value::as_array).map(|arr| {
                arr.iter().map(|item| summarize_responses_item(item, max_text_chars)).collect::<Vec<_>>()
            }).unwrap_or_default(),
            "error": body.get("error"),
        }),
        UpstreamFormat::Anthropic => json!({
            "stop_reason": body.get("stop_reason"),
            "content": body.get("content").and_then(Value::as_array).map(|arr| {
                arr.iter().map(|block| summarize_claude_message(&json!({"role":"assistant","content":[block.clone()]}), max_text_chars)["content"][0].clone()).collect::<Vec<_>>()
            }).unwrap_or_default(),
            "error": body.get("error"),
        }),
        UpstreamFormat::Google => json!({
            "body": truncate_text(&value_to_display_string(body), max_text_chars),
        }),
    }
}

fn accumulate_event(format: UpstreamFormat, event: &Value, summary: &mut ResponseSummary) {
    match format {
        UpstreamFormat::OpenAiCompletion => accumulate_openai_completion_event(event, summary),
        UpstreamFormat::OpenAiResponses => accumulate_responses_event(event, summary),
        UpstreamFormat::Anthropic => accumulate_claude_event(event, summary),
        UpstreamFormat::Google => {}
    }
}

fn accumulate_openai_completion_event(event: &Value, summary: &mut ResponseSummary) {
    if event.get("_done").and_then(Value::as_bool) == Some(true) {
        summary.terminal_event = Some("done".to_string());
        summary
            .protocol_terminal
            .get_or_insert_with(|| ProtocolTerminal {
                kind: ProtocolTerminalKind::Success,
                event_type: "done".to_string(),
                finish_reason: Some("stop".to_string()),
                incomplete_reason: None,
                error: None,
            });
        return;
    }
    let choice = event
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first());
    if let Some(choice) = choice {
        if let Some(content) = choice
            .get("delta")
            .and_then(|d| d.get("content"))
            .and_then(Value::as_str)
        {
            summary.text.push_str(content);
        }
        if let Some(reasoning) = choice
            .get("delta")
            .and_then(|d| d.get("reasoning_content"))
            .and_then(Value::as_str)
        {
            summary.reasoning.push_str(reasoning);
        }
        if let Some(tool_calls) = choice
            .get("delta")
            .and_then(|d| d.get("tool_calls"))
            .and_then(Value::as_array)
        {
            for tool_call in tool_calls {
                summary.tool_calls.push(tool_call.clone());
            }
        }
        if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
            summary.finish_reason = Some(finish_reason.to_string());
            summary.terminal_event = Some("chat.completion.chunk".to_string());
        }
    }
    if let Some(error) = event.get("error") {
        summary.error = Some(error.clone());
    }
    if let Some(terminal) = openai_protocol_terminal(event) {
        summary.protocol_terminal = Some(terminal);
    }
}

fn accumulate_responses_event(event: &Value, summary: &mut ResponseSummary) {
    let ty = event.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "response.output_text.delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                summary.text.push_str(delta);
            }
        }
        "response.reasoning_summary_text.delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                summary.reasoning.push_str(delta);
            }
        }
        "response.function_call_arguments.delta" => {
            summary.tool_calls.push(json!({
                "item_id": event.get("item_id"),
                "delta": event.get("delta"),
            }));
        }
        "response.completed" | "response.incomplete" | "response.failed" => {
            summary.terminal_event = Some(ty.to_string());
            if let Some(reason) = event
                .get("response")
                .and_then(|r| r.get("incomplete_details"))
                .and_then(|d| d.get("reason"))
                .and_then(Value::as_str)
            {
                summary.finish_reason = Some(reason.to_string());
            }
            if let Some(error) = event.get("response").and_then(|r| r.get("error")) {
                if !error.is_null() {
                    summary.error = Some(error.clone());
                }
            }
            summary.protocol_terminal = responses_protocol_terminal(event);
        }
        _ => {}
    }
}

fn accumulate_claude_event(event: &Value, summary: &mut ResponseSummary) {
    match event.get("type").and_then(Value::as_str).unwrap_or("") {
        "content_block_delta" => {
            if let Some(delta) = event.get("delta") {
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            summary.text.push_str(text);
                        }
                    }
                    "thinking_delta" => {
                        if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                            summary.reasoning.push_str(text);
                        }
                    }
                    "input_json_delta" => {
                        summary.tool_calls.push(json!({
                            "partial_json": delta.get("partial_json")
                        }));
                    }
                    _ => {}
                }
            }
        }
        "message_delta" => {
            if let Some(stop_reason) = event
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(Value::as_str)
            {
                summary.finish_reason = Some(stop_reason.to_string());
            }
        }
        "message_stop" => {
            summary.terminal_event = Some("message_stop".to_string());
            summary.protocol_terminal =
                anthropic_protocol_terminal(event, summary.finish_reason.as_deref());
        }
        "error" => {
            summary.terminal_event = Some("error".to_string());
            summary.error = event.get("error").cloned();
            summary.protocol_terminal =
                anthropic_protocol_terminal(event, summary.finish_reason.as_deref());
        }
        _ => {}
    }
}

fn truncate_text(text: &str, max_text_chars: usize) -> String {
    let chars = text.chars().count();
    if chars <= max_text_chars {
        return text.to_string();
    }
    let head = text.chars().take(max_text_chars).collect::<String>();
    format!("{head}…[{chars} chars]")
}

fn truncate_value_strings(value: &Value, max_text_chars: usize) -> Value {
    match value {
        Value::String(text) => Value::String(truncate_text(text, max_text_chars)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| truncate_value_strings(item, max_text_chars))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), truncate_value_strings(value, max_text_chars)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn value_to_display_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "<unprintable>".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::time::{Duration, Instant};

    #[test]
    fn success_terminal_projects_completed_outcome() {
        let summary = ResponseSummary {
            protocol_terminal: Some(ProtocolTerminal {
                kind: ProtocolTerminalKind::Success,
                event_type: "response.completed".to_string(),
                finish_reason: Some("stop".to_string()),
                incomplete_reason: None,
                error: None,
            }),
            ..ResponseSummary::new(16)
        };

        assert!(matches!(
            project_trace_outcome(TransportOutcome::ClientDisconnected, &summary),
            TraceOutcome::Completed
        ));
    }

    #[test]
    fn failed_terminal_does_not_project_completed_outcome() {
        let summary = ResponseSummary {
            protocol_terminal: Some(ProtocolTerminal {
                kind: ProtocolTerminalKind::Failed,
                event_type: "response.failed".to_string(),
                finish_reason: None,
                incomplete_reason: None,
                error: Some(json!({"code":"tool_error"})),
            }),
            ..ResponseSummary::new(16)
        };

        assert!(matches!(
            project_trace_outcome(TransportOutcome::ClientDisconnected, &summary),
            TraceOutcome::Failed
        ));
    }

    #[test]
    fn incomplete_terminal_does_not_project_completed_outcome() {
        let summary = ResponseSummary {
            protocol_terminal: Some(ProtocolTerminal {
                kind: ProtocolTerminalKind::Incomplete,
                event_type: "response.incomplete".to_string(),
                finish_reason: None,
                incomplete_reason: Some("pause_turn".to_string()),
                error: None,
            }),
            ..ResponseSummary::new(16)
        };

        assert!(matches!(
            project_trace_outcome(TransportOutcome::ClientDisconnected, &summary),
            TraceOutcome::Incomplete
        ));
    }

    #[test]
    fn debug_trace_recorder_writes_via_background_writer() {
        let path =
            std::env::temp_dir().join(format!("debug-trace-test-{}.jsonl", uuid::Uuid::new_v4()));
        let recorder = DebugTraceRecorder::new(&DebugTraceConfig {
            path: Some(path.to_string_lossy().to_string()),
            max_text_chars: 32,
        })
        .expect("recorder");

        recorder.record_non_stream_response(
            &DebugTraceContext {
                request_id: "req_1".to_string(),
                timestamp_ms: 1,
                path: "/openai/v1/chat/completions".to_string(),
                stream: false,
                client_model: "gpt-4.1".to_string(),
                upstream_name: "default".to_string(),
                upstream_model: "gpt-4.1".to_string(),
                client_format: UpstreamFormat::OpenAiCompletion,
                upstream_format: UpstreamFormat::OpenAiCompletion,
            },
            200,
            &json!({
                "choices": [{
                    "finish_reason": "stop",
                    "message": { "role": "assistant", "content": "hello" }
                }]
            }),
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut contents = String::new();
        while Instant::now() < deadline {
            contents = fs::read_to_string(&path).unwrap_or_default();
            if contents.contains("\"request_id\":\"req_1\"") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        assert!(contents.contains("\"request_id\":\"req_1\""), "{contents}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn debug_trace_recorder_emits_explicit_overflow_summary_when_queue_fills() {
        let path = std::env::temp_dir().join(format!(
            "debug-trace-burst-test-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let start_barrier = Arc::new(std::sync::Barrier::new(2));
        let recorder = DebugTraceRecorder::new_with_options(
            &DebugTraceConfig {
                path: Some(path.to_string_lossy().to_string()),
                max_text_chars: 128,
            },
            TraceWriterOptions {
                queue_capacity: 1,
                start_barrier: Some(start_barrier.clone()),
            },
        )
        .expect("recorder");
        let ctx = DebugTraceContext {
            request_id: String::new(),
            timestamp_ms: 1,
            path: "/openai/v1/chat/completions".to_string(),
            stream: false,
            client_model: "gpt-4.1".to_string(),
            upstream_name: "default".to_string(),
            upstream_model: "gpt-4.1".to_string(),
            client_format: UpstreamFormat::OpenAiCompletion,
            upstream_format: UpstreamFormat::OpenAiCompletion,
        };
        let body = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": { "role": "assistant", "content": "x".repeat(4096) }
            }]
        });

        for idx in 0..5usize {
            let mut entry_ctx = ctx.clone();
            entry_ctx.request_id = format!("req_{idx}");
            recorder.record_non_stream_response(&entry_ctx, 200, &body);
        }
        start_barrier.wait();
        drop(recorder);

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut contents = String::new();
        while Instant::now() < deadline {
            contents = fs::read_to_string(&path).unwrap_or_default();
            if contents.contains("\"kind\":\"debug_trace_overflow\"")
                && contents.contains("\"dropped_entries\":")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        assert!(
            contents.contains("\"kind\":\"debug_trace_overflow\""),
            "{contents}"
        );
        assert!(contents.contains("\"dropped_entries\":"), "{contents}");
        let entries: Vec<Value> = contents
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<Value>(line).expect("debug trace line should parse"))
            .collect();
        let overflow_entry = entries
            .iter()
            .find(|value| value.get("kind").and_then(Value::as_str) == Some("debug_trace_overflow"))
            .expect("overflow entry should be present");
        let persisted_responses = entries
            .iter()
            .filter(|value| value.get("phase").and_then(Value::as_str) == Some("response"))
            .filter(|value| value.get("kind").is_none())
            .count();
        let dropped_entries = overflow_entry
            .get("overflow")
            .and_then(|value| value.get("dropped_entries"))
            .and_then(Value::as_u64)
            .expect("overflow entry should report dropped entries");

        assert_eq!(
            persisted_responses as u64 + dropped_entries,
            5,
            "bounded debug trace queue should account for every attempted write via persisted entries plus explicit overflow accounting: {contents}"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn response_summary_bounds_text_reasoning_and_tool_calls_during_accumulation() {
        let mut summary = ResponseSummary::new(12);

        accumulate_event(
            UpstreamFormat::OpenAiCompletion,
            &json!({
                "choices": [{
                    "delta": {
                        "content": "hello world",
                        "reasoning_content": "reasoning trail",
                        "tool_calls": [{
                            "id": "call_1",
                            "function": {
                                "name": "lookup",
                                "arguments": "abcdefghijklmnopqrstuvwxyz"
                            }
                        }]
                    }
                }]
            }),
            &mut summary,
        );
        accumulate_event(
            UpstreamFormat::OpenAiCompletion,
            &json!({
                "choices": [{
                    "delta": {
                        "content": " plus more text",
                        "reasoning_content": " and more reasoning",
                        "tool_calls": [{
                            "id": "call_2",
                            "function": {
                                "name": "lookup_2",
                                "arguments": "mnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz"
                            }
                        }]
                    }
                }]
            }),
            &mut summary,
        );

        let text = summary.text_for_record();
        let reasoning = summary.reasoning_for_record();
        let tool_calls = summary.tool_calls_for_record();

        assert!(summary.text.head.chars().count() <= 12);
        assert!(summary.reasoning.head.chars().count() <= 12);
        assert!(text.starts_with("hello world"));
        assert!(text.contains("chars"));
        assert!(reasoning.starts_with("reasoning tr"));
        assert!(reasoning.contains("chars"));
        assert!(tool_calls.len() <= 2);
        assert!(tool_calls
            .iter()
            .any(|value| { value.get("type").and_then(Value::as_str) == Some("truncated") }));
    }
}
