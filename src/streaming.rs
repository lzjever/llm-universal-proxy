//! SSE streaming: passthrough when formats match, otherwise transform chunks (upstream → openai → client).
//!
//! Reference: 9router open-sse/handlers/chatCore/streamingHandler.js, utils/stream.js.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use serde_json::Value;

use crate::formats::UpstreamFormat;

/// Whether we need to transform the upstream SSE stream for the client.
pub fn needs_stream_translation(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
) -> bool {
    upstream_format != client_format
}

/// Stream transformer state (per 9router initState).
#[derive(Debug, Default)]
pub struct StreamState {
    pub message_id: Option<String>,
    pub model: Option<String>,
    pub tool_call_index: usize,
    pub tool_calls: std::collections::HashMap<usize, ToolCallState>,
    pub server_tool_block_index: Option<usize>,
    pub text_block_started: bool,
    pub in_thinking_block: bool,
    pub current_block_index: Option<usize>,
    pub finish_reason: Option<String>,
    pub finish_reason_sent: bool,
    pub usage: Option<serde_json::Value>,
    // OpenAI → Claude output state
    pub message_start_sent: bool,
    pub next_block_index: usize,
    pub thinking_block_started: bool,
    pub thinking_block_index: usize,
    pub text_block_index: usize,
    pub text_block_closed: bool,
    pub tool_block_indices: std::collections::HashMap<usize, usize>,
    // Gemini state
    pub function_index: usize,
    // OpenAI Responses API client output state
    pub responses_seq: u64,
    pub responses_started: bool,
    pub output_item_id: Option<String>,
    pub output_item_added: bool,
    pub responses_content_part_added: bool,
    pub responses_output_text: String,
    pub responses_reasoning_id: Option<String>,
    pub responses_reasoning_added: bool,
    pub responses_reasoning_done: bool,
    pub responses_reasoning_text: String,
}

#[derive(Debug, Default)]
pub struct ToolCallState {
    pub index: usize,
    pub id: Option<Value>,
    pub name: String,
    pub arguments: String,
    pub block_index: Option<usize>,
    pub responses_item_added: bool,
    pub responses_done: bool,
}

/// Extract one SSE event from buffer (up to and including first "\n\n"). Returns parsed JSON from "data: " line, or None.
/// Buffer is updated: consumed bytes are removed.
pub fn take_one_sse_event(buffer: &mut Vec<u8>) -> Option<Value> {
    let pos = buffer.windows(2).position(|w| w == b"\n\n")?;
    let event_bytes = buffer.drain(..=pos + 1).collect::<Vec<_>>();
    let event_str = String::from_utf8_lossy(&event_bytes);
    for line in event_str.lines() {
        let line = line.trim();
        if line.starts_with("data: ") {
            let data = line.strip_prefix("data: ").unwrap_or("").trim();
            if data == "[DONE]" || data.is_empty() {
                return Some(serde_json::json!({ "_done": true }));
            }
            return serde_json::from_str(data).ok();
        }
    }
    None
}

/// Format one JSON value as SSE "data: {json}\n\n".
pub fn format_sse_data(value: &Value) -> Vec<u8> {
    let s = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let mut out = b"data: ".to_vec();
    out.extend_from_slice(s.as_bytes());
    out.extend_from_slice(b"\n\n");
    out
}

/// Format SSE with event type line: "event: {ty}\ndata: {json}\n\n".
pub fn format_sse_event(event_type: &str, value: &Value) -> Vec<u8> {
    let s = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let mut out = format!("event: {}\n", event_type).into_bytes();
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(s.as_bytes());
    out.extend_from_slice(b"\n\n");
    out
}

/// Convert Claude SSE event to one or more OpenAI-format chunks. Updates state.
pub fn claude_event_to_openai_chunks(event: &Value, state: &mut StreamState) -> Vec<Value> {
    let ty = event.get("type").and_then(Value::as_str);
    let mut out = vec![];
    match ty {
        Some("message_start") => {
            state.message_id = event
                .get("message")
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
                .map(String::from);
            state.model = event
                .get("message")
                .and_then(|m| m.get("model"))
                .and_then(Value::as_str)
                .map(String::from);
            state.tool_call_index = 0;
            out.push(openai_chunk(
                state,
                serde_json::json!({ "role": "assistant" }),
                None,
            ));
        }
        Some("content_block_start") => {
            let block = event.get("content_block");
            let block_ty = block.and_then(|b| b.get("type").and_then(Value::as_str));
            if block_ty == Some("server_tool_use") {
                state.server_tool_block_index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|i| i as usize);
                return out;
            }
            if block_ty == Some("text") {
                state.text_block_started = true;
            } else if block_ty == Some("thinking") {
                state.in_thinking_block = true;
                state.current_block_index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|i| i as usize);
                out.push(openai_chunk(
                    state,
                    serde_json::json!({ "reasoning_content": "<think>" }),
                    None,
                ));
            } else if block_ty == Some("tool_use") {
                let block = block.unwrap();
                let idx = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let tc_index = state.tool_call_index;
                state.tool_call_index += 1;
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let tc = serde_json::json!({
                    "index": tc_index,
                    "id": block.get("id"),
                    "type": "function",
                    "function": { "name": name, "arguments": "" }
                });
                state.tool_calls.insert(
                    idx,
                    ToolCallState {
                        index: tc_index,
                        id: block.get("id").cloned(),
                        name: name.clone(),
                        arguments: String::new(),
                        block_index: None,
                        ..Default::default()
                    },
                );
                out.push(openai_chunk(
                    state,
                    serde_json::json!({ "tool_calls": [tc] }),
                    None,
                ));
            }
        }
        Some("content_block_delta") => {
            let idx = event
                .get("index")
                .and_then(Value::as_u64)
                .map(|i| i as usize);
            if state.server_tool_block_index == idx {
                return out;
            }
            let delta = event.get("delta");
            let delta_ty = delta.and_then(|d| d.get("type").and_then(Value::as_str));
            if delta_ty == Some("text_delta") {
                if let Some(t) = delta.and_then(|d| d.get("text").and_then(Value::as_str)) {
                    if !t.is_empty() {
                        out.push(openai_chunk(
                            state,
                            serde_json::json!({ "content": t }),
                            None,
                        ));
                    }
                }
            } else if delta_ty == Some("thinking_delta") {
                if let Some(t) = delta.and_then(|d| d.get("thinking").and_then(Value::as_str)) {
                    if !t.is_empty() {
                        out.push(openai_chunk(
                            state,
                            serde_json::json!({ "reasoning_content": t }),
                            None,
                        ));
                    }
                }
            } else if delta_ty == Some("input_json_delta") {
                if let Some(pj) = delta.and_then(|d| d.get("partial_json").and_then(Value::as_str))
                {
                    let chunk_json =
                        if let Some(tc) = idx.and_then(|i| state.tool_calls.get_mut(&i)) {
                            tc.arguments.push_str(pj);
                            Some(serde_json::json!({
                                "tool_calls": [{
                                    "index": tc.index,
                                    "id": tc.id,
                                    "function": { "arguments": pj }
                                }]
                            }))
                        } else {
                            None
                        };
                    if let Some(cj) = chunk_json {
                        out.push(openai_chunk(state, cj, None));
                    }
                }
            }
        }
        Some("content_block_stop") => {
            let idx = event
                .get("index")
                .and_then(Value::as_u64)
                .map(|i| i as usize);
            if state.server_tool_block_index == idx {
                state.server_tool_block_index = None;
                return out;
            }
            if state.in_thinking_block && state.current_block_index == idx {
                out.push(openai_chunk(
                    state,
                    serde_json::json!({ "reasoning_content": "" }),
                    None,
                ));
                state.in_thinking_block = false;
            }
        }
        Some("message_delta") => {
            if let Some(stop) = event
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(Value::as_str)
            {
                state.finish_reason = Some(convert_claude_stop_reason(stop));
            }
            if let Some(u) = event.get("usage") {
                state.usage = Some(u.clone());
            }
        }
        Some("message_stop") => {
            if !state.finish_reason_sent {
                let fr = state.finish_reason.clone().unwrap_or_else(|| {
                    if state.tool_calls.is_empty() {
                        "stop".to_string()
                    } else {
                        "tool_calls".to_string()
                    }
                });
                let mut chunk = openai_chunk(state, serde_json::json!({}), Some(&fr));
                // Usage with cache token reporting
                // Reference: 9router claude-to-openai.js - include cache_read_input_tokens, cache_creation_input_tokens
                if let Some(ref u) = state.usage {
                    let input_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                    let output_tokens = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                    let cache_read = u
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let cache_creation = u
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);

                    // prompt_tokens = input_tokens + cache_read + cache_creation (matches 9router)
                    let prompt_tokens = input_tokens + cache_read + cache_creation;

                    let mut usage_json = serde_json::json!({
                        "prompt_tokens": prompt_tokens,
                        "completion_tokens": output_tokens,
                        "total_tokens": prompt_tokens + output_tokens
                    });

                    // Add cache details if present
                    if cache_read > 0 {
                        usage_json["cache_read_input_tokens"] = Value::Number(cache_read.into());
                    }
                    if cache_creation > 0 {
                        usage_json["cache_creation_input_tokens"] =
                            Value::Number(cache_creation.into());
                    }

                    chunk["usage"] = usage_json;
                }
                out.push(chunk);
                state.finish_reason_sent = true;
            }
        }
        _ => {}
    }
    out
}

fn openai_chunk(state: &StreamState, delta: Value, finish_reason: Option<&str>) -> Value {
    let chunk_id = state
        .message_id
        .as_deref()
        .map(|s| {
            if s.starts_with("chatcmpl-") {
                s.to_string()
            } else {
                format!("chatcmpl-{}", s)
            }
        })
        .unwrap_or_else(|| "chatcmpl-0".to_string());
    let mut c = serde_json::json!({
        "id": chunk_id,
        "object": "chat.completion.chunk",
        "created": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "model": state.model.as_deref().unwrap_or(""),
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }]
    });
    if let Some(fr) = finish_reason {
        c["choices"][0]["finish_reason"] = serde_json::json!(fr);
    }
    c
}

fn convert_claude_stop_reason(r: &str) -> String {
    match r {
        "end_turn" => "stop",
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        "stop_sequence" => "stop",
        "model_context_window_exceeded" => "context_length_exceeded",
        _ => "stop",
    }
    .to_string()
}

/// If event is OpenAI chunk (has choices[].delta), return as single-item vec. Else return empty.
pub fn openai_event_as_chunk(event: &Value) -> Option<Value> {
    if event.get("_done").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    if event
        .get("choices")
        .and_then(Value::as_array)
        .map(|c| !c.is_empty())
        .unwrap_or(false)
    {
        return Some(event.clone());
    }
    None
}

/// Convert Gemini SSE event (response with candidates[0].content.parts) to OpenAI-format chunks.
pub fn gemini_event_to_openai_chunks(event: &Value, state: &mut StreamState) -> Vec<Value> {
    let response = event.get("response").unwrap_or(event);
    let candidates = match response.get("candidates").and_then(Value::as_array) {
        Some(c) if !c.is_empty() => c,
        _ => return vec![],
    };
    let candidate = &candidates[0];
    let content = candidate.get("content");
    let parts = content
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array);
    let mut out = vec![];

    if state.message_id.is_none() {
        state.message_id = response
            .get("responseId")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| Some("msg_gemini".to_string()));
        state.model = response
            .get("modelVersion")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| Some("gemini".to_string()));
        state.function_index = 0;
        out.push(openai_chunk(
            state,
            serde_json::json!({ "role": "assistant" }),
            None,
        ));
    }

    if let Some(parts) = parts {
        for part in parts {
            let has_thought_sig =
                part.get("thoughtSignature").is_some() || part.get("thought_signature").is_some();
            let is_thought = part.get("thought").and_then(Value::as_bool) == Some(true);
            if has_thought_sig {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    if !t.is_empty() {
                        let delta = if is_thought {
                            serde_json::json!({ "reasoning_content": t })
                        } else {
                            serde_json::json!({ "content": t })
                        };
                        out.push(openai_chunk(state, delta, None));
                    }
                }
                if let Some(fc) = part.get("functionCall") {
                    let name = fc
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let args = fc.get("args").cloned().unwrap_or(serde_json::json!({}));
                    let args_str =
                        serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
                    let tc_index = state.function_index;
                    state.function_index += 1;
                    let id = fc
                        .get("id")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!(format!("{}-{}", name, tc_index)));
                    let tc = serde_json::json!({
                        "index": tc_index,
                        "id": id,
                        "type": "function",
                        "function": { "name": name, "arguments": args_str }
                    });
                    state.tool_calls.insert(
                        tc_index,
                        ToolCallState {
                            index: tc_index,
                            id: Some(id.clone()),
                            name: name.clone(),
                            arguments: args_str,
                            block_index: None,
                            ..Default::default()
                        },
                    );
                    out.push(openai_chunk(
                        state,
                        serde_json::json!({ "tool_calls": [tc] }),
                        None,
                    ));
                }
                continue;
            }
            if let Some(t) = part.get("text").and_then(Value::as_str) {
                if !t.is_empty() {
                    out.push(openai_chunk(
                        state,
                        serde_json::json!({ "content": t }),
                        None,
                    ));
                }
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args = fc.get("args").cloned().unwrap_or(serde_json::json!({}));
                let args_str = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
                let tc_index = state.function_index;
                state.function_index += 1;
                let id = fc
                    .get("id")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!(format!("{}-{}", name, tc_index)));
                let tc = serde_json::json!({
                    "index": tc_index,
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args_str }
                });
                state.tool_calls.insert(
                    tc_index,
                    ToolCallState {
                        index: tc_index,
                        id: Some(id.clone()),
                        name,
                        arguments: args_str,
                        block_index: None,
                        ..Default::default()
                    },
                );
                out.push(openai_chunk(
                    state,
                    serde_json::json!({ "tool_calls": [tc] }),
                    None,
                ));
            }
        }
    }

    // Usage with cache token reporting
    // Reference: 9router gemini-to-openai.js - include cachedContentTokenCount as prompt_tokens_details.cached_tokens
    if let Some(usage_meta) = response.get("usageMetadata").or(event.get("usageMetadata")) {
        let prompt_tokens = usage_meta
            .get("promptTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let candidates_tokens = usage_meta
            .get("candidatesTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let thoughts_tokens = usage_meta
            .get("thoughtsTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total_tokens = usage_meta
            .get("totalTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cached_tokens = usage_meta
            .get("cachedContentTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        // completion_tokens = candidatesTokenCount + thoughtsTokenCount (matches 9router)
        let completion_tokens = candidates_tokens + thoughts_tokens;

        let mut usage_json = serde_json::json!({
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": total_tokens
        });

        // Add prompt_tokens_details if cached tokens exist
        if cached_tokens > 0 {
            usage_json["prompt_tokens_details"] = serde_json::json!({
                "cached_tokens": cached_tokens
            });
        }

        state.usage = Some(usage_json);
    }

    if let Some(finish) = candidate.get("finishReason").and_then(Value::as_str) {
        let mut fr = finish.to_lowercase();
        if fr == "stop" && !state.tool_calls.is_empty() {
            fr = "tool_calls".to_string();
        }
        let mut chunk = openai_chunk(state, serde_json::json!({}), Some(&fr));
        if let Some(ref u) = state.usage {
            chunk["usage"] = u.clone();
        }
        out.push(chunk);
        state.finish_reason = Some(fr);
        state.finish_reason_sent = true;
    }
    out
}

/// Convert OpenAI Responses API SSE event to OpenAI completion chunks.
/// Event type is in data.type (e.g. response.output_text.delta).
pub fn responses_event_to_openai_chunks(event: &Value, state: &mut StreamState) -> Vec<Value> {
    let ty = event.get("type").and_then(Value::as_str).unwrap_or("");
    let mut out = vec![];

    if ty == "response.created" {
        let resp = event.get("response").unwrap_or(event);
        state.message_id = resp.get("id").and_then(Value::as_str).map(String::from);
        state.model = Some("unknown".to_string());
        out.push(openai_chunk(
            state,
            serde_json::json!({ "role": "assistant" }),
            None,
        ));
        return out;
    }

    if ty == "response.output_text.delta" {
        let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
        if !delta.is_empty() {
            out.push(openai_chunk(
                state,
                serde_json::json!({ "content": delta }),
                None,
            ));
        }
        return out;
    }

    if ty == "response.reasoning_summary_text.delta" {
        let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
        if !delta.is_empty() {
            out.push(openai_chunk(
                state,
                serde_json::json!({ "reasoning_content": delta }),
                None,
            ));
        }
        return out;
    }

    if ty == "response.output_item.added" {
        let item = event.get("item").unwrap_or(&serde_json::Value::Null);
        let item_ty = item.get("type").and_then(Value::as_str);
        if item_ty == Some("function_call") || item_ty == Some("custom_tool_call") {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .map(String::from);
            let idx = state.tool_call_index;
            state.tool_call_index += 1;
            let id = call_id.unwrap_or_else(|| format!("call_{}", idx));
            let tc = serde_json::json!({
                "index": idx,
                "id": id,
                "type": "function",
                "function": { "name": name, "arguments": "" }
            });
            state.tool_calls.insert(
                idx,
                ToolCallState {
                    index: idx,
                    id: Some(serde_json::json!(id)),
                    name,
                    arguments: String::new(),
                    block_index: None,
                    ..Default::default()
                },
            );
            out.push(openai_chunk(
                state,
                serde_json::json!({ "tool_calls": [tc] }),
                None,
            ));
        }
        return out;
    }

    if ty == "response.function_call_arguments.delta"
        || ty == "response.custom_tool_call_input.delta"
    {
        let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
        if !delta.is_empty() {
            let idx = state.tool_call_index.saturating_sub(1);
            if let Some(tc) = state.tool_calls.get_mut(&idx) {
                tc.arguments.push_str(delta);
            }
            out.push(openai_chunk(
                state,
                serde_json::json!({
                    "tool_calls": [{ "index": idx, "function": { "arguments": delta } }]
                }),
                None,
            ));
        }
        return out;
    }

    if ty == "response.completed" || ty == "response.incomplete" || ty == "response.failed" {
        if let Some(resp) = event.get("response") {
            if let Some(u) = resp.get("usage") {
                state.usage = Some(serde_json::json!({
                    "prompt_tokens": u.get("input_tokens").or(u.get("prompt_tokens")).and_then(Value::as_u64).unwrap_or(0),
                    "completion_tokens": u.get("output_tokens").or(u.get("completion_tokens")).and_then(Value::as_u64).unwrap_or(0)
                }));
            }
        }
        if !state.finish_reason_sent {
            let finish_reason = match ty {
                "response.incomplete" => event
                    .get("response")
                    .and_then(|resp| resp.get("incomplete_details"))
                    .and_then(|details| details.get("reason"))
                    .and_then(Value::as_str)
                    .map(|reason| match reason {
                        "max_output_tokens" => "length",
                        "content_filter" => "content_filter",
                        _ => "stop",
                    })
                    .unwrap_or("stop"),
                "response.failed" => event
                    .get("response")
                    .and_then(|resp| resp.get("error"))
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_str)
                    .map(|code| match code {
                        "context_length_exceeded" => "context_length_exceeded",
                        "content_filter" | "invalid_prompt" => "content_filter",
                        _ => "stop",
                    })
                    .unwrap_or("stop"),
                _ => "stop",
            };
            let mut chunk = openai_chunk(state, serde_json::json!({}), Some(finish_reason));
            if let Some(ref u) = state.usage {
                chunk["usage"] = u.clone();
            }
            out.push(chunk);
            state.finish_reason_sent = true;
        }
        return out;
    }

    out
}

/// Translate one parsed SSE event (JSON) from upstream format to client format. Returns bytes to send (one or more "data: ...\n\n").
pub fn translate_sse_event(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    event: &Value,
    state: &mut StreamState,
) -> Vec<Vec<u8>> {
    if upstream_format == client_format {
        if event.get("_done").and_then(Value::as_bool) == Some(true) {
            return vec![b"data: [DONE]\n\n".to_vec()];
        }
        return vec![format_sse_data(event)];
    }
    let openai_chunks: Vec<Value> = match upstream_format {
        UpstreamFormat::OpenAiCompletion => openai_event_as_chunk(event).into_iter().collect(),
        UpstreamFormat::Anthropic => claude_event_to_openai_chunks(event, state),
        UpstreamFormat::Google => gemini_event_to_openai_chunks(event, state),
        UpstreamFormat::OpenAiResponses => responses_event_to_openai_chunks(event, state),
    };
    if client_format == UpstreamFormat::OpenAiCompletion {
        return openai_chunks
            .into_iter()
            .map(|c| format_sse_data(&c))
            .collect();
    }
    if client_format == UpstreamFormat::Anthropic {
        let mut out = Vec::new();
        for c in &openai_chunks {
            out.extend(openai_chunk_to_claude_sse(c, state));
        }
        if !out.is_empty() {
            return out;
        }
    }
    if client_format == UpstreamFormat::Google {
        let mut out = Vec::new();
        for c in &openai_chunks {
            out.extend(openai_chunk_to_gemini_sse(c, state));
        }
        if !out.is_empty() {
            return out;
        }
    }
    if client_format == UpstreamFormat::OpenAiResponses {
        let mut out = Vec::new();
        for c in &openai_chunks {
            out.extend(openai_chunk_to_responses_sse(c, state));
        }
        if !out.is_empty() {
            return out;
        }
    }
    openai_chunks
        .into_iter()
        .map(|c| format_sse_data(&c))
        .collect()
}

fn stop_thinking_block_claude(state: &mut StreamState, out: &mut Vec<Vec<u8>>) {
    if !state.thinking_block_started {
        return;
    }
    out.push(format_sse_event(
        "content_block_stop",
        &serde_json::json!({ "type": "content_block_stop", "index": state.thinking_block_index }),
    ));
    state.thinking_block_started = false;
}

fn stop_text_block_claude(state: &mut StreamState, out: &mut Vec<Vec<u8>>) {
    if !state.text_block_started || state.text_block_closed {
        return;
    }
    state.text_block_closed = true;
    out.push(format_sse_event(
        "content_block_stop",
        &serde_json::json!({ "type": "content_block_stop", "index": state.text_block_index }),
    ));
}

fn convert_openai_finish_to_claude(reason: &str) -> &'static str {
    match reason {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        _ => "end_turn",
    }
}

fn openai_chunk_to_claude_sse(chunk: &Value, state: &mut StreamState) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let choices = match chunk.get("choices").and_then(Value::as_array) {
        Some(c) if !c.is_empty() => c,
        _ => return out,
    };
    let choice = &choices[0];
    let delta = choice.get("delta").unwrap_or(&serde_json::Value::Null);
    let finish_reason = choice.get("finish_reason").and_then(Value::as_str);

    if !state.message_start_sent {
        state.message_start_sent = true;
        state.message_id = chunk
            .get("id")
            .and_then(Value::as_str)
            .map(|s| s.strip_prefix("chatcmpl-").unwrap_or(s).to_string())
            .filter(|s| !s.is_empty() && s != "chat" && s.len() >= 8)
            .or_else(|| Some("msg_0".to_string()));
        state.model = choice
            .get("model")
            .or(chunk.get("model"))
            .and_then(Value::as_str)
            .map(String::from);
        state.next_block_index = 0;
        let msg = serde_json::json!({
            "type": "message_start",
            "message": {
                "id": state.message_id,
                "type": "message",
                "role": "assistant",
                "model": state.model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": { "input_tokens": 0, "output_tokens": 0 }
            }
        });
        out.push(format_sse_event("message_start", &msg));
    }

    if let Some(reasoning) = delta
        .get("reasoning_content")
        .or(delta.get("reasoning"))
        .and_then(Value::as_str)
    {
        if !reasoning.is_empty() {
            stop_text_block_claude(state, &mut out);
            if !state.thinking_block_started {
                state.thinking_block_index = state.next_block_index;
                state.next_block_index += 1;
                state.thinking_block_started = true;
                let ev = serde_json::json!({
                    "type": "content_block_start",
                    "index": state.thinking_block_index,
                    "content_block": { "type": "thinking", "thinking": "" }
                });
                out.push(format_sse_event("content_block_start", &ev));
            }
            let ev = serde_json::json!({
                "type": "content_block_delta",
                "index": state.thinking_block_index,
                "delta": { "type": "thinking_delta", "thinking": reasoning }
            });
            out.push(format_sse_event("content_block_delta", &ev));
        }
    }

    if let Some(content) = delta.get("content").and_then(Value::as_str) {
        if !content.is_empty() {
            stop_thinking_block_claude(state, &mut out);
            if !state.text_block_started {
                state.text_block_index = state.next_block_index;
                state.next_block_index += 1;
                state.text_block_started = true;
                state.text_block_closed = false;
                let ev = serde_json::json!({
                    "type": "content_block_start",
                    "index": state.text_block_index,
                    "content_block": { "type": "text", "text": "" }
                });
                out.push(format_sse_event("content_block_start", &ev));
            }
            let ev = serde_json::json!({
                "type": "content_block_delta",
                "index": state.text_block_index,
                "delta": { "type": "text_delta", "text": content }
            });
            out.push(format_sse_event("content_block_delta", &ev));
        }
    }

    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tool_calls {
            let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if tc.get("id").is_some() {
                stop_thinking_block_claude(state, &mut out);
                stop_text_block_claude(state, &mut out);
                let block_index = state.next_block_index;
                state.next_block_index += 1;
                state.tool_block_indices.insert(idx, block_index);
                let name = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
                let ev = serde_json::json!({
                    "type": "content_block_start",
                    "index": block_index,
                    "content_block": { "type": "tool_use", "id": id, "name": name, "input": {} }
                });
                out.push(format_sse_event("content_block_start", &ev));
            }
            if let Some(args) = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
            {
                if !args.is_empty() {
                    if let Some(&block_index) = state.tool_block_indices.get(&idx) {
                        let ev = serde_json::json!({
                            "type": "content_block_delta",
                            "index": block_index,
                            "delta": { "type": "input_json_delta", "partial_json": args }
                        });
                        out.push(format_sse_event("content_block_delta", &ev));
                    }
                }
            }
        }
    }

    if let Some(usage) = chunk.get("usage") {
        state.usage = Some(usage.clone());
    }

    if let Some(fr) = finish_reason {
        stop_thinking_block_claude(state, &mut out);
        stop_text_block_claude(state, &mut out);
        for &block_index in state.tool_block_indices.values() {
            let ev = serde_json::json!({ "type": "content_block_stop", "index": block_index });
            out.push(format_sse_event("content_block_stop", &ev));
        }
        let stop_reason = convert_openai_finish_to_claude(fr);
        let usage = state
            .usage
            .clone()
            .unwrap_or_else(|| serde_json::json!({ "input_tokens": 0, "output_tokens": 0 }));
        let ev = serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop_reason },
            "usage": usage
        });
        out.push(format_sse_event("message_delta", &ev));
        out.push(format_sse_event(
            "message_stop",
            &serde_json::json!({ "type": "message_stop" }),
        ));
    }
    out
}

fn openai_chunk_to_gemini_sse(chunk: &Value, state: &mut StreamState) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let choices = match chunk.get("choices").and_then(Value::as_array) {
        Some(c) if !c.is_empty() => c,
        _ => return out,
    };
    let choice = &choices[0];
    let delta = choice.get("delta").unwrap_or(&serde_json::Value::Null);
    let finish_reason = choice.get("finish_reason").and_then(Value::as_str);

    if state.message_id.is_none() {
        state.message_id = chunk.get("id").and_then(Value::as_str).map(String::from);
        state.model = chunk.get("model").and_then(Value::as_str).map(String::from);
    }

    let model = state.model.as_deref().unwrap_or("gemini");
    let mut parts: Vec<Value> = vec![];

    if let Some(r) = delta.get("reasoning_content").and_then(Value::as_str) {
        if !r.is_empty() {
            parts.push(serde_json::json!({ "text": r, "thought": true }));
        }
    }
    if let Some(c) = delta.get("content").and_then(Value::as_str) {
        if !c.is_empty() {
            parts.push(serde_json::json!({ "text": c }));
        }
    }
    if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tcs {
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let args_str = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let args_val: Value =
                serde_json::from_str(args_str).unwrap_or_else(|_| serde_json::json!({}));
            let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
            parts.push(serde_json::json!({
                "functionCall": { "name": name, "args": args_val, "id": id }
            }));
        }
    }

    if !parts.is_empty() || finish_reason.is_some() {
        let fr = finish_reason
            .map(|s| s.to_uppercase())
            .unwrap_or_else(|| "".to_string());
        let candidate = serde_json::json!({
            "content": { "parts": parts },
            "finishReason": fr
        });
        let payload = serde_json::json!({
            "candidates": [candidate],
            "modelVersion": model
        });
        out.push(format_sse_data(&payload));
    }
    out
}

fn openai_chunk_to_responses_sse(chunk: &Value, state: &mut StreamState) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let choices = match chunk.get("choices").and_then(Value::as_array) {
        Some(c) if !c.is_empty() => c,
        _ => return out,
    };
    let choice = &choices[0];
    let delta = choice.get("delta").unwrap_or(&serde_json::Value::Null);
    let finish_reason = choice.get("finish_reason").and_then(Value::as_str);
    let idx = choice.get("index").and_then(Value::as_u64).unwrap_or(0);

    let mut next_seq = || {
        state.responses_seq += 1;
        state.responses_seq
    };
    let response_id = chunk
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| state.message_id.clone())
        .unwrap_or_else(|| "resp_0".to_string());

    if !state.responses_started {
        state.responses_started = true;
        state.message_id = chunk
            .get("id")
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        let created = chunk.get("created").and_then(Value::as_u64).unwrap_or(0);
        let ev = serde_json::json!({
            "type": "response.created",
            "sequence_number": next_seq(),
            "response": {
                "id": response_id,
                "object": "response",
                "created_at": created,
                "status": "in_progress",
                "background": false,
                "error": null,
                "incomplete_details": null,
                "usage": null,
                "output": []
            }
        });
        out.push(format_sse_event("response.created", &ev));
        let ev2 = serde_json::json!({
            "type": "response.in_progress",
            "sequence_number": next_seq(),
            "response": {
                "id": response_id,
                "object": "response",
                "created_at": created,
                "status": "in_progress",
                "error": null,
                "incomplete_details": null,
                "usage": null
            }
        });
        out.push(format_sse_event("response.in_progress", &ev2));
    }

    if let Some(r) = delta
        .get("reasoning_content")
        .or(delta.get("reasoning"))
        .and_then(Value::as_str)
    {
        if !r.is_empty() {
            state.responses_reasoning_text.push_str(r);
            if !state.responses_reasoning_added {
                state.responses_reasoning_added = true;
                let item_id = state
                    .responses_reasoning_id
                    .clone()
                    .unwrap_or_else(|| format!("rs_{}", uuid::Uuid::new_v4().simple()));
                state.responses_reasoning_id = Some(item_id.clone());

                let added_ev = serde_json::json!({
                    "type": "response.output_item.added",
                    "sequence_number": next_seq(),
                    "response_id": response_id,
                    "output_index": idx,
                    "item": {
                        "id": item_id,
                        "type": "reasoning",
                        "summary": []
                    }
                });
                out.push(format_sse_event("response.output_item.added", &added_ev));

                let part_added_ev = serde_json::json!({
                    "type": "response.reasoning_summary_part.added",
                    "sequence_number": next_seq(),
                    "response_id": response_id,
                    "item_id": state.responses_reasoning_id,
                    "output_index": idx,
                    "summary_index": 0,
                    "part": { "type": "summary_text", "text": "" }
                });
                out.push(format_sse_event(
                    "response.reasoning_summary_part.added",
                    &part_added_ev,
                ));
            }
            let ev = serde_json::json!({
                "type": "response.reasoning_summary_text.delta",
                "sequence_number": next_seq(),
                "response_id": response_id,
                "item_id": state.responses_reasoning_id,
                "output_index": idx,
                "summary_index": 0,
                "delta": r
            });
            out.push(format_sse_event(
                "response.reasoning_summary_text.delta",
                &ev,
            ));
        }
    }
    if let Some(c) = delta.get("content").and_then(Value::as_str) {
        if !c.is_empty() {
            // Send response.output_item.added before the first content if not sent yet
            if !state.output_item_added {
                state.output_item_added = true;
                // Generate a message item ID if we don't have one
                let item_id = state.output_item_id.clone().unwrap_or_else(|| {
                    format!(
                        "msg_{}",
                        uuid::Uuid::new_v4()
                            .to_string()
                            .replace("-", "")
                            .split_at(8)
                            .0
                    )
                });
                state.output_item_id = Some(item_id.clone());

                let output_item_ev = serde_json::json!({
                    "type": "response.output_item.added",
                    "sequence_number": next_seq(),
                    "response_id": response_id,
                    "output_index": idx,
                    "item": {
                        "id": item_id,
                        "type": "message",
                        "status": "in_progress",
                        "role": "assistant",
                        "content": []
                    }
                });
                out.push(format_sse_event(
                    "response.output_item.added",
                    &output_item_ev,
                ));
            }
            // Track the full text so terminal events can include a complete message body.
            state.responses_output_text.push_str(c);

            // Send response.content_part.added before the first content delta if not sent yet.
            if !state.responses_content_part_added {
                state.responses_content_part_added = true;
                let item_id = state
                    .output_item_id
                    .clone()
                    .unwrap_or_else(|| "msg_0".to_string());
                let content_part_ev = serde_json::json!({
                    "type": "response.content_part.added",
                    "sequence_number": next_seq(),
                    "response_id": response_id,
                    "output_index": idx,
                    "content_index": 0,
                    "item_id": item_id,
                    "part": { "type": "output_text", "text": "", "annotations": [] }
                });
                out.push(format_sse_event(
                    "response.content_part.added",
                    &content_part_ev,
                ));
            }
            let item_id = state
                .output_item_id
                .clone()
                .unwrap_or_else(|| "msg_0".to_string());
            let ev = serde_json::json!({
                "type": "response.output_text.delta",
                "sequence_number": next_seq(),
                "response_id": response_id,
                "output_index": idx,
                "content_index": 0,
                "item_id": item_id,
                "delta": c
            });
            out.push(format_sse_event("response.output_text.delta", &ev));
        }
    }
    if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tcs {
            let tc_idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let entry = state
                .tool_calls
                .entry(tc_idx)
                .or_insert_with(|| ToolCallState {
                    index: tc_idx,
                    ..Default::default()
                });
            if let Some(id) = tc.get("id").cloned() {
                entry.id = Some(id);
            }
            if let Some(name) = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
            {
                entry.name = name.to_string();
            }
            if !entry.responses_item_added && entry.id.is_some() {
                entry.responses_item_added = true;
                let call_id = entry.id.as_ref().and_then(Value::as_str).unwrap_or("");
                let name = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let ev = serde_json::json!({
                    "type": "response.output_item.added",
                    "sequence_number": next_seq(),
                    "response_id": response_id,
                    "output_index": tc_idx,
                    "item": {
                        "id": format!("fc_{}", call_id),
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": ""
                    }
                });
                out.push(format_sse_event("response.output_item.added", &ev));
            }
            if let Some(args) = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
            {
                if !args.is_empty() {
                    entry.arguments.push_str(args);
                    let ev = serde_json::json!({
                        "type": "response.function_call_arguments.delta",
                        "sequence_number": next_seq(),
                        "response_id": response_id,
                        "call_id": entry.id.as_ref().and_then(Value::as_str),
                        "name": entry.name,
                        "item_id": entry
                            .id
                            .as_ref()
                            .and_then(Value::as_str)
                            .map(|call_id| format!("fc_{}", call_id)),
                        "output_index": tc_idx,
                        "delta": args
                    });
                    out.push(format_sse_event(
                        "response.function_call_arguments.delta",
                        &ev,
                    ));
                }
            }
        }
    }

    if let Some(u) = chunk.get("usage") {
        state.usage = Some(u.clone());
    }
    if finish_reason == Some("context_length_exceeded") {
        let failed = serde_json::json!({
            "type": "response.failed",
            "sequence_number": next_seq(),
            "response": {
                "id": response_id,
                "object": "response",
                "created_at": chunk.get("created").and_then(Value::as_u64).unwrap_or(0),
                "status": "failed",
                "background": false,
                "error": {
                    "type": "invalid_request_error",
                    "code": "context_length_exceeded",
                    "message": "Your input exceeds the context window of this model. Please adjust your input and try again."
                },
                "incomplete_details": null,
                "usage": null,
                "metadata": {}
            }
        });
        out.push(format_sse_event("response.failed", &failed));
        return out;
    }

    let incomplete_reason = match finish_reason {
        Some("length") => Some("max_output_tokens"),
        Some("content_filter") => Some("content_filter"),
        _ => None,
    };

    if finish_reason.is_some() {
        if state.responses_reasoning_added && !state.responses_reasoning_done {
            state.responses_reasoning_done = true;
            let item_id = state
                .responses_reasoning_id
                .clone()
                .unwrap_or_else(|| "rs_0".to_string());
            let text = state.responses_reasoning_text.clone();
            let text_done_ev = serde_json::json!({
                "type": "response.reasoning_summary_text.done",
                "sequence_number": next_seq(),
                "response_id": response_id,
                "item_id": item_id,
                "output_index": idx,
                "summary_index": 0,
                "text": text
            });
            out.push(format_sse_event(
                "response.reasoning_summary_text.done",
                &text_done_ev,
            ));

            let part_done_ev = serde_json::json!({
                "type": "response.reasoning_summary_part.done",
                "sequence_number": next_seq(),
                "response_id": response_id,
                "item_id": item_id,
                "output_index": idx,
                "summary_index": 0,
                "part": { "type": "summary_text", "text": state.responses_reasoning_text }
            });
            out.push(format_sse_event(
                "response.reasoning_summary_part.done",
                &part_done_ev,
            ));

            let output_item_done_ev = serde_json::json!({
                "type": "response.output_item.done",
                "sequence_number": next_seq(),
                "response_id": response_id,
                "output_index": idx,
                "item": {
                    "id": state.responses_reasoning_id,
                    "type": "reasoning",
                    "summary": [{ "type": "summary_text", "text": state.responses_reasoning_text }]
                }
            });
            out.push(format_sse_event(
                "response.output_item.done",
                &output_item_done_ev,
            ));
        }

        let mut tool_calls = state.tool_calls.values_mut().collect::<Vec<_>>();
        tool_calls.sort_by_key(|tc| tc.index);
        for tool_call in tool_calls {
            if tool_call.responses_item_added && !tool_call.responses_done {
                tool_call.responses_done = true;
                let Some(call_id) = tool_call.id.as_ref().and_then(Value::as_str) else {
                    continue;
                };
                let arguments = tool_call.arguments.clone();
                let output_index = tool_call.index;

                let args_done_ev = serde_json::json!({
                    "type": "response.function_call_arguments.done",
                    "sequence_number": next_seq(),
                    "response_id": response_id,
                    "call_id": call_id,
                    "name": tool_call.name,
                    "item_id": format!("fc_{}", call_id),
                    "output_index": output_index,
                    "arguments": arguments
                });
                out.push(format_sse_event(
                    "response.function_call_arguments.done",
                    &args_done_ev,
                ));

                let output_item_done_ev = serde_json::json!({
                    "type": "response.output_item.done",
                    "sequence_number": next_seq(),
                    "response_id": response_id,
                    "output_index": output_index,
                    "item": {
                        "id": format!("fc_{}", call_id),
                        "type": "function_call",
                        "call_id": call_id,
                        "name": tool_call.name,
                        "arguments": tool_call.arguments
                    }
                });
                out.push(format_sse_event(
                    "response.output_item.done",
                    &output_item_done_ev,
                ));
            }
        }

        // Send content_part.done before completed if we had text content
        if state.responses_content_part_added {
            let item_id = state
                .output_item_id
                .clone()
                .unwrap_or_else(|| "msg_0".to_string());
            let text = state.responses_output_text.clone();
            let text_done_ev = serde_json::json!({
                "type": "response.output_text.done",
                "sequence_number": next_seq(),
                "response_id": response_id,
                "output_index": idx,
                "content_index": 0,
                "item_id": item_id,
                "text": text
            });
            out.push(format_sse_event("response.output_text.done", &text_done_ev));

            let part_done_ev = serde_json::json!({
                "type": "response.content_part.done",
                "sequence_number": next_seq(),
                "response_id": response_id,
                "output_index": idx,
                "content_index": 0,
                "item_id": item_id,
                "part": {
                    "type": "output_text",
                    "text": state.responses_output_text,
                    "annotations": []
                }
            });
            out.push(format_sse_event(
                "response.content_part.done",
                &part_done_ev,
            ));
        }

        // Send response.output_item.done if we added an output item
        if state.output_item_added {
            let item_id = state
                .output_item_id
                .clone()
                .unwrap_or_else(|| "msg_0".to_string());
            let output_item_done_ev = serde_json::json!({
                "type": "response.output_item.done",
                "sequence_number": next_seq(),
                "response_id": response_id,
                "output_index": idx,
                "item": {
                    "id": item_id,
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": state.responses_output_text,
                        "annotations": []
                    }]
                }
            });
            out.push(format_sse_event(
                "response.output_item.done",
                &output_item_done_ev,
            ));
        }

        let created = chunk.get("created").and_then(Value::as_u64).unwrap_or(0);
        let mut output = Vec::new();
        if state.responses_reasoning_added {
            output.push(serde_json::json!({
                "id": state.responses_reasoning_id,
                "type": "reasoning",
                "summary": [{ "type": "summary_text", "text": state.responses_reasoning_text }]
            }));
        }
        if state.output_item_added {
            output.push(serde_json::json!({
                "id": state.output_item_id.clone().unwrap_or_else(|| "msg_0".to_string()),
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": state.responses_output_text,
                    "annotations": []
                }]
            }));
        }
        let mut tool_call_output = state.tool_calls.values().collect::<Vec<_>>();
        tool_call_output.sort_by_key(|tc| tc.index);
        for tool_call in tool_call_output {
            if let Some(call_id) = tool_call.id.as_ref().and_then(Value::as_str) {
                output.push(serde_json::json!({
                    "id": format!("fc_{}", call_id),
                    "type": "function_call",
                    "call_id": call_id,
                    "name": tool_call.name,
                    "arguments": tool_call.arguments
                }));
            }
        }

        let mut resp = serde_json::json!({
            "id": response_id,
            "object": "response",
            "created_at": created,
            "status": if incomplete_reason.is_some() { "incomplete" } else { "completed" },
            "error": null,
            "incomplete_details": incomplete_reason.map(|reason| serde_json::json!({ "reason": reason })).unwrap_or(serde_json::Value::Null),
            "output": output
        });
        if let Some(ref u) = state.usage {
            let input_tokens = u
                .get("input_tokens")
                .and_then(Value::as_u64)
                .or(u.get("prompt_tokens").and_then(Value::as_u64))
                .unwrap_or(0);
            let output_tokens = u
                .get("output_tokens")
                .and_then(Value::as_u64)
                .or(u.get("completion_tokens").and_then(Value::as_u64))
                .unwrap_or(0);
            let total_tokens = u
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(input_tokens + output_tokens);
            let cached_tokens = u
                .get("input_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
                .or_else(|| {
                    u.get("prompt_tokens_details")
                        .and_then(|details| details.get("cached_tokens"))
                        .and_then(Value::as_u64)
                });
            let reasoning_tokens = u
                .get("output_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
                .and_then(Value::as_u64)
                .or_else(|| {
                    u.get("completion_tokens_details")
                        .and_then(|details| details.get("reasoning_tokens"))
                        .and_then(Value::as_u64)
                });

            let mut usage = serde_json::json!({
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "total_tokens": total_tokens
            });
            if let Some(cached_tokens) = cached_tokens {
                usage["input_tokens_details"] =
                    serde_json::json!({ "cached_tokens": cached_tokens });
            }
            if let Some(reasoning_tokens) = reasoning_tokens {
                usage["output_tokens_details"] =
                    serde_json::json!({ "reasoning_tokens": reasoning_tokens });
            }
            resp["usage"] = usage;
        }
        let event_type = if incomplete_reason.is_some() {
            "response.incomplete"
        } else {
            "response.completed"
        };
        let ev = serde_json::json!({
            "type": event_type,
            "sequence_number": next_seq(),
            "response": resp
        });
        out.push(format_sse_event(event_type, &ev));
    }
    out
}

/// Translate a single SSE chunk from upstream format to client format.
/// Input is raw bytes (may be partial); call from a stream that buffers until full event.
pub fn translate_response_chunk(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    chunk: &[u8],
    state: &mut StreamState,
) -> Result<Vec<Vec<u8>>, String> {
    if upstream_format == client_format {
        return Ok(vec![chunk.to_vec()]);
    }
    let event: Value = serde_json::from_slice(chunk).map_err(|e| e.to_string())?;
    Ok(translate_sse_event(
        upstream_format,
        client_format,
        &event,
        state,
    ))
}

/// Stream that buffers upstream bytes, parses SSE events, and yields translated SSE bytes.
pub struct TranslateSseStream<S, E> {
    inner: S,
    buffer: Vec<u8>,
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    state: StreamState,
    output_queue: Vec<Vec<u8>>,
    output_pos: usize,
    _error: std::marker::PhantomData<E>,
}

impl<S, E> TranslateSseStream<S, E> {
    pub fn new(inner: S, upstream_format: UpstreamFormat, client_format: UpstreamFormat) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            upstream_format,
            client_format,
            state: StreamState::default(),
            output_queue: Vec::new(),
            output_pos: 0,
            _error: std::marker::PhantomData,
        }
    }
}

impl<S, E> Stream for TranslateSseStream<S, E>
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: Into<Box<dyn std::error::Error + Send + Sync>> + Unpin,
{
    type Item = Result<bytes::Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if this.output_pos < this.output_queue.len() {
                let next = this.output_queue[this.output_pos].clone();
                this.output_pos += 1;
                if this.output_pos >= this.output_queue.len() {
                    this.output_queue.clear();
                    this.output_pos = 0;
                }
                return Poll::Ready(Some(Ok(bytes::Bytes::from(next))));
            }

            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.buffer.extend_from_slice(&chunk);
                    while let Some(event) = take_one_sse_event(&mut this.buffer) {
                        let translated = translate_sse_event(
                            this.upstream_format,
                            this.client_format,
                            &event,
                            &mut this.state,
                        );
                        this.output_queue.extend(translated);
                    }
                    if !this.output_queue.is_empty() {
                        continue;
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(std::io::Error::other(e.into().to_string()))));
                }
                Poll::Ready(None) => {
                    while let Some(event) = take_one_sse_event(&mut this.buffer) {
                        let translated = translate_sse_event(
                            this.upstream_format,
                            this.client_format,
                            &event,
                            &mut this.state,
                        );
                        this.output_queue.extend(translated);
                    }
                    if !this.output_queue.is_empty() {
                        continue;
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::UpstreamFormat;

    #[test]
    fn take_one_sse_event_parses_data_line() {
        let mut buf = b"data: {\"type\":\"message_start\"}\n\n".to_vec();
        let event = take_one_sse_event(&mut buf);
        assert!(event.is_some());
        assert_eq!(
            event.as_ref().unwrap().get("type").and_then(Value::as_str),
            Some("message_start")
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn take_one_sse_event_skips_event_line() {
        let mut buf = b"event: message_start\ndata: {\"type\":\"message_start\"}\n\n".to_vec();
        let event = take_one_sse_event(&mut buf);
        assert!(event.is_some());
        assert_eq!(
            event.as_ref().unwrap().get("type").and_then(Value::as_str),
            Some("message_start")
        );
    }

    #[test]
    fn claude_message_start_produces_openai_chunk() {
        let event = serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        });
        let mut state = StreamState::default();
        let chunks = claude_event_to_openai_chunks(&event, &mut state);
        assert_eq!(chunks.len(), 1);
        assert_eq!(state.message_id.as_deref(), Some("msg_1"));
        assert!(chunks[0].get("choices").is_some());
        assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
    }

    #[test]
    fn test_format_sse_data() {
        let v = serde_json::json!({ "x": 1 });
        let bytes = format_sse_data(&v);
        assert!(bytes.starts_with(b"data: "));
        assert!(bytes.ends_with(b"\n\n"));
    }

    #[test]
    fn format_sse_event_includes_event_type() {
        let v = serde_json::json!({ "type": "message_start" });
        let bytes = format_sse_event("message_start", &v);
        assert!(bytes.starts_with(b"event: message_start\n"));
        assert!(bytes.windows(6).any(|w| w == b"data: "));
        assert!(bytes.ends_with(b"\n\n"));
    }

    #[test]
    fn gemini_event_with_text_produces_openai_chunks() {
        let event = serde_json::json!({
            "candidates": [{
                "content": { "parts": [{ "text": "Hello" }] },
                "finishReason": "STOP"
            }],
            "modelVersion": "gemini-1.5"
        });
        let mut state = StreamState::default();
        let chunks = gemini_event_to_openai_chunks(&event, &mut state);
        assert!(!chunks.is_empty());
        assert_eq!(state.model.as_deref(), Some("gemini-1.5"));
        let content_chunk = chunks
            .iter()
            .find(|c| c["choices"][0]["delta"].get("content").is_some());
        assert!(content_chunk.is_some());
        assert_eq!(
            content_chunk.unwrap()["choices"][0]["delta"]["content"],
            "Hello"
        );
    }

    #[test]
    fn gemini_thought_part_produces_openai_reasoning_chunk() {
        let event = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": "think",
                        "thought": true,
                        "thoughtSignature": "sig"
                    }]
                },
                "finishReason": "STOP"
            }]
        });
        let mut state = StreamState::default();
        let chunks = gemini_event_to_openai_chunks(&event, &mut state);
        assert!(chunks
            .iter()
            .any(|chunk| chunk["choices"][0]["delta"]["reasoning_content"] == "think"));
    }

    #[test]
    fn claude_thinking_delta_produces_openai_reasoning_chunk() {
        let mut state = StreamState::default();
        let _ = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-3" }
            }),
            &mut state,
        );
        let _ = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "thinking", "thinking": "" }
            }),
            &mut state,
        );
        let chunks = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "think" }
            }),
            &mut state,
        );
        assert!(chunks
            .iter()
            .any(|chunk| chunk["choices"][0]["delta"]["reasoning_content"] == "think"));
    }

    #[test]
    fn responses_event_output_text_delta_produces_openai_chunk() {
        let event = serde_json::json!({
            "type": "response.output_text.delta",
            "delta": "hi",
            "output_index": 0
        });
        let mut state = StreamState {
            message_id: Some("resp_1".to_string()),
            ..Default::default()
        };
        let chunks = responses_event_to_openai_chunks(&event, &mut state);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["delta"]["content"], "hi");
    }

    #[test]
    fn responses_event_created_inits_state_and_emits_role_chunk() {
        let event = serde_json::json!({
            "type": "response.created",
            "response": { "id": "resp_abc", "object": "response", "status": "in_progress" }
        });
        let mut state = StreamState::default();
        let chunks = responses_event_to_openai_chunks(&event, &mut state);
        assert_eq!(state.message_id.as_deref(), Some("resp_abc"));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
    }

    #[test]
    fn responses_reasoning_delta_produces_openai_reasoning_chunk() {
        let event = serde_json::json!({
            "type": "response.reasoning_summary_text.delta",
            "delta": "think"
        });
        let mut state = StreamState::default();
        let chunks = responses_event_to_openai_chunks(&event, &mut state);
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0]["choices"][0]["delta"]["reasoning_content"],
            "think"
        );
    }

    #[test]
    fn responses_incomplete_event_produces_openai_length_finish() {
        let event = serde_json::json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_1",
                "incomplete_details": { "reason": "max_output_tokens" },
                "usage": { "input_tokens": 1, "output_tokens": 2 }
            }
        });
        let mut state = StreamState::default();
        let chunks = responses_event_to_openai_chunks(&event, &mut state);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "length");
        assert_eq!(chunks[0]["usage"]["prompt_tokens"], 1);
    }

    #[test]
    fn responses_failed_context_window_event_produces_openai_error_finish() {
        let event = serde_json::json!({
            "type": "response.failed",
            "response": {
                "id": "resp_1",
                "error": { "code": "context_length_exceeded" }
            }
        });
        let mut state = StreamState::default();
        let chunks = responses_event_to_openai_chunks(&event, &mut state);
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0]["choices"][0]["finish_reason"],
            "context_length_exceeded"
        );
    }

    #[test]
    fn translate_sse_event_passthrough_openai_sends_done() {
        let event = serde_json::json!({ "_done": true });
        let mut state = StreamState::default();
        let out = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiCompletion,
            &event,
            &mut state,
        );
        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with(b"data: [DONE]"));
    }

    #[test]
    fn openai_chunk_to_claude_sse_emits_message_start_then_content_block() {
        let chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null }]
        });
        let mut state = StreamState::default();
        let out = openai_chunk_to_claude_sse(&chunk, &mut state);
        assert!(!out.is_empty());
        assert!(state.message_start_sent);
        let chunk2 = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
        });
        let out2 = openai_chunk_to_claude_sse(&chunk2, &mut state);
        assert!(!out2.is_empty());
        let has_content_block = out2
            .iter()
            .any(|b| String::from_utf8_lossy(b).contains("content_block"));
        assert!(has_content_block);
    }

    #[test]
    fn openai_chunk_to_claude_sse_emits_thinking_blocks() {
        let reasoning_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": { "reasoning_content": "think" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });
        let mut state = StreamState::default();
        let out1 = openai_chunk_to_claude_sse(&reasoning_chunk, &mut state);
        let out2 = openai_chunk_to_claude_sse(&finish_chunk, &mut state);
        let joined = out1
            .into_iter()
            .chain(out2)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"type\":\"thinking\""));
        assert!(joined.contains("thinking_delta"));
        assert!(joined.contains("message_stop"));
    }

    #[test]
    fn openai_chunk_to_gemini_sse_emits_thought_parts() {
        let chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{ "index": 0, "delta": { "reasoning_content": "think" }, "finish_reason": null }]
        });
        let mut state = StreamState::default();
        let out = openai_chunk_to_gemini_sse(&chunk, &mut state);
        let joined = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"thought\":true"));
        assert!(joined.contains("\"text\":\"think\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_emits_content_part_added_before_delta() {
        let mut state = StreamState::default();
        let role_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null }]
        });
        let text_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": "hello" }, "finish_reason": null }]
        });

        let _ = openai_chunk_to_responses_sse(&role_chunk, &mut state);
        let out = openai_chunk_to_responses_sse(&text_chunk, &mut state);
        let joined = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("event: response.content_part.added"));
        assert!(joined.contains("event: response.output_text.delta"));
        assert!(joined.contains("\"delta\":\"hello\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_includes_accumulated_text_in_done_events() {
        let mut state = StreamState::default();
        let role_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null }]
        });
        let text_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": "done-text" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "usage": { "prompt_tokens": 1, "completion_tokens": 2 },
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });

        let _ = openai_chunk_to_responses_sse(&role_chunk, &mut state);
        let _ = openai_chunk_to_responses_sse(&text_chunk, &mut state);
        let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"text\":\"done-text\""));
        assert!(joined.contains("\"output\":[{"));
    }

    #[test]
    fn openai_chunk_to_responses_sse_closes_function_calls() {
        let mut state = StreamState::default();
        let tool_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "lookup", "arguments": "{\"x\":1}" }
                    }]
                },
                "finish_reason": null
            }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
        });

        let _ = openai_chunk_to_responses_sse(&tool_chunk, &mut state);
        let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("response.function_call_arguments.done"));
        assert!(joined.contains("response.output_item.done"));
        assert!(joined.contains("\"type\":\"function_call\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_wraps_reasoning_with_item_lifecycle() {
        let mut state = StreamState::default();
        let reasoning_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": { "reasoning_content": "think" },
                "finish_reason": null
            }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });

        let out1 = openai_chunk_to_responses_sse(&reasoning_chunk, &mut state);
        let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out1
            .into_iter()
            .chain(out2)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("response.reasoning_summary_part.added"));
        assert!(joined.contains("response.reasoning_summary_text.delta"));
        assert!(joined.contains("response.reasoning_summary_text.done"));
        assert!(joined.contains("\"type\":\"reasoning\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_preserves_usage_details_and_total_tokens() {
        let mut state = StreamState::default();
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 25,
                "prompt_tokens_details": { "cached_tokens": 3 },
                "completion_tokens_details": { "reasoning_tokens": 2 }
            },
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });

        let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"total_tokens\":25"));
        assert!(joined.contains("\"input_tokens_details\":{\"cached_tokens\":3}"));
        assert!(joined.contains("\"output_tokens_details\":{\"reasoning_tokens\":2}"));
    }

    #[test]
    fn openai_chunk_to_responses_sse_includes_response_id_on_child_events() {
        let mut state = StreamState::default();
        let text_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });

        let out1 = openai_chunk_to_responses_sse(&text_chunk, &mut state);
        let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out1
            .into_iter()
            .chain(out2)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"type\":\"response.output_item.added\""));
        assert!(joined.contains("\"type\":\"response.content_part.added\""));
        assert!(joined.contains("\"type\":\"response.output_text.delta\""));
        assert!(joined.contains("\"type\":\"response.output_text.done\""));
        assert!(joined.contains("\"response_id\":\"chatcmpl-msg123\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_includes_call_metadata_on_function_events() {
        let mut state = StreamState::default();
        let tool_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "lookup", "arguments": "{\"x\":1}" }
                    }]
                },
                "finish_reason": null
            }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
        });

        let out1 = openai_chunk_to_responses_sse(&tool_chunk, &mut state);
        let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out1
            .into_iter()
            .chain(out2)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"type\":\"response.function_call_arguments.delta\""));
        assert!(joined.contains("\"type\":\"response.function_call_arguments.done\""));
        assert!(joined.contains("\"call_id\":\"call_1\""));
        assert!(joined.contains("\"name\":\"lookup\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_adds_empty_annotations_to_text_parts() {
        let mut state = StreamState::default();
        let text_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });

        let out1 = openai_chunk_to_responses_sse(&text_chunk, &mut state);
        let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out1
            .into_iter()
            .chain(out2)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"type\":\"response.content_part.added\""));
        assert!(joined.contains("\"type\":\"response.content_part.done\""));
        assert!(joined.contains("\"annotations\":[]"));
    }

    #[test]
    fn openai_chunk_to_responses_sse_includes_null_error_fields_on_response_events() {
        let mut state = StreamState::default();
        let text_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });

        let out1 = openai_chunk_to_responses_sse(&text_chunk, &mut state);
        let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out1
            .into_iter()
            .chain(out2)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"type\":\"response.created\""));
        assert!(joined.contains("\"type\":\"response.in_progress\""));
        assert!(joined.contains("\"type\":\"response.completed\""));
        assert!(joined.contains("\"error\":null"));
        assert!(joined.contains("\"incomplete_details\":null"));
    }

    #[test]
    fn openai_chunk_does_not_double_prefix_existing_chatcmpl_ids() {
        let state = StreamState {
            message_id: Some("chatcmpl-msg123".to_string()),
            ..Default::default()
        };
        let chunk = openai_chunk(&state, serde_json::json!({"content":"Hi"}), None);
        assert_eq!(chunk["id"], "chatcmpl-msg123");
    }

    #[test]
    fn claude_context_window_exceeded_maps_to_responses_failed_event() {
        let mut state = StreamState::default();
        let start = serde_json::json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "model": "glm-5"
            }
        });
        let delta = serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "model_context_window_exceeded" },
            "usage": { "input_tokens": 0, "output_tokens": 0 }
        });
        let stop = serde_json::json!({
            "type": "message_stop"
        });

        let mut out = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &start,
            &mut state,
        );
        out.extend(translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &delta,
            &mut state,
        ));
        out.extend(translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &stop,
            &mut state,
        ));

        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"type\":\"response.failed\""));
        assert!(joined.contains("\"code\":\"context_length_exceeded\""));
        assert!(!joined.contains("\"type\":\"response.completed\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_maps_length_finish_to_incomplete() {
        let mut state = StreamState::default();
        let text_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }]
        });

        let out1 = openai_chunk_to_responses_sse(&text_chunk, &mut state);
        let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out1
            .into_iter()
            .chain(out2)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"type\":\"response.incomplete\""));
        assert!(joined.contains("\"reason\":\"max_output_tokens\""));
        assert!(!joined.contains("\"type\":\"response.completed\""));
    }
}
