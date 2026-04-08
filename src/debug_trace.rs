use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;
use serde::Serialize;
use serde_json::{json, Value};

use crate::config::DebugTraceConfig;
use crate::formats::UpstreamFormat;
use crate::streaming::take_one_sse_event;

#[derive(Clone)]
pub struct DebugTraceRecorder {
    sink: Arc<Mutex<std::fs::File>>,
    max_text_chars: usize,
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

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum StreamOutcome {
    Completed,
    ClientDisconnected,
    StreamError,
}

#[derive(Default)]
struct ResponseSummary {
    text: String,
    reasoning: String,
    tool_calls: Vec<Value>,
    terminal_event: Option<String>,
    finish_reason: Option<String>,
    error: Option<Value>,
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
        let path = config.path.as_ref()?;
        let parent = Path::new(path).parent();
        if let Some(parent) = parent {
            if !parent.as_os_str().is_empty() && create_dir_all(parent).is_err() {
                return None;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        Some(Self {
            sink: Arc::new(Mutex::new(file)),
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
            "outcome": StreamOutcome::Completed,
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
            summary: ResponseSummary::default(),
            finalized: false,
        }
    }

    fn record_stream_result(
        &self,
        ctx: &DebugTraceContext,
        status: u16,
        summary: &ResponseSummary,
        outcome: StreamOutcome,
    ) {
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
            "response": {
                "terminal_event": summary.terminal_event,
                "finish_reason": summary.finish_reason,
                "text": truncate_text(&summary.text, self.max_text_chars),
                "reasoning": truncate_text(&summary.reasoning, self.max_text_chars),
                "tool_calls": summary.tool_calls,
                "error": summary.error,
            }
        }));
    }

    fn write_entry(&self, value: Value) {
        let Ok(mut sink) = self.sink.lock() else {
            return;
        };
        let Ok(line) = serde_json::to_vec(&value) else {
            return;
        };
        let _ = sink.write_all(&line);
        let _ = sink.write_all(b"\n");
        let _ = sink.flush();
    }
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
                        StreamOutcome::StreamError,
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
                        StreamOutcome::Completed,
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
                StreamOutcome::ClientDisconnected,
            );
            self.finalized = true;
        }
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
            summary.tool_calls.extend(tool_calls.iter().cloned());
        }
        if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
            summary.finish_reason = Some(finish_reason.to_string());
            summary.terminal_event = Some("chat.completion.chunk".to_string());
        }
    }
    if let Some(error) = event.get("error") {
        summary.error = Some(error.clone());
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
        }
        "error" => {
            summary.terminal_event = Some("error".to_string());
            summary.error = event.get("error").cloned();
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

fn value_to_display_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "<unprintable>".to_string()),
    }
}
