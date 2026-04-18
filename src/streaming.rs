//! SSE streaming: passthrough when formats match, otherwise transform chunks (upstream → openai → client).
//!
//! Reference: 9router open-sse/handlers/chatCore/streamingHandler.js, utils/stream.js.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use serde_json::Value;

use crate::formats::UpstreamFormat;
use crate::translate::{
    anthropic_tool_use_type_for_openai_tool_call, classify_openai_finish_for_anthropic,
    classify_portable_non_success_terminal, gemini_finish_reason_to_openai,
    responses_failed_code_to_openai_finish, AnthropicTerminal,
    OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE,
};

/// Whether we need to transform the upstream SSE stream for the client.
pub fn needs_stream_translation(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
) -> bool {
    upstream_format != client_format
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFatalRejection {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClaudeBlockKind {
    Text,
    Thinking,
    ToolUse,
    ServerToolUse,
}

#[derive(Debug, Clone, Default)]
struct ClaudeBlockState {
    kind: Option<ClaudeBlockKind>,
    signature: Option<String>,
    annotations: Vec<Value>,
    omitted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ClaudeThinkingProvenanceState {
    block_index: usize,
    signature: Option<String>,
    omitted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponsesMessagePartKind {
    OutputText,
    Refusal,
}

#[derive(Debug, Clone, Default)]
struct ResponsesMessagePartState {
    kind: Option<ResponsesMessagePartKind>,
    text: String,
    annotations: Vec<Value>,
}

/// Stream transformer state (per 9router initState).
#[derive(Debug, Default)]
pub struct StreamState {
    pub message_id: Option<String>,
    pub model: Option<String>,
    pub openai_tool_call_index: usize,
    pub claude_tool_use_index: usize,
    pub openai_tool_calls: std::collections::HashMap<usize, ToolCallState>,
    pub claude_tool_uses: std::collections::HashMap<usize, ClaudeToolUseState>,
    claude_blocks: std::collections::HashMap<usize, ClaudeBlockState>,
    claude_thinking_provenance: Vec<ClaudeThinkingProvenanceState>,
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
    pub fatal_rejection: Option<StreamFatalRejection>,
    pub tool_block_indices: std::collections::HashMap<usize, usize>,
    // Gemini state
    pub function_index: usize,
    pub gemini_dummy_signature_emitted: bool,
    gemini_candidate_index: Option<usize>,
    openai_choice_index: Option<usize>,
    pub openai_role_sent: bool,
    // OpenAI Responses API client output state
    pub responses_seq: u64,
    pub responses_started: bool,
    pub output_item_id: Option<String>,
    pub output_item_added: bool,
    pub responses_content_part_added: bool,
    pub responses_output_text: String,
    responses_message_parts: Vec<ResponsesMessagePartState>,
    responses_text_part_index: Option<usize>,
    responses_refusal_part_index: Option<usize>,
    pub responses_reasoning_id: Option<String>,
    pub responses_reasoning_added: bool,
    pub responses_reasoning_done: bool,
    pub responses_reasoning_text: String,
    pub responses_next_output_index: u64,
    pub responses_reasoning_output_index: Option<u64>,
    pub responses_message_output_index: Option<u64>,
    pub openai_seen_content: String,
    pub openai_seen_refusal: String,
    pub openai_seen_reasoning: String,
    pub openai_terminal_error: Option<Value>,
    pub responses_terminal_sent: bool,
    pub gemini_next_tool_call_to_emit: usize,
}

#[derive(Debug, Default)]
pub struct ToolCallState {
    pub index: usize,
    pub id: Option<Value>,
    pub name: String,
    pub arguments: String,
    pub tool_type: Option<String>,
    pub proxied_tool_kind: Option<String>,
    pub gemini_emitted_arguments: Option<String>,
    pub arguments_seeded_from_start: bool,
    pub block_index: Option<usize>,
    pub responses_item_id: Option<String>,
    pub responses_item_added: bool,
    pub responses_done: bool,
}

#[derive(Debug, Default)]
pub struct ClaudeToolUseState {
    pub openai_index: usize,
    pub id: Option<Value>,
    pub name: String,
    pub arguments: String,
    pub arguments_seeded_from_start: bool,
    pub start_arguments_emitted: bool,
}

fn dedupe_tool_call_state_by_call_id(
    tool_calls: &mut std::collections::HashMap<usize, ToolCallState>,
    tc_idx: usize,
    incoming_id: Option<&Value>,
) {
    let Some(incoming_id) = incoming_id else {
        return;
    };
    let existing_key = tool_calls.iter().find_map(|(key, entry)| {
        ((*key != tc_idx) && entry.id.as_ref() == Some(incoming_id)).then_some(*key)
    });
    let Some(existing_key) = existing_key else {
        return;
    };
    let Some(mut existing_entry) = tool_calls.remove(&existing_key) else {
        return;
    };
    existing_entry.index = tc_idx;
    tool_calls
        .entry(tc_idx)
        .and_modify(|entry| {
            if entry.id.is_none() {
                entry.id = existing_entry.id.clone();
            }
            if entry.name.is_empty() {
                entry.name = existing_entry.name.clone();
            }
            if entry.arguments.is_empty() {
                entry.arguments = existing_entry.arguments.clone();
            }
            if entry.tool_type.is_none() {
                entry.tool_type = existing_entry.tool_type.clone();
            }
            if entry.proxied_tool_kind.is_none() {
                entry.proxied_tool_kind = existing_entry.proxied_tool_kind.clone();
            }
            if entry.gemini_emitted_arguments.is_none() {
                entry.gemini_emitted_arguments = existing_entry.gemini_emitted_arguments.clone();
            }
            if entry.block_index.is_none() {
                entry.block_index = existing_entry.block_index;
            }
            if entry.responses_item_id.is_none() {
                entry.responses_item_id = existing_entry.responses_item_id.clone();
            }
            entry.responses_item_added |= existing_entry.responses_item_added;
            entry.responses_done |= existing_entry.responses_done;
        })
        .or_insert(existing_entry);
}

fn merge_seeded_tool_arguments(seed: &str, delta: &str) -> String {
    if delta.is_empty() {
        return seed.to_string();
    }

    for trim in 0..=seed.len() {
        let end = seed.len().saturating_sub(trim);
        if !seed.is_char_boundary(end) {
            continue;
        }
        let candidate = format!("{}{}", &seed[..end], delta);
        if serde_json::from_str::<Value>(&candidate).is_ok() {
            return candidate;
        }
    }

    format!("{seed}{delta}")
}

fn openai_stream_tool_call_type(value: &Value) -> &'static str {
    match value.get("type").and_then(Value::as_str) {
        Some("custom") | Some("custom_tool_call") => "custom",
        _ => "function",
    }
}

fn gemini_candidate_index(candidate: &Value) -> usize {
    candidate
        .get("index")
        .or_else(|| candidate.get("candidateIndex"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize
}

fn tool_call_state_type(state: &ToolCallState) -> &str {
    state.tool_type.as_deref().unwrap_or("function")
}

fn responses_tool_call_item_type(tool_type: &str) -> &'static str {
    if tool_type == "custom" {
        "custom_tool_call"
    } else {
        "function_call"
    }
}

fn responses_tool_call_delta_event_type(tool_type: &str) -> &'static str {
    if tool_type == "custom" {
        "response.custom_tool_call_input.delta"
    } else {
        "response.function_call_arguments.delta"
    }
}

fn responses_tool_call_done_event_type(tool_type: &str) -> &'static str {
    if tool_type == "custom" {
        "response.custom_tool_call_input.done"
    } else {
        "response.function_call_arguments.done"
    }
}

fn responses_tool_call_payload_field(tool_type: &str) -> &'static str {
    if tool_type == "custom" {
        "input"
    } else {
        "arguments"
    }
}

fn openai_usage_to_anthropic_usage_stream(usage: &Value) -> Value {
    serde_json::json!({
        "input_tokens": usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
        "output_tokens": usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0)
    })
}

fn copy_unknown_usage_fields(
    source: &serde_json::Map<String, Value>,
    target: &mut Value,
    reserved_keys: &[&str],
) {
    for (key, value) in source {
        if reserved_keys.contains(&key.as_str()) || target.get(key).is_some() {
            continue;
        }
        target[key] = value.clone();
    }
}

/// Extract one SSE event from buffer. Returns parsed JSON from "data: " line, or None.
/// Buffer is updated: consumed bytes are removed.
///
/// Supports both `\n\n` and `\r\n\r\n` line endings, since some upstream servers
/// (e.g., vLLM/uvicorn) emit SSE with CRLF separators.
pub fn take_one_sse_event(buffer: &mut Vec<u8>) -> Option<Value> {
    // Try CRLF first (\r\n\r\n), then LF (\n\n)
    let pos = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 2) // point at the second \r\n so drain removes all 4 bytes
        .or_else(|| buffer.windows(2).position(|w| w == b"\n\n"))?;
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
    let mut out = format!("event: {event_type}\n").into_bytes();
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(s.as_bytes());
    out.extend_from_slice(b"\n\n");
    out
}

/// Convert Claude SSE event to one or more OpenAI-format chunks. Updates state.
pub fn claude_event_to_openai_chunks(event: &Value, state: &mut StreamState) -> Vec<Value> {
    if state.fatal_rejection.is_some() {
        return vec![];
    }
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
            state.claude_tool_use_index = 0;
            state.claude_tool_uses.clear();
            state.claude_blocks.clear();
            state.claude_thinking_provenance.clear();
            emit_openai_assistant_role_if_needed(state, &mut out);
        }
        Some("content_block_start") => {
            let block = event.get("content_block");
            let block_ty = block.and_then(|b| b.get("type").and_then(Value::as_str));
            let idx = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            match block_ty {
                Some("text") => {
                    state.claude_blocks.insert(
                        idx,
                        ClaudeBlockState {
                            kind: Some(ClaudeBlockKind::Text),
                            ..Default::default()
                        },
                    );
                    state.text_block_started = true;
                }
                Some("thinking") => {
                    let omitted = block
                        .and_then(|b| {
                            b.get("thinking")
                                .and_then(|thinking| thinking.get("display"))
                                .or_else(|| b.get("display"))
                        })
                        .and_then(Value::as_str)
                        == Some("omitted");
                    state.claude_blocks.insert(
                        idx,
                        ClaudeBlockState {
                            kind: Some(ClaudeBlockKind::Thinking),
                            omitted,
                            ..Default::default()
                        },
                    );
                    state.in_thinking_block = true;
                    state.current_block_index = event
                        .get("index")
                        .and_then(Value::as_u64)
                        .map(|i| i as usize);
                }
                Some("tool_use") | Some("server_tool_use") => {
                    let block = block.unwrap();
                    let tc_index = state.claude_tool_use_index;
                    state.claude_tool_use_index += 1;
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let seeded_arguments = block
                        .get("input")
                        .filter(|input| !input.is_null())
                        .filter(|input| !matches!(input, Value::Object(map) if map.is_empty()))
                        .and_then(|input| serde_json::to_string(input).ok())
                        .filter(|serialized| serialized != "null" && serialized != "{}")
                        .unwrap_or_default();
                    let mut tc = serde_json::json!({
                        "index": tc_index,
                        "id": block.get("id"),
                        "type": "function",
                        "function": { "name": name, "arguments": seeded_arguments }
                    });
                    if block_ty == Some("server_tool_use") {
                        tc["proxied_tool_kind"] =
                            Value::String("anthropic_server_tool_use".to_string());
                    }
                    state.claude_blocks.insert(
                        idx,
                        ClaudeBlockState {
                            kind: Some(if block_ty == Some("server_tool_use") {
                                ClaudeBlockKind::ServerToolUse
                            } else {
                                ClaudeBlockKind::ToolUse
                            }),
                            ..Default::default()
                        },
                    );
                    state.claude_tool_uses.insert(
                        idx,
                        ClaudeToolUseState {
                            openai_index: tc_index,
                            id: block.get("id").cloned(),
                            name: name.clone(),
                            arguments: seeded_arguments.clone(),
                            arguments_seeded_from_start: !seeded_arguments.is_empty(),
                            start_arguments_emitted: !seeded_arguments.is_empty(),
                        },
                    );
                    emit_openai_assistant_role_if_needed(state, &mut out);
                    out.push(openai_chunk(
                        state,
                        serde_json::json!({ "tool_calls": [tc] }),
                        None,
                    ));
                }
                Some(other) => {
                    let message = if other == "server_tool_result" {
                        "Anthropic server_tool_result blocks cannot be translated losslessly."
                            .to_string()
                    } else {
                        format!(
                            "Anthropic content block type `{other}` cannot be translated losslessly."
                        )
                    };
                    return reject_openai_stream(
                        state,
                        "invalid_request_error",
                        "unsupported_anthropic_stream_event",
                        message,
                    );
                }
                None => {
                    return reject_openai_stream(
                        state,
                        "invalid_request_error",
                        "unsupported_anthropic_stream_event",
                        "Anthropic content block start event is missing a block type.",
                    );
                }
            }
        }
        Some("content_block_delta") => {
            let idx = event
                .get("index")
                .and_then(Value::as_u64)
                .map(|i| i as usize);
            let delta = event.get("delta");
            let delta_ty = delta.and_then(|d| d.get("type").and_then(Value::as_str));
            match delta_ty {
                Some("text_delta") => {
                    if let Some(t) = delta.and_then(|d| d.get("text").and_then(Value::as_str)) {
                        if !t.is_empty() {
                            emit_openai_assistant_role_if_needed(state, &mut out);
                            out.push(openai_chunk(
                                state,
                                serde_json::json!({ "content": t }),
                                None,
                            ));
                        }
                    }
                }
                Some("thinking_delta") => {
                    if let Some(t) = delta.and_then(|d| d.get("thinking").and_then(Value::as_str))
                    {
                        let omitted = idx
                            .and_then(|block_index| state.claude_blocks.get(&block_index))
                            .map(|block| block.omitted)
                            .unwrap_or(false);
                        if !t.is_empty() && !omitted {
                            emit_openai_assistant_role_if_needed(state, &mut out);
                            out.push(openai_chunk(
                                state,
                                serde_json::json!({ "reasoning_content": t }),
                                None,
                            ));
                        }
                    }
                }
                Some("input_json_delta") => {
                    if let Some(pj) =
                        delta.and_then(|d| d.get("partial_json").and_then(Value::as_str))
                    {
                        let chunk_json =
                            if let Some(tc) = idx.and_then(|i| state.claude_tool_uses.get_mut(&i))
                            {
                                let delta_to_emit =
                                    if tc.arguments_seeded_from_start && !tc.arguments.is_empty() {
                                        tc.arguments =
                                            merge_seeded_tool_arguments(&tc.arguments, pj);
                                        tc.arguments_seeded_from_start = false;
                                        pj.to_string()
                                    } else {
                                        tc.arguments.push_str(pj);
                                        pj.to_string()
                                    };
                                Some(serde_json::json!({
                                        "tool_calls": [{
                                            "index": tc.openai_index,
                                            "id": tc.id,
                                            "function": { "arguments": delta_to_emit }
                                        }]
                                }))
                            } else {
                                None
                            };
                        if let Some(cj) = chunk_json {
                            emit_openai_assistant_role_if_needed(state, &mut out);
                            out.push(openai_chunk(state, cj, None));
                        }
                    }
                }
                Some("signature_delta") => {
                    let Some(block_index) = idx else {
                        return reject_openai_stream(
                            state,
                            "invalid_request_error",
                            "unsupported_anthropic_stream_event",
                            "Anthropic signature_delta is missing a block index.",
                        );
                    };
                    let Some(block_state) = state.claude_blocks.get_mut(&block_index) else {
                        return reject_openai_stream(
                            state,
                            "invalid_request_error",
                            "unsupported_anthropic_stream_event",
                            "Anthropic signature_delta referenced an unknown block.",
                        );
                    };
                    if block_state.kind != Some(ClaudeBlockKind::Thinking) {
                        return reject_openai_stream(
                            state,
                            "invalid_request_error",
                            "unsupported_anthropic_stream_event",
                            "Anthropic signature_delta is only valid for thinking blocks.",
                        );
                    }
                    if let Some(signature) =
                        delta.and_then(|d| d.get("signature").and_then(Value::as_str))
                    {
                        block_state.signature = Some(signature.to_string());
                    }
                }
                Some("citations_delta") => {
                    let Some(block_index) = idx else {
                        return reject_openai_stream(
                            state,
                            "invalid_request_error",
                            "unsupported_anthropic_stream_event",
                            "Anthropic citations_delta is missing a block index.",
                        );
                    };
                    let Some(block_state) = state.claude_blocks.get_mut(&block_index) else {
                        return reject_openai_stream(
                            state,
                            "invalid_request_error",
                            "unsupported_anthropic_stream_event",
                            "Anthropic citations_delta referenced an unknown block.",
                        );
                    };
                    if block_state.kind != Some(ClaudeBlockKind::Text) {
                        return reject_openai_stream(
                            state,
                            "invalid_request_error",
                            "unsupported_anthropic_stream_event",
                            "Anthropic citations_delta is only valid for text blocks.",
                        );
                    }
                    let mut annotations = delta
                        .and_then(|d| d.get("citations").and_then(Value::as_array))
                        .cloned()
                        .unwrap_or_default();
                    if let Some(citation) = delta.and_then(|d| d.get("citation")).cloned() {
                        annotations.push(citation);
                    }
                    if !annotations.is_empty() {
                        block_state.annotations.extend(annotations.clone());
                        emit_openai_assistant_role_if_needed(state, &mut out);
                        out.push(openai_chunk(
                            state,
                            serde_json::json!({ "annotations": annotations }),
                            None,
                        ));
                    }
                }
                Some(other) => {
                    return reject_openai_stream(
                        state,
                        "invalid_request_error",
                        "unsupported_anthropic_stream_event",
                        format!(
                            "Anthropic content block delta `{other}` cannot be translated losslessly."
                        ),
                    );
                }
                None => {
                    return reject_openai_stream(
                        state,
                        "invalid_request_error",
                        "unsupported_anthropic_stream_event",
                        "Anthropic content block delta is missing a delta type.",
                    );
                }
            }
        }
        Some("content_block_stop") => {
            let idx = event
                .get("index")
                .and_then(Value::as_u64)
                .map(|i| i as usize);
            let seeded_tool_chunk = if let Some(i) = idx {
                if let Some(tc) = state.claude_tool_uses.get_mut(&i) {
                    if tc.arguments_seeded_from_start
                        && !tc.start_arguments_emitted
                        && !tc.arguments.is_empty()
                    {
                        tc.arguments_seeded_from_start = false;
                        Some(serde_json::json!({
                            "tool_calls": [{
                                "index": tc.openai_index,
                                "id": tc.id,
                                "function": { "arguments": tc.arguments.clone() }
                            }]
                        }))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(chunk_json) = seeded_tool_chunk {
                out.push(openai_chunk(state, chunk_json, None));
            }
            if let Some(i) = idx {
                if let Some(block_state) = state.claude_blocks.get(&i) {
                    if block_state.kind == Some(ClaudeBlockKind::Thinking) {
                        state
                            .claude_thinking_provenance
                            .retain(|entry| entry.block_index != i);
                        state
                            .claude_thinking_provenance
                            .push(ClaudeThinkingProvenanceState {
                                block_index: i,
                                signature: block_state.signature.clone(),
                                omitted: block_state.omitted,
                            });
                    }
                }
                state.claude_blocks.remove(&i);
            }
            if state.in_thinking_block && state.current_block_index == idx {
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
                    if state.claude_tool_uses.is_empty() {
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
                    if let Some(extra_fields) = u.as_object() {
                        for (key, value) in extra_fields {
                            if usage_json.get(key).is_none() {
                                usage_json[key] = value.clone();
                            }
                        }
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
                format!("chatcmpl-{s}")
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

fn emit_openai_assistant_role_if_needed(state: &mut StreamState, out: &mut Vec<Value>) {
    if state.openai_role_sent {
        return;
    }
    out.push(openai_chunk(
        state,
        serde_json::json!({ "role": "assistant" }),
        None,
    ));
    state.openai_role_sent = true;
}

fn mark_stream_fatal_rejection(
    state: &mut StreamState,
    message: impl Into<String>,
) -> String {
    let message = message.into();
    if state.fatal_rejection.is_none() {
        state.fatal_rejection = Some(StreamFatalRejection {
            message: message.clone(),
        });
    }
    message
}

fn reject_openai_stream(
    state: &mut StreamState,
    error_type: &str,
    code: &str,
    message: impl Into<String>,
) -> Vec<Value> {
    let message = mark_stream_fatal_rejection(state, message);
    state.finish_reason = Some("error".to_string());
    state.finish_reason_sent = true;
    let mut chunk = openai_chunk(state, serde_json::json!({}), Some("error"));
    if let Some(ref usage) = state.usage {
        chunk["usage"] = usage.clone();
    }
    chunk["error"] = serde_json::json!({
        "type": error_type,
        "code": code,
        "message": message
    });
    vec![chunk]
}

fn convert_claude_stop_reason(r: &str) -> String {
    match r {
        "end_turn" => "stop",
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        "stop_sequence" => "stop",
        "pause_turn" => "pause_turn",
        "refusal" => "content_filter",
        "model_context_window_exceeded" => "context_length_exceeded",
        _ => "stop",
    }
    .to_string()
}

fn gemini_finish_reason_to_openai_stream(finish_reason: &str, has_tool_calls: bool) -> String {
    gemini_finish_reason_to_openai(Some(finish_reason), has_tool_calls)
}

fn openai_finish_reason_to_gemini_stream(finish_reason: &str) -> &'static str {
    match finish_reason {
        "stop" | "tool_calls" => "STOP",
        "length" => "MAX_TOKENS",
        "content_filter" => "SAFETY",
        "pause_turn" | "context_length_exceeded" | "tool_error" | "error" => "OTHER",
        _ => "STOP",
    }
}

const GEMINI_DUMMY_THOUGHT_SIGNATURE_STREAM: &str = "skip_thought_signature_validator";

fn responses_failed_code_to_openai_finish_stream(code: Option<&str>) -> &'static str {
    responses_failed_code_to_openai_finish(code)
}

fn responses_usage_to_openai_usage_stream(usage: &Value) -> Value {
    let input_tokens = usage
        .get("input_tokens")
        .or(usage.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .or(usage.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens + output_tokens);

    let mut mapped = serde_json::json!({
        "prompt_tokens": input_tokens,
        "completion_tokens": output_tokens,
        "total_tokens": total_tokens
    });

    if let Some(cached_tokens) = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
        })
    {
        mapped["prompt_tokens_details"] = serde_json::json!({ "cached_tokens": cached_tokens });
    }

    if let Some(reasoning_tokens) = usage
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
                .and_then(Value::as_u64)
        })
    {
        mapped["completion_tokens_details"] =
            serde_json::json!({ "reasoning_tokens": reasoning_tokens });
    }

    if let Some(obj) = usage.as_object() {
        copy_unknown_usage_fields(
            obj,
            &mut mapped,
            &[
                "input_tokens",
                "prompt_tokens",
                "output_tokens",
                "completion_tokens",
                "total_tokens",
                "input_tokens_details",
                "prompt_tokens_details",
                "output_tokens_details",
                "completion_tokens_details",
            ],
        );
    }

    mapped
}

fn openai_usage_to_responses_usage_stream(usage: &Value) -> Value {
    let input_tokens = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens + output_tokens);

    let mut mapped = serde_json::json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens
    });

    if let Some(details) = usage.get("prompt_tokens_details") {
        if let Some(cached) = details.get("cached_tokens").and_then(Value::as_u64) {
            mapped["input_tokens_details"] = serde_json::json!({ "cached_tokens": cached });
        }
    }
    if let Some(details) = usage.get("completion_tokens_details") {
        if let Some(reasoning) = details.get("reasoning_tokens").and_then(Value::as_u64) {
            mapped["output_tokens_details"] = serde_json::json!({ "reasoning_tokens": reasoning });
        }
    }

    if let Some(obj) = usage.as_object() {
        copy_unknown_usage_fields(
            obj,
            &mut mapped,
            &[
                "prompt_tokens",
                "completion_tokens",
                "total_tokens",
                "prompt_tokens_details",
                "completion_tokens_details",
            ],
        );
    }

    mapped
}

fn responses_event_tool_call_index(event: &Value, state: &StreamState) -> Option<usize> {
    if let Some(item_id) = event.get("item_id").and_then(Value::as_str) {
        if let Some((idx, _)) = state
            .openai_tool_calls
            .iter()
            .find(|(_, tool_call)| tool_call.responses_item_id.as_deref() == Some(item_id))
        {
            return Some(*idx);
        }
    }
    event
        .get("output_index")
        .and_then(Value::as_u64)
        .and_then(|output_index| {
            state.openai_tool_calls.iter().find_map(|(idx, tool_call)| {
                (tool_call.block_index == Some(output_index as usize)).then_some(*idx)
            })
        })
}

/// If event is OpenAI chunk (has choices[].delta), return as single-item vec. Else return empty.
pub fn openai_event_as_chunk(event: &Value) -> Option<Value> {
    if event.get("_done").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    if event.get("usage").is_some() {
        return Some(event.clone());
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

fn reject_openai_multi_choice_for_non_openai_sink(state: &mut StreamState) -> Vec<Value> {
    reject_openai_stream(
        state,
        "invalid_request_error",
        "unsupported_openai_stream_event",
        "OpenAI streaming response with multiple choices cannot be translated losslessly.",
    )
}

fn ensure_single_openai_choice_for_non_openai_sink(
    chunk: &Value,
    state: &mut StreamState,
) -> Result<(), Vec<Value>> {
    if state.message_id.is_none() {
        state.message_id = chunk.get("id").and_then(Value::as_str).map(String::from);
    }
    if state.model.is_none() {
        state.model = chunk.get("model").and_then(Value::as_str).map(String::from);
    }
    if let Some(usage) = chunk.get("usage") {
        state.usage = Some(usage.clone());
    }

    let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
        return Ok(());
    };
    if choices.is_empty() {
        return Ok(());
    }
    if choices.len() > 1 {
        return Err(reject_openai_multi_choice_for_non_openai_sink(state));
    }

    let choice_index = choices[0]
        .get("index")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    match state.openai_choice_index {
        Some(previous) if previous != choice_index || choice_index != 0 => {
            Err(reject_openai_multi_choice_for_non_openai_sink(state))
        }
        None if choice_index != 0 => Err(reject_openai_multi_choice_for_non_openai_sink(state)),
        None => {
            state.openai_choice_index = Some(choice_index);
            Ok(())
        }
        Some(_) => Ok(()),
    }
}

fn unsupported_gemini_output_part_kind(part: &Value) -> Option<String> {
    part.as_object().and_then(|obj| {
        obj.iter().find_map(|(key, value)| {
            (!value.is_null()
                && !matches!(
                    key.as_str(),
                    "text" | "functionCall" | "thought" | "thoughtSignature" | "thought_signature"
                ))
            .then(|| key.clone())
        })
    })
}

fn anthropic_stream_incompatibility_for_non_anthropic_sink(event: &Value) -> Option<&'static str> {
    match event.get("type").and_then(Value::as_str) {
        Some("content_block_start") => {
            let content_block = event.get("content_block")?;
            if content_block.get("type").and_then(Value::as_str) != Some("thinking") {
                return None;
            }
            let omitted = content_block
                .get("thinking")
                .and_then(|thinking| thinking.get("display"))
                .or_else(|| content_block.get("display"))
                .and_then(Value::as_str)
                == Some("omitted");
            Some(if omitted {
                "Anthropic omitted thinking blocks cannot be translated losslessly."
            } else {
                "Anthropic thinking blocks cannot be translated losslessly."
            })
        }
        Some("content_block_delta") => (event
            .get("delta")
            .and_then(|delta| delta.get("type"))
            .and_then(Value::as_str)
            == Some("signature_delta"))
        .then_some("Anthropic thinking signature provenance cannot be translated losslessly."),
        _ => None,
    }
}

/// Convert Gemini SSE event (response with candidates[0].content.parts) to OpenAI-format chunks.
pub fn gemini_event_to_openai_chunks(event: &Value, state: &mut StreamState) -> Vec<Value> {
    if state.fatal_rejection.is_some() {
        return vec![];
    }
    let response = event.get("response").unwrap_or(event);
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

        if cached_tokens > 0 {
            usage_json["prompt_tokens_details"] = serde_json::json!({
                "cached_tokens": cached_tokens
            });
        }
        if thoughts_tokens > 0 {
            usage_json["completion_tokens_details"] = serde_json::json!({
                "reasoning_tokens": thoughts_tokens
            });
        }

        state.usage = Some(usage_json);
    }

    let candidates = match response.get("candidates").and_then(Value::as_array) {
        Some(c) if c.len() > 1 => {
            return reject_openai_stream(
                state,
                "invalid_request_error",
                "unsupported_gemini_stream_event",
                "Gemini streaming response with multiple candidates cannot be translated losslessly.",
            );
        }
        Some(c) if !c.is_empty() => c,
        _ => {
            if let Some(block_reason) = response
                .get("promptFeedback")
                .and_then(|feedback| feedback.get("blockReason"))
                .and_then(Value::as_str)
            {
                let finish_reason = gemini_finish_reason_to_openai_stream(
                    block_reason,
                    !state.openai_tool_calls.is_empty(),
                );
                let mut chunk = openai_chunk(state, serde_json::json!({}), Some(&finish_reason));
                if let Some(ref u) = state.usage {
                    chunk["usage"] = u.clone();
                }
                out.push(chunk);
                state.finish_reason = Some(finish_reason);
                state.finish_reason_sent = true;
                return out;
            }
            return reject_openai_stream(
                state,
                "invalid_request_error",
                "unsupported_gemini_stream_event",
                "Gemini streaming response omitted candidates without a terminal block reason.",
            );
        }
    };
    let candidate_indices = candidates
        .iter()
        .map(gemini_candidate_index)
        .collect::<Vec<_>>();
    if candidate_indices.iter().any(|index| *index != 0) {
        return reject_openai_stream(
            state,
            "invalid_request_error",
            "unsupported_gemini_stream_event",
            "Gemini streaming response with multiple candidates cannot be translated losslessly.",
        );
    }
    if let Some(previous) = state.gemini_candidate_index {
        if candidate_indices.iter().any(|index| *index != previous) {
            return reject_openai_stream(
                state,
                "invalid_request_error",
                "unsupported_gemini_stream_event",
                "Gemini streaming response with multiple candidates cannot be translated losslessly.",
            );
        }
    } else {
        state.gemini_candidate_index = candidate_indices.first().copied();
    }
    let candidate = &candidates[0];
    let content = candidate.get("content");
    let parts = content
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array);

    if let Some(parts) = parts {
        for part in parts {
            if let Some(kind) = unsupported_gemini_output_part_kind(part) {
                return reject_openai_stream(
                    state,
                    "invalid_request_error",
                    "unsupported_gemini_stream_event",
                    format!(
                        "Gemini streaming output part `{kind}` cannot be translated losslessly."
                    ),
                );
            }
            let has_thought_sig =
                part.get("thoughtSignature").is_some() || part.get("thought_signature").is_some();
            let is_thought = part.get("thought").and_then(Value::as_bool) == Some(true);
            if has_thought_sig || is_thought {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    if !t.is_empty() {
                        emit_openai_assistant_role_if_needed(state, &mut out);
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
                    state.openai_tool_calls.insert(
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
                    emit_openai_assistant_role_if_needed(state, &mut out);
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
                    emit_openai_assistant_role_if_needed(state, &mut out);
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
                state.openai_tool_calls.insert(
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
                emit_openai_assistant_role_if_needed(state, &mut out);
                out.push(openai_chunk(
                    state,
                    serde_json::json!({ "tool_calls": [tc] }),
                    None,
                ));
            }
        }
    }

    if let Some(finish) = candidate.get("finishReason").and_then(Value::as_str) {
        let fr = gemini_finish_reason_to_openai_stream(finish, !state.openai_tool_calls.is_empty());
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
        emit_openai_assistant_role_if_needed(state, &mut out);
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

    if ty == "response.refusal.delta" || ty == "response.refusal.done" {
        let raw_refusal = if ty == "response.refusal.delta" {
            event.get("delta").and_then(Value::as_str)
        } else {
            event.get("refusal").and_then(Value::as_str)
        }
        .unwrap_or("");
        if let Some(delta) = normalize_openai_stream_text(raw_refusal, &mut state.openai_seen_refusal)
        {
            out.push(openai_chunk(
                state,
                serde_json::json!({ "refusal": delta }),
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
            let output_index = event
                .get("output_index")
                .and_then(Value::as_u64)
                .map(|idx| idx as usize);
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .map(String::from);
            let idx = state.openai_tool_call_index;
            state.openai_tool_call_index += 1;
            let id = call_id.unwrap_or_else(|| format!("call_{idx}"));
            let tool_type = if item_ty == Some("custom_tool_call") {
                "custom"
            } else {
                "function"
            };
            let arguments = if tool_type == "custom" {
                item.get("input").and_then(Value::as_str).unwrap_or("")
            } else {
                item.get("arguments").and_then(Value::as_str).unwrap_or("")
            }
            .to_string();
            let mut tc = serde_json::json!({
                "index": idx,
                "id": id.clone(),
                "type": tool_type,
                "function": { "name": name.clone(), "arguments": arguments.clone() }
            });
            if let Some(proxied_tool_kind) = item.get("proxied_tool_kind").cloned() {
                tc["proxied_tool_kind"] = proxied_tool_kind;
            }
            state.openai_tool_calls.insert(
                idx,
                ToolCallState {
                    index: idx,
                    id: Some(serde_json::json!(id)),
                    name,
                    arguments,
                    tool_type: Some(tool_type.to_string()),
                    proxied_tool_kind: item
                        .get("proxied_tool_kind")
                        .and_then(Value::as_str)
                        .map(String::from),
                    block_index: output_index,
                    responses_item_id: item.get("id").and_then(Value::as_str).map(String::from),
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
            let idx = responses_event_tool_call_index(event, state)
                .unwrap_or_else(|| state.openai_tool_call_index.saturating_sub(1));
            let tool_type = if ty == "response.custom_tool_call_input.delta" {
                "custom"
            } else {
                "function"
            };
            let proxied_tool_kind = {
                let tc = state
                    .openai_tool_calls
                    .entry(idx)
                    .or_insert_with(|| ToolCallState {
                        index: idx,
                        ..Default::default()
                    });
                tc.tool_type
                    .get_or_insert_with(|| tool_type.to_string());
                if tc.responses_item_id.is_none() {
                    tc.responses_item_id = event
                        .get("item_id")
                        .and_then(Value::as_str)
                        .map(String::from);
                }
                if tc.block_index.is_none() {
                    tc.block_index = event
                        .get("output_index")
                        .and_then(Value::as_u64)
                        .map(|output_index| output_index as usize);
                }
                tc.arguments.push_str(delta);
                tc.proxied_tool_kind.clone()
            };
            let mut tool_call_delta = serde_json::json!({
                "index": idx,
                "type": tool_type,
                "function": { "arguments": delta }
            });
            if let Some(proxied_tool_kind) = proxied_tool_kind {
                tool_call_delta["proxied_tool_kind"] = Value::String(proxied_tool_kind);
            }
            out.push(openai_chunk(
                state,
                serde_json::json!({ "tool_calls": [tool_call_delta] }),
                None,
            ));
        }
        return out;
    }

    if ty == "response.completed" || ty == "response.incomplete" || ty == "response.failed" {
        if let Some(resp) = event.get("response") {
            if let Some(u) = resp.get("usage") {
                state.usage = Some(responses_usage_to_openai_usage_stream(u));
            }
        }
        if !state.finish_reason_sent {
            let has_tool_calls = !state.openai_tool_calls.is_empty()
                || event
                    .get("response")
                    .and_then(|resp| resp.get("output"))
                    .and_then(Value::as_array)
                    .map(|output| {
                        output.iter().any(|item| {
                            matches!(
                                item.get("type").and_then(Value::as_str),
                                Some("function_call") | Some("custom_tool_call")
                            )
                        })
                    })
                    .unwrap_or(false);
            let finish_reason = match ty {
                "response.incomplete" => event
                    .get("response")
                    .and_then(|resp| resp.get("incomplete_details"))
                    .and_then(|details| details.get("reason"))
                    .and_then(Value::as_str)
                    .map(|reason| match reason {
                        "max_output_tokens" => "length",
                        "content_filter" => "content_filter",
                        "pause_turn" => "pause_turn",
                        _ => "stop",
                    })
                    .unwrap_or("stop"),
                "response.failed" => event
                    .get("response")
                    .and_then(|resp| resp.get("error"))
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_str)
                    .map(|code| responses_failed_code_to_openai_finish_stream(Some(code)))
                    .unwrap_or_else(|| responses_failed_code_to_openai_finish_stream(None)),
                _ => {
                    if has_tool_calls {
                        "tool_calls"
                    } else {
                        "stop"
                    }
                }
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
    if upstream_format != client_format && state.fatal_rejection.is_some() {
        return Vec::new();
    }
    if upstream_format == UpstreamFormat::Anthropic && client_format != UpstreamFormat::Anthropic {
        if let Some(message) = anthropic_stream_incompatibility_for_non_anthropic_sink(event) {
            let openai_chunks = reject_openai_stream(
                state,
                "invalid_request_error",
                "unsupported_anthropic_stream_event",
                message,
            );
            if client_format == UpstreamFormat::OpenAiCompletion {
                return openai_chunks
                    .into_iter()
                    .map(|chunk| format_sse_data(&chunk))
                    .collect();
            }
            if client_format == UpstreamFormat::Google {
                let mut out = Vec::new();
                for chunk in &openai_chunks {
                    out.extend(openai_chunk_to_gemini_sse(chunk, state));
                }
                return out;
            }
            if client_format == UpstreamFormat::OpenAiResponses {
                let mut out = Vec::new();
                for chunk in &openai_chunks {
                    out.extend(openai_chunk_to_responses_sse(chunk, state));
                }
                return out;
            }
        }
    }
    if upstream_format == UpstreamFormat::OpenAiCompletion
        && client_format == UpstreamFormat::OpenAiResponses
        && event.get("_done").and_then(Value::as_bool) == Some(true)
    {
        if !state.responses_terminal_sent {
            let response_id = state
                .message_id
                .clone()
                .unwrap_or_else(|| "resp_0".to_string());
            return emit_openai_responses_terminal(state, &response_id, 0, 0);
        }
        return Vec::new();
    }
    if upstream_format == client_format {
        if event.get("_done").and_then(Value::as_bool) == Some(true) {
            return vec![b"data: [DONE]\n\n".to_vec()];
        }
        return vec![format_sse_data(event)];
    }
    if upstream_format == UpstreamFormat::Anthropic
        && event.get("type").and_then(Value::as_str) == Some("error")
    {
        return anthropic_error_event_to_client_sse(event, client_format, state);
    }
    let openai_chunks: Vec<Value> = match upstream_format {
        UpstreamFormat::OpenAiCompletion => openai_event_as_chunk(event).into_iter().collect(),
        UpstreamFormat::Anthropic => claude_event_to_openai_chunks(event, state),
        UpstreamFormat::Google => gemini_event_to_openai_chunks(event, state),
        UpstreamFormat::OpenAiResponses => responses_event_to_openai_chunks(event, state),
    };
    let openai_chunks = if upstream_format == UpstreamFormat::OpenAiCompletion
        && client_format != UpstreamFormat::OpenAiCompletion
    {
        let mut validated = Vec::with_capacity(openai_chunks.len());
        let mut rejection = None;
        for chunk in openai_chunks {
            match ensure_single_openai_choice_for_non_openai_sink(&chunk, state) {
                Ok(()) => validated.push(chunk),
                Err(rejected) => {
                    rejection = Some(rejected);
                    break;
                }
            }
        }
        rejection.unwrap_or(validated)
    } else {
        openai_chunks
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
        return out;
    }
    if client_format == UpstreamFormat::Google {
        let mut out = Vec::new();
        for c in &openai_chunks {
            out.extend(openai_chunk_to_gemini_sse(c, state));
        }
        return out;
    }
    if client_format == UpstreamFormat::OpenAiResponses {
        let mut out = Vec::new();
        for c in &openai_chunks {
            out.extend(openai_chunk_to_responses_sse(c, state));
        }
        return out;
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

fn minimax_reasoning_details_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}

fn is_minimax_chunk(chunk: &Value, state: &StreamState) -> bool {
    chunk
        .get("model")
        .and_then(Value::as_str)
        .or(state.model.as_deref())
        .map(|model| model.starts_with("MiniMax-"))
        .unwrap_or(false)
}

fn normalize_openai_stream_text(incoming: &str, seen: &mut String) -> Option<String> {
    if incoming.is_empty() {
        return None;
    }
    if incoming == seen {
        return None;
    }

    let delta = if incoming.starts_with(seen.as_str()) {
        incoming[seen.len()..].to_string()
    } else {
        incoming.to_string()
    };

    if incoming.starts_with(seen.as_str()) {
        *seen = incoming.to_string();
    } else {
        seen.push_str(incoming);
    }

    (!delta.is_empty()).then_some(delta)
}

fn openai_chunk_reasoning_delta(delta: &Value, state: &mut StreamState) -> Option<String> {
    if let Some(reasoning) = delta
        .get("reasoning_content")
        .or(delta.get("reasoning"))
        .and_then(Value::as_str)
    {
        return normalize_openai_stream_text(reasoning, &mut state.openai_seen_reasoning);
    }
    let reasoning = minimax_reasoning_details_text(delta.get("reasoning_details"))?;
    normalize_openai_stream_text(&reasoning, &mut state.openai_seen_reasoning)
}

fn openai_chunk_content_delta(delta: &Value, state: &mut StreamState) -> Option<String> {
    let content = delta.get("content").and_then(Value::as_str)?;
    normalize_openai_stream_text(content, &mut state.openai_seen_content)
}

fn openai_chunk_refusal_delta(delta: &Value, state: &mut StreamState) -> Option<String> {
    let refusal = delta.get("refusal").and_then(Value::as_str)?;
    normalize_openai_stream_text(refusal, &mut state.openai_seen_refusal)
}

fn openai_chunk_annotations_delta(delta: &Value) -> Vec<Value> {
    delta.get("annotations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn responses_message_part_delta_event_type(kind: ResponsesMessagePartKind) -> &'static str {
    match kind {
        ResponsesMessagePartKind::OutputText => "response.output_text.delta",
        ResponsesMessagePartKind::Refusal => "response.refusal.delta",
    }
}

fn responses_message_part_done_event_type(kind: ResponsesMessagePartKind) -> &'static str {
    match kind {
        ResponsesMessagePartKind::OutputText => "response.output_text.done",
        ResponsesMessagePartKind::Refusal => "response.refusal.done",
    }
}

fn responses_message_part_value(part: &ResponsesMessagePartState) -> Value {
    match part.kind.unwrap_or(ResponsesMessagePartKind::OutputText) {
        ResponsesMessagePartKind::OutputText => serde_json::json!({
            "type": "output_text",
            "text": part.text,
            "annotations": part.annotations
        }),
        ResponsesMessagePartKind::Refusal => serde_json::json!({
            "type": "refusal",
            "refusal": part.text
        }),
    }
}

fn next_responses_seq(state: &mut StreamState) -> u64 {
    state.responses_seq += 1;
    state.responses_seq
}

fn reserve_responses_output_index(state: &mut StreamState) -> u64 {
    let idx = state.responses_next_output_index;
    state.responses_next_output_index += 1;
    idx
}

fn responses_reasoning_output_index(state: &mut StreamState) -> u64 {
    if let Some(idx) = state.responses_reasoning_output_index {
        idx
    } else {
        let idx = reserve_responses_output_index(state);
        state.responses_reasoning_output_index = Some(idx);
        idx
    }
}

fn responses_message_output_index(state: &mut StreamState) -> u64 {
    if let Some(idx) = state.responses_message_output_index {
        idx
    } else {
        let idx = reserve_responses_output_index(state);
        state.responses_message_output_index = Some(idx);
        idx
    }
}

fn responses_tool_output_index(state: &mut StreamState, tc_idx: usize) -> u64 {
    if let Some(existing) = state
        .openai_tool_calls
        .get(&tc_idx)
        .and_then(|tool_call| tool_call.block_index)
    {
        existing as u64
    } else {
        let output_index = reserve_responses_output_index(state) as usize;
        let entry = state
            .openai_tool_calls
            .entry(tc_idx)
            .or_insert_with(|| ToolCallState {
                index: tc_idx,
                ..Default::default()
            });
        entry.block_index = Some(output_index);
        output_index as u64
    }
}

fn ensure_responses_message_item_added(
    state: &mut StreamState,
    response_id: &str,
    output_index: u64,
    out: &mut Vec<Vec<u8>>,
) {
    if state.output_item_added {
        return;
    }
    state.output_item_added = true;
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
        "sequence_number": next_responses_seq(state),
        "response_id": response_id,
        "output_index": output_index,
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

fn ensure_responses_message_part(
    state: &mut StreamState,
    response_id: &str,
    output_index: u64,
    kind: ResponsesMessagePartKind,
    out: &mut Vec<Vec<u8>>,
) -> usize {
    let existing = match kind {
        ResponsesMessagePartKind::OutputText => state.responses_text_part_index,
        ResponsesMessagePartKind::Refusal => state.responses_refusal_part_index,
    };
    if let Some(index) = existing {
        return index;
    }

    let index = state.responses_message_parts.len();
    let part = ResponsesMessagePartState {
        kind: Some(kind),
        ..Default::default()
    };
    let part_value = responses_message_part_value(&part);
    state.responses_message_parts.push(part);
    state.responses_content_part_added = true;
    match kind {
        ResponsesMessagePartKind::OutputText => state.responses_text_part_index = Some(index),
        ResponsesMessagePartKind::Refusal => state.responses_refusal_part_index = Some(index),
    }

    let item_id = state
        .output_item_id
        .clone()
        .unwrap_or_else(|| "msg_0".to_string());
    let content_part_ev = serde_json::json!({
        "type": "response.content_part.added",
        "sequence_number": next_responses_seq(state),
        "response_id": response_id,
        "output_index": output_index,
        "content_index": index,
        "item_id": item_id,
        "part": part_value
    });
    out.push(format_sse_event(
        "response.content_part.added",
        &content_part_ev,
    ));
    index
}

fn emit_openai_responses_terminal(
    state: &mut StreamState,
    response_id: &str,
    created: u64,
    _idx: u64,
) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let finish_reason = state.finish_reason.clone();
    let Some(finish_reason) = finish_reason.as_deref() else {
        return out;
    };

    let incomplete_reason = match finish_reason {
        "length" => Some("max_output_tokens"),
        "content_filter" => Some("content_filter"),
        "pause_turn" => Some("pause_turn"),
        _ => None,
    };
    let failed_error = match finish_reason {
        "context_length_exceeded" => Some(serde_json::json!({
            "type": "invalid_request_error",
            "code": "context_length_exceeded",
            "message": "Your input exceeds the context window of this model. Please adjust your input and try again."
        })),
        "error" => state.openai_terminal_error.clone().or_else(|| {
            Some(serde_json::json!({
                "type": "server_error",
                "code": "error",
                "message": "The provider returned an error."
            }))
        }),
        "tool_error" => Some(serde_json::json!({
            "type": "invalid_request_error",
            "code": "tool_error",
            "message": "The provider reported a tool or protocol error."
        })),
        _ => None,
    };

    if state.responses_reasoning_added && !state.responses_reasoning_done {
        state.responses_reasoning_done = true;
        let output_index = if let Some(idx) = state.responses_reasoning_output_index {
            idx
        } else {
            responses_reasoning_output_index(state)
        };
        let item_id = state
            .responses_reasoning_id
            .clone()
            .unwrap_or_else(|| "rs_0".to_string());
        let text = state.responses_reasoning_text.clone();
        let text_done_ev = serde_json::json!({
            "type": "response.reasoning_summary_text.done",
            "sequence_number": next_responses_seq(state),
            "response_id": response_id,
            "item_id": item_id,
            "output_index": output_index,
            "summary_index": 0,
            "text": text
        });
        out.push(format_sse_event(
            "response.reasoning_summary_text.done",
            &text_done_ev,
        ));

        let part_done_ev = serde_json::json!({
            "type": "response.reasoning_summary_part.done",
            "sequence_number": next_responses_seq(state),
            "response_id": response_id,
            "item_id": item_id,
            "output_index": output_index,
            "summary_index": 0,
            "part": { "type": "summary_text", "text": state.responses_reasoning_text }
        });
        out.push(format_sse_event(
            "response.reasoning_summary_part.done",
            &part_done_ev,
        ));

        let output_item_done_ev = serde_json::json!({
            "type": "response.output_item.done",
            "sequence_number": next_responses_seq(state),
            "response_id": response_id,
            "output_index": output_index,
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

    let mut tool_call_keys = state.openai_tool_calls.keys().cloned().collect::<Vec<_>>();
    tool_call_keys.sort_unstable();
    let mut completed_tool_calls = Vec::new();
    for key in tool_call_keys {
        let Some(tool_call) = state.openai_tool_calls.get_mut(&key) else {
            continue;
        };
        if tool_call.responses_item_added && !tool_call.responses_done {
            tool_call.responses_done = true;
            let Some(call_id) = tool_call.id.as_ref().and_then(Value::as_str) else {
                continue;
            };
            completed_tool_calls.push((
                call_id.to_string(),
                tool_call.name.clone(),
                tool_call.arguments.clone(),
                tool_call.block_index.unwrap_or(tool_call.index),
                tool_call_state_type(tool_call).to_string(),
                tool_call.proxied_tool_kind.clone(),
            ));
        }
    }
    for (call_id, name, arguments, output_index, tool_type, proxied_tool_kind) in
        completed_tool_calls
    {
        let payload_field = responses_tool_call_payload_field(&tool_type);
        let mut args_done_ev = serde_json::json!({
            "type": responses_tool_call_done_event_type(&tool_type),
            "sequence_number": next_responses_seq(state),
            "response_id": response_id,
            "call_id": call_id,
            "name": name,
            "item_id": format!("fc_{}", call_id),
            "output_index": output_index,
        });
        args_done_ev[payload_field] = Value::String(arguments.clone());
        out.push(format_sse_event(
            responses_tool_call_done_event_type(&tool_type),
            &args_done_ev,
        ));

        let mut output_item_done_ev = serde_json::json!({
            "type": "response.output_item.done",
            "sequence_number": next_responses_seq(state),
            "response_id": response_id,
            "output_index": output_index,
            "item": {
                "id": format!("fc_{}", call_id),
                "type": responses_tool_call_item_type(&tool_type),
                "call_id": call_id,
                "name": name,
            }
        });
        output_item_done_ev["item"][payload_field] = Value::String(arguments.clone());
        if let Some(proxied_tool_kind) = proxied_tool_kind {
            output_item_done_ev["item"]["proxied_tool_kind"] = Value::String(proxied_tool_kind);
        }
        out.push(format_sse_event(
            "response.output_item.done",
            &output_item_done_ev,
        ));
    }

    if state.output_item_added {
        let output_index = if let Some(idx) = state.responses_message_output_index {
            idx
        } else {
            responses_message_output_index(state)
        };
        let item_id = state
            .output_item_id
            .clone()
            .unwrap_or_else(|| "msg_0".to_string());
        let message_parts = state.responses_message_parts.clone();
        let mut content = Vec::with_capacity(message_parts.len());
        for (content_index, part) in message_parts.iter().enumerate() {
            let part_value = responses_message_part_value(part);
            match part.kind.unwrap_or(ResponsesMessagePartKind::OutputText) {
                ResponsesMessagePartKind::OutputText => {
                    let done_ev = serde_json::json!({
                        "type": responses_message_part_done_event_type(ResponsesMessagePartKind::OutputText),
                        "sequence_number": next_responses_seq(state),
                        "response_id": response_id,
                        "output_index": output_index,
                        "content_index": content_index,
                        "item_id": item_id,
                        "text": part.text
                    });
                    out.push(format_sse_event(
                        responses_message_part_done_event_type(ResponsesMessagePartKind::OutputText),
                        &done_ev,
                    ));
                }
                ResponsesMessagePartKind::Refusal => {
                    let done_ev = serde_json::json!({
                        "type": responses_message_part_done_event_type(ResponsesMessagePartKind::Refusal),
                        "sequence_number": next_responses_seq(state),
                        "response_id": response_id,
                        "output_index": output_index,
                        "content_index": content_index,
                        "item_id": item_id,
                        "refusal": part.text
                    });
                    out.push(format_sse_event(
                        responses_message_part_done_event_type(ResponsesMessagePartKind::Refusal),
                        &done_ev,
                    ));
                }
            }
            let part_done_ev = serde_json::json!({
                "type": "response.content_part.done",
                "sequence_number": next_responses_seq(state),
                "response_id": response_id,
                "output_index": output_index,
                "content_index": content_index,
                "item_id": item_id,
                "part": part_value
            });
            out.push(format_sse_event(
                "response.content_part.done",
                &part_done_ev,
            ));
            content.push(part_value);
        }
        let output_item_done_ev = serde_json::json!({
            "type": "response.output_item.done",
            "sequence_number": next_responses_seq(state),
            "response_id": response_id,
            "output_index": output_index,
            "item": {
                "id": item_id,
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": content
            }
        });
        out.push(format_sse_event(
            "response.output_item.done",
            &output_item_done_ev,
        ));
    }

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
            "content": state
                .responses_message_parts
                .iter()
                .map(responses_message_part_value)
                .collect::<Vec<_>>()
        }));
    }
    let mut tool_call_output = state.openai_tool_calls.values().collect::<Vec<_>>();
    tool_call_output.sort_by_key(|tc| tc.index);
    for tool_call in tool_call_output {
        if let Some(call_id) = tool_call.id.as_ref().and_then(Value::as_str) {
            let payload_field = responses_tool_call_payload_field(tool_call_state_type(tool_call));
            let mut item = serde_json::json!({
                "id": format!("fc_{}", call_id),
                "type": responses_tool_call_item_type(tool_call_state_type(tool_call)),
                "call_id": call_id,
                "name": tool_call.name,
            });
            item[payload_field] = Value::String(tool_call.arguments.clone());
            if let Some(proxied_tool_kind) = tool_call.proxied_tool_kind.clone() {
                item["proxied_tool_kind"] = Value::String(proxied_tool_kind);
            }
            output.push(item);
        }
    }

    let mut resp = serde_json::json!({
        "id": response_id,
        "object": "response",
        "created_at": created,
        "status": if failed_error.is_some() {
            "failed"
        } else if incomplete_reason.is_some() {
            "incomplete"
        } else {
            "completed"
        },
        "error": failed_error.clone().unwrap_or(serde_json::Value::Null),
        "incomplete_details": incomplete_reason.map(|reason| serde_json::json!({ "reason": reason })).unwrap_or(serde_json::Value::Null),
        "output": output
    });
    if let Some(ref u) = state.usage {
        resp["usage"] = openai_usage_to_responses_usage_stream(u);
    }
    let event_type = if failed_error.is_some() {
        "response.failed"
    } else if incomplete_reason.is_some() {
        "response.incomplete"
    } else {
        "response.completed"
    };
    let ev = serde_json::json!({
        "type": event_type,
        "sequence_number": next_responses_seq(state),
        "response": resp
    });
    out.push(format_sse_event(event_type, &ev));
    state.responses_terminal_sent = true;
    out
}

fn anthropic_error_event(error_type: &str, message: &str) -> Vec<u8> {
    format_sse_event(
        "error",
        &serde_json::json!({
            "type": "error",
            "error": {
                "type": error_type,
                "message": message
            }
        }),
    )
}

fn reject_anthropic_stream(
    state: &mut StreamState,
    error_type: &str,
    message: impl Into<String>,
) -> Vec<Vec<u8>> {
    let message = mark_stream_fatal_rejection(state, message);
    vec![anthropic_error_event(error_type, &message)]
}

fn openai_chunk_to_claude_sse(chunk: &Value, state: &mut StreamState) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if state.fatal_rejection.is_some() && chunk.get("error").is_none() {
        return out;
    }
    let choices = match chunk.get("choices").and_then(Value::as_array) {
        Some(c) if !c.is_empty() => c,
        _ => return out,
    };
    let choice = &choices[0];
    let delta = choice.get("delta").unwrap_or(&serde_json::Value::Null);
    let finish_reason = choice.get("finish_reason").and_then(Value::as_str);
    let reasoning_delta = openai_chunk_reasoning_delta(delta, state);

    if reasoning_delta.as_deref().is_some_and(|reasoning| !reasoning.is_empty()) {
        return reject_anthropic_stream(
            state,
            "invalid_request_error",
            OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE,
        );
    }

    if let Some(message) = delta
        .get("tool_calls")
        .and_then(Value::as_array)
        .and_then(|tool_calls| {
            tool_calls
                .iter()
                .find_map(|tool_call| anthropic_tool_use_type_for_openai_tool_call(tool_call).err())
        })
    {
        return reject_anthropic_stream(state, "invalid_request_error", message);
    }

    if let Some(fr) = finish_reason {
        if let AnthropicTerminal::Error {
            error_type,
            message,
        } = classify_openai_finish_for_anthropic(fr)
        {
            let delta_is_empty = delta
                .as_object()
                .map(|obj| obj.is_empty())
                .unwrap_or_else(|| delta.is_null());
            if !state.message_start_sent && delta_is_empty {
                out.push(anthropic_error_event(error_type, message));
                return out;
            }
        }
    }

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

    if let Some(content) = openai_chunk_content_delta(delta, state) {
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
                let tool_use_type = match anthropic_tool_use_type_for_openai_tool_call(tc) {
                    Ok(tool_use_type) => tool_use_type,
                    Err(message) => {
                        return reject_anthropic_stream(state, "invalid_request_error", message);
                    }
                };
                let ev = serde_json::json!({
                    "type": "content_block_start",
                    "index": block_index,
                    "content_block": { "type": tool_use_type, "id": id, "name": name, "input": {} }
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
        state.usage = Some(openai_usage_to_anthropic_usage_stream(usage));
    }

    if let Some(fr) = finish_reason {
        match classify_openai_finish_for_anthropic(fr) {
            AnthropicTerminal::StopReason(stop_reason) => {
                stop_thinking_block_claude(state, &mut out);
                stop_text_block_claude(state, &mut out);
                for &block_index in state.tool_block_indices.values() {
                    let ev =
                        serde_json::json!({ "type": "content_block_stop", "index": block_index });
                    out.push(format_sse_event("content_block_stop", &ev));
                }
                let usage = state.usage.clone().unwrap_or_else(
                    || serde_json::json!({ "input_tokens": 0, "output_tokens": 0 }),
                );
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
            AnthropicTerminal::Error {
                error_type,
                message,
            } => {
                out.push(anthropic_error_event(error_type, message));
            }
        }
    }
    out
}

fn openai_chunk_to_gemini_sse(chunk: &Value, state: &mut StreamState) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if state.fatal_rejection.is_some() && chunk.get("error").is_none() {
        return out;
    }
    if let Some(usage) = chunk.get("usage") {
        state.usage = Some(usage.clone());
    }
    if let Some(error) = chunk.get("error") {
        let mut payload = serde_json::json!({ "error": error });
        if let Some(response_id) = chunk.get("id").cloned() {
            payload["responseId"] = response_id;
        }
        if let Some(model) = chunk.get("model").cloned() {
            payload["modelVersion"] = model;
        }
        out.push(format_sse_data(&payload));
        return out;
    }
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

    let model = state.model.clone().unwrap_or_else(|| "gemini".to_string());
    let mut parts: Vec<Value> = vec![];

    if let Some(r) = openai_chunk_reasoning_delta(delta, state) {
        if !r.is_empty() {
            parts.push(serde_json::json!({ "text": r, "thought": true }));
        }
    }
    if let Some(c) = openai_chunk_content_delta(delta, state) {
        if !c.is_empty() {
            parts.push(serde_json::json!({ "text": c }));
        }
    }
    if let Some(refusal) = openai_chunk_refusal_delta(delta, state) {
        if !refusal.is_empty() {
            parts.push(serde_json::json!({ "text": refusal }));
        }
    }
    if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tcs {
            let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let entry = state
                .openai_tool_calls
                .entry(idx)
                .or_insert_with(|| ToolCallState {
                    index: idx,
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
            if let Some(arguments) = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
            {
                if !arguments.is_empty() {
                    entry.arguments.push_str(arguments);
                }
            }
        }
    }

    loop {
        let next_idx = state.gemini_next_tool_call_to_emit;
        let Some(entry) = state.openai_tool_calls.get_mut(&next_idx) else {
            break;
        };
        let next_part = {
            if entry.arguments.is_empty()
                || entry.gemini_emitted_arguments.as_deref() == Some(entry.arguments.as_str())
            {
                None
            } else {
                let Ok(args_val) = serde_json::from_str::<Value>(&entry.arguments) else {
                    break;
                };
                entry.gemini_emitted_arguments = Some(entry.arguments.clone());
                Some((
                    entry.name.clone(),
                    args_val,
                    entry
                        .id
                        .as_ref()
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ))
            }
        };
        let Some((name, args_val, id)) = next_part else {
            break;
        };
        let mut part = serde_json::json!({
            "functionCall": { "name": name, "args": args_val, "id": id }
        });
        if !state.gemini_dummy_signature_emitted {
            part["thoughtSignature"] =
                Value::String(GEMINI_DUMMY_THOUGHT_SIGNATURE_STREAM.to_string());
            state.gemini_dummy_signature_emitted = true;
        }
        parts.push(part);
        state.gemini_next_tool_call_to_emit += 1;
    }

    if !parts.is_empty() || finish_reason.is_some() {
        let fr = finish_reason
            .map(openai_finish_reason_to_gemini_stream)
            .unwrap_or("");
        let mut candidate = serde_json::json!({
            "content": { "parts": parts },
            "finishReason": fr
        });
        if fr.is_empty() {
            candidate
                .as_object_mut()
                .expect("candidate object")
                .remove("finishReason");
        }
        let mut payload = serde_json::json!({
            "candidates": [candidate],
            "modelVersion": model
        });
        if let Some(response_id) = chunk.get("id").cloned() {
            payload["responseId"] = response_id;
        }
        if let Some(usage) = chunk.get("usage").or(state.usage.as_ref()) {
            let prompt_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let completion_tokens = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let total_tokens = usage
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(prompt_tokens + completion_tokens);
            let reasoning_tokens = usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let cached_tokens = usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let mut usage_metadata = serde_json::json!({
                "promptTokenCount": prompt_tokens,
                "candidatesTokenCount": completion_tokens.saturating_sub(reasoning_tokens),
                "totalTokenCount": total_tokens
            });
            if reasoning_tokens > 0 {
                usage_metadata["thoughtsTokenCount"] = reasoning_tokens.into();
            }
            if cached_tokens > 0 {
                usage_metadata["cachedContentTokenCount"] = cached_tokens.into();
            }
            payload["usageMetadata"] = usage_metadata;
        }
        out.push(format_sse_data(&payload));
    }
    out
}

fn openai_chunk_to_responses_sse(chunk: &Value, state: &mut StreamState) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if let Some(error) = chunk.get("error") {
        state.openai_terminal_error = Some(error.clone());
    }
    let choices = chunk.get("choices").and_then(Value::as_array);
    let choice = choices.and_then(|c| c.first());
    let delta = choice
        .and_then(|choice| choice.get("delta"))
        .unwrap_or(&serde_json::Value::Null);
    let finish_reason = choice
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str);
    let idx = choice
        .and_then(|choice| choice.get("index"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let response_id = chunk
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| state.message_id.clone())
        .unwrap_or_else(|| "resp_0".to_string());

    if !state.responses_started {
        state.responses_started = true;
        state.openai_terminal_error = chunk.get("error").cloned();
        state.message_id = chunk
            .get("id")
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        let created = chunk.get("created").and_then(Value::as_u64).unwrap_or(0);
        let ev = serde_json::json!({
            "type": "response.created",
            "sequence_number": next_responses_seq(state),
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
            "sequence_number": next_responses_seq(state),
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

    if let Some(r) = openai_chunk_reasoning_delta(delta, state) {
        if !r.is_empty() {
            state.responses_reasoning_text.push_str(&r);
            let output_index = responses_reasoning_output_index(state);
            if !state.responses_reasoning_added {
                state.responses_reasoning_added = true;
                let item_id = state
                    .responses_reasoning_id
                    .clone()
                    .unwrap_or_else(|| format!("rs_{}", uuid::Uuid::new_v4().simple()));
                state.responses_reasoning_id = Some(item_id.clone());

                let added_ev = serde_json::json!({
                    "type": "response.output_item.added",
                    "sequence_number": next_responses_seq(state),
                    "response_id": response_id,
                    "output_index": output_index,
                    "item": {
                        "id": item_id,
                        "type": "reasoning",
                        "summary": []
                    }
                });
                out.push(format_sse_event("response.output_item.added", &added_ev));

                let part_added_ev = serde_json::json!({
                    "type": "response.reasoning_summary_part.added",
                    "sequence_number": next_responses_seq(state),
                    "response_id": response_id,
                    "item_id": state.responses_reasoning_id,
                    "output_index": output_index,
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
                "sequence_number": next_responses_seq(state),
                "response_id": response_id,
                "item_id": state.responses_reasoning_id,
                "output_index": output_index,
                "summary_index": 0,
                "delta": r
            });
            out.push(format_sse_event(
                "response.reasoning_summary_text.delta",
                &ev,
            ));
        }
    }
    let content_delta = openai_chunk_content_delta(delta, state);
    let refusal_delta = openai_chunk_refusal_delta(delta, state);
    let annotations_delta = openai_chunk_annotations_delta(delta);

    let had_content_delta = content_delta.is_some();
    if let Some(c) = content_delta {
        if !c.is_empty() {
            let output_index = responses_message_output_index(state);
            ensure_responses_message_item_added(state, &response_id, output_index, &mut out);
            let content_index = ensure_responses_message_part(
                state,
                &response_id,
                output_index,
                ResponsesMessagePartKind::OutputText,
                &mut out,
            );
            state.responses_output_text.push_str(&c);
            if let Some(part) = state.responses_message_parts.get_mut(content_index) {
                part.text.push_str(&c);
                if !annotations_delta.is_empty() {
                    part.annotations.extend(annotations_delta.clone());
                }
            }
            let item_id = state
                .output_item_id
                .clone()
                .unwrap_or_else(|| "msg_0".to_string());
            let event_type =
                responses_message_part_delta_event_type(ResponsesMessagePartKind::OutputText);
            let ev = serde_json::json!({
                "type": event_type,
                "sequence_number": next_responses_seq(state),
                "response_id": response_id,
                "output_index": output_index,
                "content_index": content_index,
                "item_id": item_id,
                "delta": c
            });
            out.push(format_sse_event(event_type, &ev));
        }
    }
    if !annotations_delta.is_empty() && !had_content_delta {
        let output_index = responses_message_output_index(state);
        ensure_responses_message_item_added(state, &response_id, output_index, &mut out);
        let content_index = ensure_responses_message_part(
            state,
            &response_id,
            output_index,
            ResponsesMessagePartKind::OutputText,
            &mut out,
        );
        if let Some(part) = state.responses_message_parts.get_mut(content_index) {
            part.annotations.extend(annotations_delta.clone());
        }
    }
    if let Some(refusal) = refusal_delta {
        if !refusal.is_empty() {
            let output_index = responses_message_output_index(state);
            ensure_responses_message_item_added(state, &response_id, output_index, &mut out);
            let content_index = ensure_responses_message_part(
                state,
                &response_id,
                output_index,
                ResponsesMessagePartKind::Refusal,
                &mut out,
            );
            if let Some(part) = state.responses_message_parts.get_mut(content_index) {
                part.text.push_str(&refusal);
            }
            let item_id = state
                .output_item_id
                .clone()
                .unwrap_or_else(|| "msg_0".to_string());
            let event_type = responses_message_part_delta_event_type(ResponsesMessagePartKind::Refusal);
            let ev = serde_json::json!({
                "type": event_type,
                "sequence_number": next_responses_seq(state),
                "response_id": response_id,
                "output_index": output_index,
                "content_index": content_index,
                "item_id": item_id,
                "delta": refusal
            });
            out.push(format_sse_event(event_type, &ev));
        }
    }
    if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tcs {
            let tc_idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            dedupe_tool_call_state_by_call_id(&mut state.openai_tool_calls, tc_idx, tc.get("id"));
            let output_index = responses_tool_output_index(state, tc_idx);
            let mut item_added: Option<(String, String, String, Option<String>)> = None;
            let mut args_delta: Option<(String, String, String, String, Option<String>)> = None;
            {
                let entry =
                    state
                        .openai_tool_calls
                        .entry(tc_idx)
                        .or_insert_with(|| ToolCallState {
                            index: tc_idx,
                            ..Default::default()
                        });
                if let Some(id) = tc.get("id").cloned() {
                    entry.id = Some(id);
                }
                let tool_type = openai_stream_tool_call_type(tc).to_string();
                if entry.tool_type.is_none() || tc.get("type").is_some() {
                    entry.tool_type = Some(tool_type.clone());
                }
                if let Some(proxied_tool_kind) = tc
                    .get("proxied_tool_kind")
                    .and_then(Value::as_str)
                    .map(String::from)
                {
                    entry.proxied_tool_kind = Some(proxied_tool_kind);
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
                    let call_id = entry
                        .id
                        .as_ref()
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    entry.responses_item_id = Some(format!("fc_{call_id}"));
                    item_added = Some((
                        call_id,
                        entry.name.clone(),
                        tool_call_state_type(entry).to_string(),
                        entry.proxied_tool_kind.clone(),
                    ));
                }
                if let Some(args) = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                {
                    if !args.is_empty() {
                        entry.arguments.push_str(args);
                        let call_id = entry
                            .id
                            .as_ref()
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        args_delta = Some((
                            call_id,
                            entry.name.clone(),
                            args.to_string(),
                            tool_call_state_type(entry).to_string(),
                            entry.proxied_tool_kind.clone(),
                        ));
                    }
                }
            }
            if let Some((call_id, name, tool_type, proxied_tool_kind)) = item_added {
                let payload_field = responses_tool_call_payload_field(&tool_type);
                let mut ev = serde_json::json!({
                    "type": "response.output_item.added",
                    "sequence_number": next_responses_seq(state),
                    "response_id": response_id,
                    "output_index": output_index,
                    "item": {
                        "id": format!("fc_{}", call_id),
                        "type": responses_tool_call_item_type(&tool_type),
                        "call_id": call_id,
                        "name": name,
                    }
                });
                ev["item"][payload_field] = Value::String(String::new());
                if let Some(proxied_tool_kind) = proxied_tool_kind {
                    ev["item"]["proxied_tool_kind"] = Value::String(proxied_tool_kind);
                }
                out.push(format_sse_event("response.output_item.added", &ev));
            }
            if let Some((call_id, name, args, tool_type, proxied_tool_kind)) = args_delta {
                let mut ev = serde_json::json!({
                    "type": responses_tool_call_delta_event_type(&tool_type),
                    "sequence_number": next_responses_seq(state),
                    "response_id": response_id,
                    "call_id": call_id,
                    "name": name,
                    "item_id": format!("fc_{}", call_id),
                    "output_index": output_index,
                    "delta": args
                });
                if let Some(proxied_tool_kind) = proxied_tool_kind {
                    ev["proxied_tool_kind"] = Value::String(proxied_tool_kind);
                }
                out.push(format_sse_event(
                    responses_tool_call_delta_event_type(&tool_type),
                    &ev,
                ));
            }
        }
    }

    if let Some(u) = chunk.get("usage") {
        state.usage = Some(u.clone());
    }
    if matches!(
        finish_reason,
        Some("context_length_exceeded") | Some("error") | Some("tool_error")
    ) {
        state.finish_reason = finish_reason.map(str::to_string);
        out.extend(emit_openai_responses_terminal(
            state,
            &response_id,
            chunk.get("created").and_then(Value::as_u64).unwrap_or(0),
            idx,
        ));
        return out;
    }
    if let Some(fr) = finish_reason {
        state.finish_reason = Some(fr.to_string());
    }
    let has_real_usage = chunk
        .get("usage")
        .map(|usage| !usage.is_null())
        .unwrap_or(false);
    let should_finalize_now = if state.finish_reason.is_some() && !state.responses_terminal_sent {
        has_real_usage || !is_minimax_chunk(chunk, state)
    } else {
        false
    };
    if should_finalize_now {
        let created = chunk.get("created").and_then(Value::as_u64).unwrap_or(0);
        out.extend(emit_openai_responses_terminal(
            state,
            &response_id,
            created,
            idx,
        ));
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

fn anthropic_error_event_to_client_sse(
    event: &Value,
    client_format: UpstreamFormat,
    state: &mut StreamState,
) -> Vec<Vec<u8>> {
    let error = event.get("error").unwrap_or(&Value::Null);
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("api_error");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Anthropic streaming error");

    let (normalized_type, normalized_code, finish_reason) =
        normalize_anthropic_stream_error(error_type, message);

    match client_format {
        UpstreamFormat::OpenAiResponses => {
            state.responses_seq += 1;
            let response_id = state
                .message_id
                .clone()
                .unwrap_or_else(|| format!("resp_error_{}", uuid::Uuid::new_v4().simple()));
            let failed = serde_json::json!({
                "type": "response.failed",
                "sequence_number": state.responses_seq,
                "response": {
                    "id": response_id,
                    "object": "response",
                    "created_at": 0,
                    "status": "failed",
                    "background": false,
                    "error": {
                        "type": normalized_type,
                        "code": normalized_code,
                        "message": message
                    },
                    "incomplete_details": null,
                    "usage": null,
                    "metadata": {}
                }
            });
            vec![format_sse_event("response.failed", &failed)]
        }
        UpstreamFormat::OpenAiCompletion => {
            let mut chunk = openai_chunk(state, serde_json::json!({}), Some(finish_reason));
            chunk["error"] = serde_json::json!({
                "type": normalized_type,
                "code": normalized_code,
                "message": message
            });
            vec![format_sse_data(&chunk), b"data: [DONE]\n\n".to_vec()]
        }
        UpstreamFormat::Anthropic => vec![format_sse_data(event)],
        UpstreamFormat::Google => vec![],
    }
}

fn normalize_anthropic_stream_error(
    error_type: &str,
    message: &str,
) -> (&'static str, Option<&'static str>, &'static str) {
    let lower_type = error_type.to_ascii_lowercase();
    let lower_message = message.to_ascii_lowercase();
    if lower_type.contains("overloaded") || lower_type.contains("api_error") {
        let code = Some("server_is_overloaded");
        return (
            "server_error",
            code,
            classify_portable_non_success_terminal(code),
        );
    }
    if lower_type.contains("rate_limit") {
        let code = Some("rate_limit_exceeded");
        return (
            "rate_limit_error",
            code,
            classify_portable_non_success_terminal(code),
        );
    }
    if lower_type.contains("invalid_request")
        && (lower_message.contains("context window")
            || lower_message.contains("context_length_exceeded")
            || lower_message.contains("too many tokens")
            || lower_message.contains("maximum context length"))
    {
        return (
            "invalid_request_error",
            Some("context_length_exceeded"),
            "context_length_exceeded",
        );
    }
    if lower_type.contains("invalid_request")
        && (lower_message.contains("refusal") || lower_message.contains("content filter"))
    {
        return (
            "invalid_request_error",
            Some("content_filter"),
            "content_filter",
        );
    }
    let code = Some("server_is_overloaded");
    (
        "server_error",
        code,
        classify_portable_non_success_terminal(code),
    )
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

    fn parse_sse_json(bytes: &[u8]) -> Value {
        let mut buf = bytes.to_vec();
        take_one_sse_event(&mut buf).expect("parse sse event")
    }

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
    fn take_one_sse_event_handles_crlf_separators() {
        // Some upstream servers (e.g., vLLM/uvicorn) use \r\n\r\n as SSE separator
        let mut buf =
            b"data: {\"id\":\"chat123\",\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\r\n\r\n"
                .to_vec();
        let event = take_one_sse_event(&mut buf);
        assert!(event.is_some());
        assert_eq!(
            event.as_ref().unwrap().get("id").and_then(Value::as_str),
            Some("chat123")
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn take_one_sse_event_handles_crlf_done_marker() {
        let mut buf = b"data: [DONE]\r\n\r\n".to_vec();
        let event = take_one_sse_event(&mut buf);
        assert!(event.is_some());
        assert_eq!(
            event.as_ref().unwrap().get("_done"),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn take_one_sse_event_handles_mixed_crlf_and_lf() {
        // Buffer with one CRLF event followed by one LF event
        let mut buf = b"data: {\"first\":true}\r\n\r\ndata: {\"second\":true}\n\n".to_vec();
        let e1 = take_one_sse_event(&mut buf);
        assert!(e1.is_some());
        assert_eq!(
            e1.as_ref().unwrap().get("first"),
            Some(&serde_json::json!(true))
        );
        let e2 = take_one_sse_event(&mut buf);
        assert!(e2.is_some());
        assert_eq!(
            e2.as_ref().unwrap().get("second"),
            Some(&serde_json::json!(true))
        );
        assert!(buf.is_empty());
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
    fn claude_thinking_boundaries_do_not_emit_synthetic_reasoning_markers() {
        let mut state = StreamState::default();
        let _ = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-3" }
            }),
            &mut state,
        );
        let start_chunks = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "thinking", "thinking": "" }
            }),
            &mut state,
        );
        let stop_chunks = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_stop",
                "index": 0
            }),
            &mut state,
        );

        assert!(start_chunks.is_empty());
        assert!(stop_chunks.is_empty());
    }

    #[test]
    fn claude_signature_delta_updates_block_state_without_reasoning_chunk() {
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
                "delta": { "type": "signature_delta", "signature": "sig_123" }
            }),
            &mut state,
        );

        assert!(chunks.is_empty(), "chunks = {chunks:?}");
        assert_eq!(
            state
                .claude_blocks
                .get(&0)
                .and_then(|block| block.signature.as_deref()),
            Some("sig_123")
        );
    }

    #[test]
    fn claude_unknown_typed_delta_still_fails_closed() {
        let mut state = StreamState::default();
        let _ = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-3" }
            }),
            &mut state,
        );

        let chunks = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "unknown_future_delta", "payload": true }
            }),
            &mut state,
        );

        assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
        assert!(chunks[0]["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("unknown_future_delta"));
    }

    #[test]
    fn translate_sse_event_anthropic_thinking_to_openai_fails_closed_at_start_and_suppresses_followups(
    ) {
        let mut state = StreamState::default();
        let first = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-3" }
            }),
            &mut state,
        );
        let second = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "thinking",
                    "thinking": "ponder"
                }
            }),
            &mut state,
        );
        let third = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "hidden" }
            }),
            &mut state,
        );
        let fourth = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "signature_delta", "signature": "sig_123" }
            }),
            &mut state,
        );
        let joined = second
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!first.is_empty(), "message_start should still initialize stream");
        assert!(joined.contains("\"finish_reason\":\"error\""), "{joined}");
        assert!(
            joined.contains("thinking blocks cannot be translated losslessly"),
            "{joined}"
        );
        assert!(third.is_empty(), "follow-up after fatal reject should be suppressed");
        assert!(fourth.is_empty(), "follow-up after fatal reject should be suppressed");
        assert!(
            !joined.contains("reasoning_content"),
            "start-time reject must not leak reasoning_content: {joined}"
        );
    }

    #[test]
    fn translate_sse_event_anthropic_thinking_to_responses_fails_closed_at_start_and_suppresses_followups(
    ) {
        let mut state = StreamState::default();
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-3" }
            }),
            &mut state,
        );
        let rejected = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "thinking",
                    "thinking": "ponder"
                }
            }),
            &mut state,
        );
        let suppressed_reasoning = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "hidden" }
            }),
            &mut state,
        );
        let suppressed_signature = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "signature_delta", "signature": "sig_123" }
            }),
            &mut state,
        );
        let suppressed_stop = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "content_block_stop",
                "index": 0
            }),
            &mut state,
        );
        let joined = rejected
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            joined.contains("\"type\":\"response.failed\""),
            "joined = {joined}"
        );
        assert!(
            joined.contains("\"code\":\"unsupported_anthropic_stream_event\""),
            "joined = {joined}"
        );
        assert!(
            joined.contains("thinking blocks cannot be translated losslessly"),
            "joined = {joined}"
        );
        assert!(
            !joined.contains("response.reasoning_"),
            "start-time reject must not emit reasoning lifecycle events: {joined}"
        );
        assert!(suppressed_reasoning.is_empty(), "follow-up after fatal reject should be suppressed");
        assert!(suppressed_signature.is_empty(), "follow-up after fatal reject should be suppressed");
        assert!(suppressed_stop.is_empty(), "follow-up after fatal reject should be suppressed");
    }

    #[test]
    fn translate_sse_event_anthropic_thinking_to_gemini_fails_closed_at_start_and_suppresses_followups(
    ) {
        let mut state = StreamState::default();
        let first = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-3" }
            }),
            &mut state,
        );
        let rejected = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
            &serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "thinking",
                    "thinking": "ponder"
                }
            }),
            &mut state,
        );
        let suppressed = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "hidden" }
            }),
            &mut state,
        );
        let joined = rejected
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(first.is_empty(), "message_start alone should not emit Gemini content");
        assert!(joined.contains("\"error\""), "{joined}");
        assert!(
            joined.contains("\"code\":\"unsupported_anthropic_stream_event\""),
            "{joined}"
        );
        assert!(
            joined.contains("thinking blocks cannot be translated losslessly"),
            "{joined}"
        );
        assert!(
            !joined.contains("\"thought\":true") && !joined.contains("\"text\":\"hidden\""),
            "start-time reject must not emit Gemini thought/text output: {joined}"
        );
        assert!(suppressed.is_empty(), "follow-up after fatal reject should be suppressed");
    }

    #[test]
    fn translate_sse_event_anthropic_omitted_thinking_still_fails_closed_at_start() {
        let mut state = StreamState::default();
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-3" }
            }),
            &mut state,
        );
        let rejected = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "thinking",
                    "thinking": { "display": "omitted" }
                }
            }),
            &mut state,
        );

        let joined = rejected
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"finish_reason\":\"error\""), "{joined}");
        assert!(
            joined.contains("thinking blocks cannot be translated losslessly")
                || joined.contains("omitted thinking"),
            "{joined}"
        );
    }

    #[test]
    fn claude_server_tool_result_block_rejects_instead_of_succeeding() {
        let mut state = StreamState::default();
        let _ = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-3" }
            }),
            &mut state,
        );

        let chunks = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "server_tool_result",
                    "tool_use_id": "server_1",
                    "content": [{ "type": "text", "text": "result" }]
                }
            }),
            &mut state,
        );

        assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
        assert!(chunks[0]["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("server_tool_result"));
    }

    #[test]
    fn claude_message_stop_preserves_extra_usage_fields() {
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
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn" },
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "service_tier": "priority",
                    "server_tool_use": { "web_search_requests": 2 }
                }
            }),
            &mut state,
        );

        let chunks = claude_event_to_openai_chunks(
            &serde_json::json!({ "type": "message_stop" }),
            &mut state,
        );

        assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
        assert_eq!(chunks[0]["usage"]["prompt_tokens"], 10);
        assert_eq!(chunks[0]["usage"]["completion_tokens"], 5);
        assert_eq!(chunks[0]["usage"]["service_tier"], "priority");
        assert_eq!(
            chunks[0]["usage"]["server_tool_use"]["web_search_requests"],
            2
        );
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
    fn responses_refusal_events_produce_openai_refusal_deltas() {
        let mut state = StreamState {
            message_id: Some("resp_1".to_string()),
            ..Default::default()
        };

        let delta_chunks = responses_event_to_openai_chunks(
            &serde_json::json!({
                "type": "response.refusal.delta",
                "item_id": "msg_1",
                "output_index": 0,
                "content_index": 1,
                "delta": "No"
            }),
            &mut state,
        );
        let done_chunks = responses_event_to_openai_chunks(
            &serde_json::json!({
                "type": "response.refusal.done",
                "item_id": "msg_1",
                "output_index": 0,
                "content_index": 1,
                "refusal": "Nope"
            }),
            &mut state,
        );

        assert_eq!(delta_chunks.len(), 1);
        assert_eq!(delta_chunks[0]["choices"][0]["delta"]["refusal"], "No");
        assert_eq!(done_chunks.len(), 1);
        assert_eq!(done_chunks[0]["choices"][0]["delta"]["refusal"], "pe");
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
    fn responses_incomplete_pause_turn_event_produces_openai_pause_turn_finish() {
        let event = serde_json::json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_1",
                "incomplete_details": { "reason": "pause_turn" }
            }
        });
        let mut state = StreamState::default();
        let chunks = responses_event_to_openai_chunks(&event, &mut state);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "pause_turn");
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
    fn responses_failed_unknown_event_produces_openai_error_finish() {
        let event = serde_json::json!({
            "type": "response.failed",
            "response": {
                "id": "resp_1",
                "error": { "code": "server_error" },
                "output": [{
                    "id": "fc_1",
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup_weather",
                    "arguments": "{\"city\":\"Tokyo\"}"
                }]
            }
        });
        let mut state = StreamState::default();
        let chunks = responses_event_to_openai_chunks(&event, &mut state);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
    }

    #[test]
    fn responses_failed_tool_validation_event_produces_openai_tool_error_finish() {
        let event = serde_json::json!({
            "type": "response.failed",
            "response": {
                "id": "resp_1",
                "error": { "code": "tool_validation_error" }
            }
        });
        let mut state = StreamState::default();
        let chunks = responses_event_to_openai_chunks(&event, &mut state);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "tool_error");
    }

    #[test]
    fn responses_completed_tool_call_event_produces_openai_tool_calls_finish() {
        let mut state = StreamState::default();
        let _ = responses_event_to_openai_chunks(
            &serde_json::json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "id": "fc_item_1",
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup"
                }
            }),
            &mut state,
        );
        let chunks = responses_event_to_openai_chunks(
            &serde_json::json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "status": "completed",
                    "output": [{
                        "id": "fc_item_1",
                        "type": "function_call",
                        "call_id": "call_1",
                        "name": "lookup",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }]
                }
            }),
            &mut state,
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn responses_custom_tool_call_events_preserve_custom_type_and_input_deltas() {
        let mut state = StreamState::default();
        let added_chunks = responses_event_to_openai_chunks(
            &serde_json::json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "id": "custom_item_1",
                    "type": "custom_tool_call",
                    "call_id": "call_1",
                    "name": "code_exec",
                    "input": "print('"
                }
            }),
            &mut state,
        );
        let delta_chunks = responses_event_to_openai_chunks(
            &serde_json::json!({
                "type": "response.custom_tool_call_input.delta",
                "output_index": 0,
                "item_id": "custom_item_1",
                "delta": "hi')"
            }),
            &mut state,
        );

        assert_eq!(added_chunks[0]["choices"][0]["delta"]["tool_calls"][0]["type"], "custom");
        assert_eq!(
            added_chunks[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            "print('"
        );
        assert_eq!(
            delta_chunks[0]["choices"][0]["delta"]["tool_calls"][0]["type"],
            "custom"
        );
        assert_eq!(
            delta_chunks[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            "hi')"
        );
    }

    #[test]
    fn responses_function_call_argument_delta_binds_by_item_identity() {
        let mut state = StreamState::default();
        let _ = responses_event_to_openai_chunks(
            &serde_json::json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "id": "fc_item_0",
                    "type": "function_call",
                    "call_id": "call_0",
                    "name": "first"
                }
            }),
            &mut state,
        );
        let _ = responses_event_to_openai_chunks(
            &serde_json::json!({
                "type": "response.output_item.added",
                "output_index": 1,
                "item": {
                    "id": "fc_item_1",
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "second"
                }
            }),
            &mut state,
        );

        let chunks = responses_event_to_openai_chunks(
            &serde_json::json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "fc_item_0",
                "output_index": 0,
                "delta": "{\"city\":\"Tokyo\"}"
            }),
            &mut state,
        );

        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0]["choices"][0]["delta"]["tool_calls"][0]["index"],
            0
        );
        assert_eq!(
            state
                .openai_tool_calls
                .get(&0)
                .expect("first tool")
                .arguments,
            "{\"city\":\"Tokyo\"}"
        );
        assert_eq!(
            state
                .openai_tool_calls
                .get(&1)
                .expect("second tool")
                .arguments,
            ""
        );
    }

    #[test]
    fn openai_chunk_to_responses_sse_allocates_unique_output_indices_per_item_kind() {
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
        let text_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": { "content": "Hi" },
                "finish_reason": null
            }]
        });
        let tool_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "lookup", "arguments": "{\"city\":\"Tokyo\"}" }
                    }]
                },
                "finish_reason": null
            }]
        });

        let joined = openai_chunk_to_responses_sse(&reasoning_chunk, &mut state)
            .into_iter()
            .chain(openai_chunk_to_responses_sse(&text_chunk, &mut state))
            .chain(openai_chunk_to_responses_sse(&tool_chunk, &mut state))
            .map(|bytes| parse_sse_json(&bytes))
            .filter(|event| {
                event.get("type").and_then(Value::as_str) == Some("response.output_item.added")
            })
            .collect::<Vec<_>>();

        assert_eq!(joined.len(), 3, "events = {joined:?}");
        let indices = joined
            .iter()
            .map(|event| event["output_index"].as_u64().expect("output index"))
            .collect::<Vec<_>>();
        let unique = indices
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), 3, "indices = {indices:?}");
        assert_eq!(joined[0]["item"]["type"], "reasoning");
        assert_eq!(joined[1]["item"]["type"], "message");
        assert_eq!(joined[2]["item"]["type"], "function_call");
    }

    #[test]
    fn openai_chat_stream_to_responses_allocates_distinct_output_indices_for_reasoning_message_and_tool_calls(
    ) {
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
        let text_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": { "content": "Hi" },
                "finish_reason": null
            }]
        });
        let tool_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "lookup", "arguments": "{\"city\":\"Tokyo\"}" }
                    }]
                },
                "finish_reason": null
            }]
        });

        let output_item_indices = openai_chunk_to_responses_sse(&reasoning_chunk, &mut state)
            .into_iter()
            .chain(openai_chunk_to_responses_sse(&text_chunk, &mut state))
            .chain(openai_chunk_to_responses_sse(&tool_chunk, &mut state))
            .map(|bytes| parse_sse_json(&bytes))
            .filter(|event| {
                event.get("type").and_then(Value::as_str) == Some("response.output_item.added")
            })
            .map(|event| event["output_index"].as_u64().expect("output index"))
            .collect::<Vec<_>>();

        let unique = output_item_indices
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            output_item_indices.len(),
            3,
            "indices = {output_item_indices:?}"
        );
        assert_eq!(unique.len(), 3, "indices = {output_item_indices:?}");
    }

    #[test]
    fn responses_terminal_usage_preserves_cache_and_reasoning_details() {
        let event = serde_json::json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_1",
                "incomplete_details": { "reason": "max_output_tokens" },
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 7,
                    "total_tokens": 18,
                    "input_tokens_details": { "cached_tokens": 3 },
                    "output_tokens_details": { "reasoning_tokens": 2 }
                }
            }
        });
        let mut state = StreamState::default();
        let chunks = responses_event_to_openai_chunks(&event, &mut state);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["usage"]["total_tokens"], 18);
        assert_eq!(
            chunks[0]["usage"]["prompt_tokens_details"]["cached_tokens"],
            3
        );
        assert_eq!(
            chunks[0]["usage"]["completion_tokens_details"]["reasoning_tokens"],
            2
        );
    }

    #[test]
    fn claude_pause_turn_stream_maps_to_openai_pause_turn_finish() {
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
                "type": "message_delta",
                "delta": { "stop_reason": "pause_turn" }
            }),
            &mut state,
        );
        let chunks = claude_event_to_openai_chunks(
            &serde_json::json!({ "type": "message_stop" }),
            &mut state,
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "pause_turn");
    }

    #[test]
    fn claude_refusal_stream_maps_to_openai_content_filter_finish() {
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
                "type": "message_delta",
                "delta": { "stop_reason": "refusal" }
            }),
            &mut state,
        );
        let chunks = claude_event_to_openai_chunks(
            &serde_json::json!({ "type": "message_stop" }),
            &mut state,
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "content_filter");
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
    fn openai_chunk_to_claude_sse_maps_context_window_finish_reason() {
        let chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "context_length_exceeded" }]
        });
        let mut state = StreamState::default();
        let out = openai_chunk_to_claude_sse(&chunk, &mut state);
        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"stop_reason\":\"model_context_window_exceeded\""));
    }

    #[test]
    fn openai_chunk_to_claude_sse_emits_error_event_for_error_finish() {
        let chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "error" }]
        });
        let mut state = StreamState::default();
        let out = openai_chunk_to_claude_sse(&chunk, &mut state);
        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"type\":\"error\""));
        assert!(joined.contains("\"api_error\""));
        assert!(!joined.contains("\"stop_reason\":\"end_turn\""));
        assert!(!joined.contains("\"type\":\"message_stop\""));
    }

    #[test]
    fn openai_chunk_to_claude_sse_emits_error_event_for_tool_error_finish() {
        let chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_error" }]
        });
        let mut state = StreamState::default();
        let out = openai_chunk_to_claude_sse(&chunk, &mut state);
        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"type\":\"error\""));
        assert!(joined.contains("\"invalid_request_error\""));
        assert!(!joined.contains("\"stop_reason\":\"end_turn\""));
        assert!(!joined.contains("\"type\":\"message_stop\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_maps_pause_turn_to_incomplete() {
        let mut state = StreamState::default();
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "pause_turn" }]
        });
        let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"type\":\"response.incomplete\""));
        assert!(joined.contains("\"reason\":\"pause_turn\""));
    }

    #[test]
    fn openai_chunk_to_claude_sse_rejects_reasoning_without_provenance() {
        let reasoning_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": { "role": "assistant", "reasoning_content": "think" }, "finish_reason": null }]
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
        assert!(joined.contains("event: error"));
        assert!(joined.contains("\"type\":\"error\""));
        assert!(joined.contains("\"invalid_request_error\""));
        assert!(joined.contains("reasoning"));
        assert!(joined.contains("provenance"));
        assert!(!joined.contains("message_start"));
        assert!(!joined.contains("text_delta"));
        assert!(!joined.contains("message_stop"));
    }

    #[test]
    fn openai_chunk_to_claude_sse_drops_followup_chunks_after_reasoning_rejection() {
        let reasoning_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": { "reasoning_content": "think" }, "finish_reason": null }]
        });
        let content_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 },
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });
        let mut state = StreamState::default();
        let joined = openai_chunk_to_claude_sse(&reasoning_chunk, &mut state)
            .into_iter()
            .chain(openai_chunk_to_claude_sse(&content_chunk, &mut state))
            .chain(openai_chunk_to_claude_sse(&finish_chunk, &mut state))
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("event: error"));
        assert_eq!(joined.matches("event: error").count(), 1, "{joined}");
        assert!(!joined.contains("text_delta"));
        assert!(!joined.contains("message_stop"));
    }

    #[test]
    fn openai_chunk_to_claude_sse_rejects_custom_tool_calls_without_downgrading() {
        let custom_tool_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_custom",
                        "type": "custom",
                        "function": {
                            "name": "code_exec",
                            "arguments": "print('hi')"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });

        let mut state = StreamState::default();
        let joined = openai_chunk_to_claude_sse(&custom_tool_chunk, &mut state)
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("event: error"), "{joined}");
        assert!(joined.contains("\"type\":\"error\""), "{joined}");
        assert!(joined.contains("custom tools"), "{joined}");
        assert!(!joined.contains("message_start"), "{joined}");
        assert!(!joined.contains("content_block_start"), "{joined}");
        assert!(!joined.contains("tool_use"), "{joined}");
        assert!(!joined.contains("input_json_delta"), "{joined}");
    }

    #[test]
    fn openai_chunk_to_claude_sse_drops_followup_chunks_after_custom_tool_rejection() {
        let custom_tool_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_custom",
                        "type": "custom",
                        "function": {
                            "name": "code_exec",
                            "arguments": "print('hi')"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let content_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 },
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });

        let mut state = StreamState::default();
        let joined = openai_chunk_to_claude_sse(&custom_tool_chunk, &mut state)
            .into_iter()
            .chain(openai_chunk_to_claude_sse(&content_chunk, &mut state))
            .chain(openai_chunk_to_claude_sse(&finish_chunk, &mut state))
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(joined.matches("event: error").count(), 1, "{joined}");
        assert!(!joined.contains("text_delta"), "{joined}");
        assert!(!joined.contains("message_stop"), "{joined}");
        assert!(!joined.contains("tool_use"), "{joined}");
    }

    #[test]
    fn translate_sse_event_responses_to_anthropic_rejects_custom_tool_call_and_suppresses_followups() {
        let added_event = serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "id": "ctc_1",
                "type": "custom_tool_call",
                "call_id": "call_custom",
                "name": "code_exec",
                "input": "print('"
            }
        });
        let delta_event = serde_json::json!({
            "type": "response.custom_tool_call_input.delta",
            "output_index": 0,
            "item_id": "ctc_1",
            "delta": "hi')"
        });
        let complete_event = serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 1,
                "status": "completed",
                "output": [{
                    "type": "custom_tool_call",
                    "call_id": "call_custom",
                    "name": "code_exec",
                    "input": "print('hi')"
                }]
            }
        });

        let mut state = StreamState::default();
        let first = translate_sse_event(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            &added_event,
            &mut state,
        );
        let first_joined = first
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(first_joined.contains("event: error"), "{first_joined}");
        assert!(!first_joined.contains("tool_use"), "{first_joined}");

        let second = translate_sse_event(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            &delta_event,
            &mut state,
        );
        assert!(second.is_empty(), "{second:?}");

        let third = translate_sse_event(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            &complete_event,
            &mut state,
        );
        assert!(third.is_empty(), "{third:?}");
    }

    #[test]
    fn translate_sse_event_responses_to_anthropic_does_not_fallback_after_reasoning_rejection() {
        let reasoning_event = serde_json::json!({
            "type": "response.reasoning_summary_text.delta",
            "delta": "think"
        });
        let content_event = serde_json::json!({
            "type": "response.output_text.delta",
            "delta": "Hi"
        });
        let complete_event = serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 1,
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "Hi" }]
                }]
            }
        });

        let mut state = StreamState::default();
        let error_joined = translate_sse_event(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            &reasoning_event,
            &mut state,
        )
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
        assert!(error_joined.contains("event: error"), "{error_joined}");

        let content_out = translate_sse_event(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            &content_event,
            &mut state,
        );
        assert!(content_out.is_empty(), "{content_out:?}");

        let finish_out = translate_sse_event(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            &complete_event,
            &mut state,
        );
        assert!(finish_out.is_empty(), "{finish_out:?}");
    }

    #[test]
    fn openai_chunk_to_claude_sse_translates_usage_to_anthropic_shape() {
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 18
            },
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });
        let mut state = StreamState::default();
        let joined = openai_chunk_to_claude_sse(&finish_chunk, &mut state)
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"usage\":{\"input_tokens\":11,\"output_tokens\":7"));
        assert!(!joined.contains("\"prompt_tokens\""));
        assert!(!joined.contains("\"completion_tokens\""));
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
    fn openai_chunk_to_gemini_sse_maps_portable_finish_reason_names() {
        let chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }]
        });
        let mut state = StreamState::default();
        let out = openai_chunk_to_gemini_sse(&chunk, &mut state);
        let joined = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"finishReason\":\"MAX_TOKENS\""));
        assert!(!joined.contains("\"finishReason\":\"LENGTH\""));
    }

    #[test]
    fn openai_chunk_to_gemini_sse_adds_dummy_signature_to_first_tool_call() {
        let chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call_1",
                            "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                        },
                        {
                            "index": 1,
                            "id": "call_2",
                            "function": { "name": "lookup_time", "arguments": "{\"city\":\"Tokyo\"}" }
                        }
                    ]
                },
                "finish_reason": null
            }]
        });
        let mut state = StreamState::default();
        let out = openai_chunk_to_gemini_sse(&chunk, &mut state);
        let joined = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"thoughtSignature\":\"skip_thought_signature_validator\""));
        assert_eq!(joined.matches("\"thoughtSignature\"").count(), 1);
    }

    #[test]
    fn openai_chunk_to_gemini_sse_emits_tool_calls_in_call_index_order() {
        let mut state = StreamState::default();
        let higher_index_first = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 1,
                        "id": "call_1",
                        "function": {
                            "name": "lookup_time",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let lower_index_later = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_0",
                        "function": {
                            "name": "lookup_weather",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });

        let out1 = openai_chunk_to_gemini_sse(&higher_index_first, &mut state);
        assert!(out1.is_empty(), "higher index should wait for lower index");

        let out2 = openai_chunk_to_gemini_sse(&lower_index_later, &mut state);
        assert_eq!(out2.len(), 1);
        let payload = parse_sse_json(&out2[0]);
        let parts = payload["candidates"][0]["content"]["parts"]
            .as_array()
            .expect("parts");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["functionCall"]["id"], "call_0");
        assert_eq!(
            parts[0]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
        assert_eq!(parts[1]["functionCall"]["id"], "call_1");
        assert!(parts[1].get("thoughtSignature").is_none());
    }

    #[test]
    fn openai_chunk_to_gemini_sse_waits_for_earlier_incomplete_tool_before_later_parseable_tool() {
        let mut state = StreamState::default();
        let first_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_0",
                        "function": {
                            "name": "lookup_weather",
                            "arguments": "{\"city\":\"To"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let second_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 1,
                        "id": "call_1",
                        "function": {
                            "name": "lookup_time",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let third_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {
                            "arguments": "kyo\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });

        assert!(openai_chunk_to_gemini_sse(&first_chunk, &mut state).is_empty());
        let out2 = openai_chunk_to_gemini_sse(&second_chunk, &mut state);
        assert!(
            out2.is_empty(),
            "later parseable tool must wait for earlier incomplete one: {out2:?}"
        );

        let out3 = openai_chunk_to_gemini_sse(&third_chunk, &mut state);
        assert_eq!(out3.len(), 1);
        let payload = parse_sse_json(&out3[0]);
        let parts = payload["candidates"][0]["content"]["parts"]
            .as_array()
            .expect("gemini parts");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["functionCall"]["id"], "call_0");
        assert_eq!(parts[1]["functionCall"]["id"], "call_1");
        assert_eq!(
            parts[0]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
        assert!(parts[1].get("thoughtSignature").is_none());
    }

    #[test]
    fn gemini_event_to_openai_chunks_maps_portable_finish_and_reasoning_usage() {
        let event = serde_json::json!({
            "response": {
                "responseId": "gem_resp_1",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": { "parts": [{ "text": "Hi" }], "role": "model" },
                    "finishReason": "SAFETY"
                }],
                "usageMetadata": {
                    "promptTokenCount": 11,
                    "candidatesTokenCount": 5,
                    "thoughtsTokenCount": 2,
                    "totalTokenCount": 18,
                    "cachedContentTokenCount": 3
                }
            }
        });
        let mut state = StreamState::default();
        let chunks = gemini_event_to_openai_chunks(&event, &mut state);
        let finish_chunk = chunks
            .iter()
            .find(|chunk| chunk["choices"][0]["finish_reason"].is_string())
            .expect("finish chunk");
        assert_eq!(
            finish_chunk["choices"][0]["finish_reason"],
            "content_filter"
        );
        assert_eq!(finish_chunk["usage"]["total_tokens"], 18);
        assert_eq!(
            finish_chunk["usage"]["prompt_tokens_details"]["cached_tokens"],
            3
        );
        assert_eq!(
            finish_chunk["usage"]["completion_tokens_details"]["reasoning_tokens"],
            2
        );
    }

    #[test]
    fn gemini_event_to_openai_chunks_handles_prompt_feedback_without_candidates() {
        let event = serde_json::json!({
            "response": {
                "responseId": "gem_resp_feedback",
                "modelVersion": "gemini-2.5",
                "promptFeedback": { "blockReason": "SAFETY" },
                "usageMetadata": {
                    "promptTokenCount": 3,
                    "totalTokenCount": 3
                }
            }
        });
        let mut state = StreamState::default();
        let chunks = gemini_event_to_openai_chunks(&event, &mut state);
        let finish_chunk = chunks
            .iter()
            .find(|chunk| chunk["choices"][0]["finish_reason"].is_string())
            .expect("finish chunk");

        assert_eq!(
            finish_chunk["choices"][0]["finish_reason"],
            "content_filter"
        );
        assert_eq!(finish_chunk["usage"]["prompt_tokens"], 3);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk["choices"][0]["delta"].get("role").is_none()),
            "prompt block should not fabricate assistant role: {chunks:?}"
        );
    }

    #[test]
    fn gemini_candidate_less_partial_rejects_instead_of_emitting_role_only_chunk() {
        let event = serde_json::json!({
            "response": {
                "responseId": "gem_resp_partial",
                "modelVersion": "gemini-2.5",
                "usageMetadata": {
                    "promptTokenCount": 3,
                    "totalTokenCount": 3
                }
            }
        });
        let mut state = StreamState::default();
        let chunks = gemini_event_to_openai_chunks(&event, &mut state);

        assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
        assert!(chunks[0]["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("candidate"));
        assert!(chunks[0]["choices"][0]["delta"].as_object().unwrap().is_empty());
    }

    #[test]
    fn gemini_multi_candidate_rejects_instead_of_silently_using_candidate_zero() {
        let event = serde_json::json!({
            "response": {
                "responseId": "gem_resp_multi",
                "modelVersion": "gemini-2.5",
                "candidates": [
                    {
                        "content": { "parts": [{ "text": "candidate-0" }], "role": "model" },
                        "finishReason": "STOP"
                    },
                    {
                        "content": { "parts": [{ "text": "candidate-1" }], "role": "model" },
                        "finishReason": "STOP"
                    }
                ],
                "usageMetadata": {
                    "promptTokenCount": 3,
                    "candidatesTokenCount": 5,
                    "totalTokenCount": 8
                }
            }
        });
        let mut state = StreamState::default();
        let chunks = gemini_event_to_openai_chunks(&event, &mut state);

        assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
        assert!(chunks[0]["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("multiple candidates"));
        let rendered = serde_json::to_string(&chunks).expect("render chunks");
        assert!(!rendered.contains("candidate-0"), "{rendered}");
        assert!(!rendered.contains("candidate-1"), "{rendered}");
    }

    #[test]
    fn translate_sse_event_gemini_cross_frame_multi_candidate_rejects_and_suppresses_followups() {
        let mut state = StreamState::default();

        let first = translate_sse_event(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "response": {
                    "responseId": "gem_resp_multi",
                    "modelVersion": "gemini-2.5",
                    "candidates": [{
                        "index": 1,
                        "content": { "parts": [{ "text": "candidate-1" }], "role": "model" }
                    }]
                }
            }),
            &mut state,
        );
        let second = translate_sse_event(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "response": {
                    "responseId": "gem_resp_multi",
                    "modelVersion": "gemini-2.5",
                    "candidates": [{
                        "index": 0,
                        "content": { "parts": [{ "text": "candidate-0" }], "role": "model" }
                    }]
                }
            }),
            &mut state,
        );

        let first_joined = first
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(first_joined.contains("\"finish_reason\":\"error\""));
        assert!(first_joined.contains("multiple candidates"));
        assert!(second.is_empty(), "follow-up after fatal reject should be suppressed");
    }

    #[test]
    fn translate_sse_event_openai_to_non_openai_single_frame_multi_choice_fails_closed() {
        for client_format in [UpstreamFormat::Google, UpstreamFormat::OpenAiResponses] {
            let mut state = StreamState::default();
            let out = translate_sse_event(
                UpstreamFormat::OpenAiCompletion,
                client_format,
                &serde_json::json!({
                    "id": "chatcmpl-msg123",
                    "model": "gpt-4o",
                    "choices": [
                        {
                            "index": 0,
                            "delta": { "content": "candidate-0" },
                            "finish_reason": null
                        },
                        {
                            "index": 1,
                            "delta": { "content": "candidate-1" },
                            "finish_reason": null
                        }
                    ]
                }),
                &mut state,
            );

            let joined = out
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !out.is_empty(),
                "fatal reject should emit a terminal compatibility error for {client_format:?}"
            );
            assert!(
                state.fatal_rejection.is_some(),
                "client_format = {client_format:?}, out = {joined}"
            );
            assert!(
                !joined.contains("candidate-0") && !joined.contains("candidate-1"),
                "fatal reject must not leak mixed-choice content for {client_format:?}: {joined}"
            );
        }
    }

    #[test]
    fn translate_sse_event_openai_to_non_openai_cross_frame_multi_choice_rejects_and_suppresses() {
        for client_format in [UpstreamFormat::Google, UpstreamFormat::OpenAiResponses] {
            let mut state = StreamState::default();
            let first = translate_sse_event(
                UpstreamFormat::OpenAiCompletion,
                client_format,
                &serde_json::json!({
                    "id": "chatcmpl-msg123",
                    "model": "gpt-4o",
                    "choices": [{
                        "index": 0,
                        "delta": { "content": "candidate-0" },
                        "finish_reason": null
                    }]
                }),
                &mut state,
            );
            assert!(
                state.fatal_rejection.is_none(),
                "single-choice frame should not be rejected early for {client_format:?}: {first:?}"
            );
            let second = translate_sse_event(
                UpstreamFormat::OpenAiCompletion,
                client_format,
                &serde_json::json!({
                    "id": "chatcmpl-msg123",
                    "model": "gpt-4o",
                    "choices": [{
                        "index": 1,
                        "delta": { "content": "candidate-1" },
                        "finish_reason": null
                    }]
                }),
                &mut state,
            );
            let third = translate_sse_event(
                UpstreamFormat::OpenAiCompletion,
                client_format,
                &serde_json::json!({
                    "id": "chatcmpl-msg123",
                    "model": "gpt-4o",
                    "choices": [{
                        "index": 0,
                        "delta": { "content": "after-reject" },
                        "finish_reason": null
                    }]
                }),
                &mut state,
            );

            let second_joined = second
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !second.is_empty(),
                "mixed-choice rejection should emit a terminal compatibility error for {client_format:?}"
            );
            assert!(
                state.fatal_rejection.is_some(),
                "second frame should fatal reject mixed choice stream for {client_format:?}: {second_joined}"
            );
            assert!(
                !second_joined.contains("candidate-1"),
                "fatal reject must not leak mixed-choice content for {client_format:?}: {second_joined}"
            );
            assert!(
                third.is_empty(),
                "follow-up after fatal reject should be suppressed for {client_format:?}"
            );
        }
    }

    #[test]
    fn gemini_stream_non_success_finish_reasons_do_not_collapse_to_success() {
        let cases = [
            ("MALFORMED_FUNCTION_CALL", "tool_error"),
            ("UNEXPECTED_TOOL_CALL", "tool_error"),
            ("TOO_MANY_TOOL_CALLS", "tool_error"),
            ("MISSING_THOUGHT_SIGNATURE", "tool_error"),
            ("IMAGE_OTHER", "error"),
            ("NO_IMAGE", "error"),
            ("LANGUAGE", "error"),
        ];

        for (reason, expected) in cases {
            let event = serde_json::json!({
                "response": {
                    "responseId": format!("gem_{reason}"),
                    "modelVersion": "gemini-2.5",
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{
                                "functionCall": {
                                    "id": "call_1",
                                    "name": "lookup_weather",
                                    "args": { "city": "Tokyo" }
                                }
                            }]
                        },
                        "finishReason": reason
                    }]
                }
            });
            let mut state = StreamState::default();
            let chunks = gemini_event_to_openai_chunks(&event, &mut state);
            let finish_chunk = chunks
                .iter()
                .find(|chunk| chunk["choices"][0]["finish_reason"].is_string())
                .expect("finish chunk");
            assert_eq!(
                finish_chunk["choices"][0]["finish_reason"], expected,
                "reason = {reason}, chunk = {finish_chunk:?}"
            );
        }
    }

    #[test]
    fn openai_chunk_to_gemini_sse_waits_for_complete_tool_call_arguments() {
        let mut state = StreamState::default();
        let first_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "lookup_weather",
                            "arguments": "{\"city\":\"To"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let second_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {
                            "arguments": "kyo\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });

        let out1 = openai_chunk_to_gemini_sse(&first_chunk, &mut state);
        assert!(
            out1.is_empty(),
            "first fragment should not emit partial args"
        );

        let out2 = openai_chunk_to_gemini_sse(&second_chunk, &mut state);
        assert_eq!(out2.len(), 1);
        let payload = parse_sse_json(&out2[0]);
        let parts = payload["candidates"][0]["content"]["parts"]
            .as_array()
            .expect("gemini parts");
        assert_eq!(parts.len(), 1);
        assert_eq!(
            parts[0]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
        assert_eq!(parts[0]["functionCall"]["id"], "call_1");
        assert_eq!(parts[0]["functionCall"]["name"], "lookup_weather");
        assert_eq!(parts[0]["functionCall"]["args"]["city"], "Tokyo");
    }

    #[test]
    fn openai_refusal_stream_maps_to_gemini_text_part_and_safety_terminal() {
        let mut state = StreamState::default();
        let refusal_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": { "refusal": "Cannot comply" },
                "finish_reason": null
            }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "content_filter"
            }]
        });

        let out1 = openai_chunk_to_gemini_sse(&refusal_chunk, &mut state);
        let out2 = openai_chunk_to_gemini_sse(&finish_chunk, &mut state);

        assert_eq!(out1.len(), 1, "out1 = {out1:?}");
        let refusal_payload = parse_sse_json(&out1[0]);
        assert_eq!(
            refusal_payload["candidates"][0]["content"]["parts"][0]["text"],
            "Cannot comply"
        );
        assert_eq!(out2.len(), 1, "out2 = {out2:?}");
        let finish_payload = parse_sse_json(&out2[0]);
        assert_eq!(finish_payload["candidates"][0]["finishReason"], "SAFETY");
    }

    #[test]
    fn gemini_inline_data_output_rejects_instead_of_silent_drop() {
        let mut state = StreamState::default();
        let chunks = gemini_event_to_openai_chunks(
            &serde_json::json!({
                "response": {
                    "responseId": "gem_resp_inline",
                    "modelVersion": "gemini-2.5",
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{
                                "inlineData": {
                                    "mimeType": "image/png",
                                    "data": "AAAA"
                                }
                            }]
                        }
                    }]
                }
            }),
            &mut state,
        );

        assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
        assert!(
            chunks[0]["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("inlineData"),
            "chunks = {chunks:?}"
        );
    }

    #[test]
    fn openai_chunk_to_gemini_sse_adds_dummy_signature_to_first_parseable_tool_call() {
        let mut state = StreamState::default();
        let first_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_0",
                        "function": {
                            "name": "lookup_weather",
                            "arguments": "{\"city\":\"To"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let second_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 1,
                        "id": "call_1",
                        "function": {
                            "name": "lookup_time",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });
        let third_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {
                            "arguments": "kyo\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });

        let out1 = openai_chunk_to_gemini_sse(&first_chunk, &mut state);
        assert!(out1.is_empty());

        let out2 = openai_chunk_to_gemini_sse(&second_chunk, &mut state);
        assert!(
            out2.is_empty(),
            "later tool calls must wait for earlier incomplete indices"
        );

        let out3 = openai_chunk_to_gemini_sse(&third_chunk, &mut state);
        assert_eq!(out3.len(), 1);
        let payload = parse_sse_json(&out3[0]);
        let parts = payload["candidates"][0]["content"]["parts"]
            .as_array()
            .expect("gemini parts");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["functionCall"]["id"], "call_0");
        assert_eq!(parts[1]["functionCall"]["id"], "call_1");
        assert_eq!(
            parts[0]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
        assert!(parts[1].get("thoughtSignature").is_none());
    }

    #[test]
    fn translate_sse_event_openai_to_gemini_suppresses_usage_only_chunk_instead_of_leaking_openai_json()
    {
        let mut state = StreamState::default();
        let out = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &serde_json::json!({
                "id": "chatcmpl-msg123",
                "model": "gpt-4o",
                "usage": {
                    "prompt_tokens": 11,
                    "completion_tokens": 7,
                    "total_tokens": 18
                },
                "choices": []
            }),
            &mut state,
        );

        assert!(out.is_empty(), "usage-only chunk should be buffered/suppressed");
    }

    #[test]
    fn translate_sse_event_openai_to_gemini_suppresses_incomplete_tool_args_chunk_instead_of_leaking_openai_json(
    ) {
        let mut state = StreamState::default();
        let out = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &serde_json::json!({
                "id": "chatcmpl-msg123",
                "model": "gpt-4o",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_1",
                            "function": {
                                "name": "lookup_weather",
                                "arguments": "{\"city\":\"To"
                            }
                        }]
                    },
                    "finish_reason": null
                }]
            }),
            &mut state,
        );

        assert!(out.is_empty(), "incomplete tool args should not leak raw OpenAI chunk");
    }

    #[test]
    fn openai_chunk_to_responses_sse_maps_error_finish_to_failed() {
        let mut state = StreamState::default();
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "error" }]
        });
        let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"type\":\"response.failed\""));
        assert!(joined.contains("\"code\":\"error\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_emits_refusal_events() {
        let mut state = StreamState::default();
        let role_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null }]
        });
        let refusal_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "refusal": "Cannot comply" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "content_filter" }]
        });

        let _ = openai_chunk_to_responses_sse(&role_chunk, &mut state);
        let out1 = openai_chunk_to_responses_sse(&refusal_chunk, &mut state);
        let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out1
            .into_iter()
            .chain(out2)
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("response.refusal.delta"));
        assert!(joined.contains("response.refusal.done"));
        assert!(joined.contains("\"type\":\"refusal\""));
        assert!(joined.contains("\"refusal\":\"Cannot comply\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_preserves_text_and_refusal_parts_in_terminal_output() {
        let mut state = StreamState::default();
        let role_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null }]
        });
        let text_chunk_1 = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": "Visible" }, "finish_reason": null }]
        });
        let refusal_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "refusal": "Denied" }, "finish_reason": null }]
        });
        let text_chunk_2 = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": " answer" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "content_filter" }]
        });

        let _ = openai_chunk_to_responses_sse(&role_chunk, &mut state);
        let _ = openai_chunk_to_responses_sse(&text_chunk_1, &mut state);
        let _ = openai_chunk_to_responses_sse(&refusal_chunk, &mut state);
        let _ = openai_chunk_to_responses_sse(&text_chunk_2, &mut state);
        let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);

        let terminal = out
            .iter()
            .map(|bytes| parse_sse_json(bytes))
            .find(|event| {
                matches!(
                    event.get("type").and_then(Value::as_str),
                    Some("response.completed") | Some("response.incomplete")
                )
            })
            .expect("terminal response event");
        let content = terminal["response"]["output"][0]["content"]
            .as_array()
            .expect("message content array");

        assert_eq!(content.len(), 2, "content = {content:?}");
        assert_eq!(content[0]["type"], "output_text");
        assert_eq!(content[0]["text"], "Visible answer");
        assert_eq!(content[1]["type"], "refusal");
        assert_eq!(content[1]["refusal"], "Denied");
    }

    #[test]
    fn claude_citations_delta_preserves_annotations_through_responses_terminal() {
        let mut state = StreamState::default();
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-3" }
            }),
            &mut state,
        );
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" }
            }),
            &mut state,
        );
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "Fact" }
            }),
            &mut state,
        );
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "citations_delta",
                    "citation": { "type": "url_citation", "url": "https://example.com/fact" }
                }
            }),
            &mut state,
        );
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn" },
                "usage": { "input_tokens": 2, "output_tokens": 1, "service_tier": "priority" }
            }),
            &mut state,
        );
        let out = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({ "type": "message_stop" }),
            &mut state,
        );

        let terminal = out
            .iter()
            .map(|bytes| parse_sse_json(bytes))
            .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
            .expect("response.completed");
        let content = terminal["response"]["output"][0]["content"]
            .as_array()
            .expect("message content");

        assert_eq!(content[0]["annotations"][0]["url"], "https://example.com/fact");
    }

    #[test]
    fn anthropic_extra_usage_fields_survive_to_responses_completed_usage() {
        let mut state = StreamState::default();
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-3" }
            }),
            &mut state,
        );
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn" },
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "service_tier": "priority",
                    "server_tool_use": { "web_search_requests": 2 }
                }
            }),
            &mut state,
        );

        let out = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({ "type": "message_stop" }),
            &mut state,
        );
        let terminal = out
            .iter()
            .map(|bytes| parse_sse_json(bytes))
            .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
            .expect("response.completed");

        assert_eq!(terminal["response"]["usage"]["service_tier"], "priority");
        assert_eq!(
            terminal["response"]["usage"]["server_tool_use"]["web_search_requests"],
            2
        );
    }

    #[test]
    fn openai_chunk_to_responses_sse_maps_tool_error_finish_to_failed() {
        let mut state = StreamState::default();
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_error" }]
        });
        let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"type\":\"response.failed\""));
        assert!(joined.contains("\"code\":\"tool_error\""));
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
    fn openai_chunk_to_responses_sse_preserves_custom_and_proxied_tool_kinds() {
        let mut state = StreamState::default();
        let custom_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_custom",
                        "type": "custom",
                        "function": { "name": "code_exec", "arguments": "print('hi')" }
                    }]
                },
                "finish_reason": null
            }]
        });
        let proxied_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 1,
                        "id": "call_server",
                        "proxied_tool_kind": "anthropic_server_tool_use",
                        "function": { "name": "web_search", "arguments": "{\"query\":\"rust\"}" }
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

        let out1 = openai_chunk_to_responses_sse(&custom_chunk, &mut state);
        let out2 = openai_chunk_to_responses_sse(&proxied_chunk, &mut state);
        let out3 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out1
            .into_iter()
            .chain(out2)
            .chain(out3)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("response.custom_tool_call_input.delta"));
        assert!(joined.contains("response.custom_tool_call_input.done"));
        assert!(joined.contains("\"type\":\"custom_tool_call\""));
        assert!(joined.contains("\"proxied_tool_kind\":\"anthropic_server_tool_use\""));
    }

    #[test]
    fn anthropic_tool_use_does_not_duplicate_function_call_in_responses_completed() {
        let mut state = StreamState::default();
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-test" }
            }),
            &mut state,
        );
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "exec_command",
                    "input": {}
                }
            }),
            &mut state,
        );
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": { "type": "input_json_delta", "partial_json": "{\"cmd\":\"pwd\"}" }
            }),
            &mut state,
        );
        let _ = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "message_delta",
                "delta": { "stop_reason": "tool_use" },
                "usage": { "input_tokens": 10, "output_tokens": 5 }
            }),
            &mut state,
        );
        let out = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({ "type": "message_stop" }),
            &mut state,
        );

        let completed = out
            .iter()
            .map(|bytes| parse_sse_json(bytes))
            .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
            .expect("response.completed event");
        let output = completed["response"]["output"]
            .as_array()
            .expect("response output array");
        let function_calls = output
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .collect::<Vec<_>>();
        assert_eq!(function_calls.len(), 1);
        assert_eq!(function_calls[0]["call_id"], "call_1");
    }

    #[test]
    fn claude_tool_use_start_with_input_seeds_openai_arguments() {
        let mut state = StreamState::default();
        let _ = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-test" }
            }),
            &mut state,
        );
        let chunks = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_start",
                "index": 2,
                "content_block": {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "exec_command",
                    "input": { "cmd": "pwd" }
                }
            }),
            &mut state,
        );

        assert_eq!(chunks.len(), 1);
        let tool_calls = chunks[0]["choices"][0]["delta"]["tool_calls"]
            .as_array()
            .expect("tool_calls array");
        assert_eq!(tool_calls[0]["function"]["arguments"], "{\"cmd\":\"pwd\"}");
    }

    #[test]
    fn claude_tool_use_seeded_input_and_json_delta_are_both_preserved() {
        let mut state = StreamState::default();
        let _ = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-test" }
            }),
            &mut state,
        );
        let _ = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_start",
                "index": 2,
                "content_block": {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "exec_command",
                    "input": { "cmd": "pw" }
                }
            }),
            &mut state,
        );
        let delta_chunks = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 2,
                "delta": { "type": "input_json_delta", "partial_json": "d\"}" }
            }),
            &mut state,
        );

        assert_eq!(
            state
                .claude_tool_uses
                .get(&2)
                .expect("tool state")
                .arguments,
            "{\"cmd\":\"pwd\"}"
        );
        assert!(
            !delta_chunks.is_empty(),
            "delta should remain visible when start input was seeded"
        );
        let delta_tool_calls = delta_chunks[0]["choices"][0]["delta"]["tool_calls"]
            .as_array()
            .expect("delta tool_calls");
        assert_eq!(delta_tool_calls[0]["function"]["arguments"], "d\"}");
    }

    #[test]
    fn claude_server_tool_use_is_preserved_in_stream_as_marked_tool_call() {
        let mut state = StreamState::default();
        let _ = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-test" }
            }),
            &mut state,
        );
        let chunks = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {
                    "type": "server_tool_use",
                    "id": "server_1",
                    "name": "web_search",
                    "input": { "query": "rust" }
                }
            }),
            &mut state,
        );

        assert_eq!(chunks.len(), 1);
        let tool_calls = chunks[0]["choices"][0]["delta"]["tool_calls"]
            .as_array()
            .expect("tool calls");
        assert_eq!(tool_calls[0]["function"]["name"], "web_search");
        assert_eq!(
            tool_calls[0]["proxied_tool_kind"],
            "anthropic_server_tool_use"
        );
    }

    #[test]
    fn claude_empty_tool_input_waits_for_delta_arguments() {
        let mut state = StreamState::default();
        let _ = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "message_start",
                "message": { "id": "msg_1", "model": "claude-test" }
            }),
            &mut state,
        );
        let start_chunks = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_start",
                "index": 2,
                "content_block": {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "exec_command",
                    "input": {}
                }
            }),
            &mut state,
        );
        let delta_chunks = claude_event_to_openai_chunks(
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 2,
                "delta": { "type": "input_json_delta", "partial_json": "{\"cmd\":\"pwd\"}" }
            }),
            &mut state,
        );

        let start_tool_calls = start_chunks[0]["choices"][0]["delta"]["tool_calls"]
            .as_array()
            .expect("start tool_calls");
        assert_eq!(start_tool_calls[0]["function"]["arguments"], "");
        let delta_tool_calls = delta_chunks[0]["choices"][0]["delta"]["tool_calls"]
            .as_array()
            .expect("delta tool_calls");
        assert_eq!(
            delta_tool_calls[0]["function"]["arguments"],
            "{\"cmd\":\"pwd\"}"
        );
    }

    #[test]
    fn openai_chunk_to_claude_sse_restores_server_tool_use_from_proxied_tool_kind() {
        let mut state = StreamState::default();
        let chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "server_1",
                        "proxied_tool_kind": "anthropic_server_tool_use",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\":\"rust\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });

        let out = openai_chunk_to_claude_sse(&chunk, &mut state);
        let content_block_start = out
            .iter()
            .map(|bytes| parse_sse_json(bytes))
            .find(|event| event.get("type").and_then(Value::as_str) == Some("content_block_start"))
            .expect("content_block_start event");

        assert_eq!(
            content_block_start["content_block"]["type"],
            "server_tool_use"
        );
    }

    #[test]
    fn openai_chunk_to_claude_sse_preserves_standard_function_tool_use() {
        let mut state = StreamState::default();
        let chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "lookup_weather",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": null
            }]
        });

        let out = openai_chunk_to_claude_sse(&chunk, &mut state);
        let joined = out
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"type\":\"tool_use\""), "{joined}");
        assert!(joined.contains("input_json_delta"), "{joined}");
        assert!(!joined.contains("event: error"), "{joined}");
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
    fn openai_chunk_to_responses_sse_maps_minimax_reasoning_details() {
        let mut state = StreamState::default();
        let reasoning_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "MiniMax-M2.7-highspeed",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": { "reasoning_details": [{ "text": "internal thinking" }] },
                "finish_reason": null
            }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "MiniMax-M2.7-highspeed",
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

        assert!(joined.contains("\"type\":\"response.reasoning_summary_text.delta\""));
        assert!(joined.contains("\"delta\":\"internal thinking\""));
        assert!(!joined.contains("\"type\":\"response.output_text.delta\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_dedupes_minimax_cumulative_text() {
        let mut state = StreamState::default();
        let chunk1 = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "MiniMax-M2.7-highspeed",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": "Hello" }, "finish_reason": null }]
        });
        let chunk2 = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "MiniMax-M2.7-highspeed",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": "Hello world" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "MiniMax-M2.7-highspeed",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });
        let usage_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "MiniMax-M2.7-highspeed",
            "created": 123,
            "choices": [],
            "usage": { "prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12 }
        });

        let out1 = openai_chunk_to_responses_sse(&chunk1, &mut state);
        let out2 = openai_chunk_to_responses_sse(&chunk2, &mut state);
        let _ = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let out3 = openai_chunk_to_responses_sse(&usage_chunk, &mut state);
        let joined = out1
            .into_iter()
            .chain(out2)
            .chain(out3)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"delta\":\"Hello\""));
        assert!(joined.contains("\"delta\":\" world\""));
        assert!(joined.contains("\"text\":\"Hello world\""));
    }

    #[test]
    fn openai_chunk_to_responses_sse_waits_for_usage_only_chunk_before_completed() {
        let mut state = StreamState::default();
        let text_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "MiniMax-M2.7-highspeed",
            "created": 123,
            "choices": [{ "index": 0, "delta": { "content": "Hello" }, "finish_reason": null }]
        });
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "MiniMax-M2.7-highspeed",
            "created": 123,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        });
        let usage_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "MiniMax-M2.7-highspeed",
            "created": 123,
            "choices": [],
            "usage": {
                "prompt_tokens": 42,
                "completion_tokens": 172,
                "total_tokens": 214,
                "completion_tokens_details": { "reasoning_tokens": 162 }
            }
        });

        let _ = openai_chunk_to_responses_sse(&text_chunk, &mut state);
        let finish_out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let usage_out = openai_chunk_to_responses_sse(&usage_chunk, &mut state);
        let finish_joined = finish_out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let usage_joined = usage_out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!finish_joined.contains("\"type\":\"response.completed\""));
        assert!(usage_joined.contains("\"type\":\"response.completed\""));
        assert!(usage_joined.contains("\"total_tokens\":214"));
        assert!(usage_joined.contains("\"output_tokens_details\":{\"reasoning_tokens\":162}"));
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
    fn openai_chunk_to_responses_sse_preserves_usage_on_context_failure() {
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
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "context_length_exceeded" }]
        });

        let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"type\":\"response.failed\""));
        assert!(joined.contains("\"code\":\"context_length_exceeded\""));
        assert!(joined.contains("\"total_tokens\":25"));
        assert!(joined.contains("\"input_tokens_details\":{\"cached_tokens\":3}"));
        assert!(joined.contains("\"output_tokens_details\":{\"reasoning_tokens\":2}"));
    }

    #[test]
    fn openai_chunk_to_responses_sse_preserves_specific_error_on_incompatibility_failure() {
        let mut state = StreamState::default();
        let finish_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "error": {
                "type": "invalid_request_error",
                "code": "unsupported_openai_stream_event",
                "message": "OpenAI streaming response with multiple choices cannot be translated losslessly."
            },
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "error" }]
        });

        let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
        let joined = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("\"type\":\"response.failed\""));
        assert!(joined.contains("\"type\":\"invalid_request_error\""));
        assert!(joined.contains("\"code\":\"unsupported_openai_stream_event\""));
        assert!(joined.contains("multiple choices"));
        assert!(!joined.contains("\"type\":\"server_error\",\"code\":\"error\""));
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

    #[test]
    fn anthropic_error_event_maps_to_responses_failed() {
        let mut state = StreamState::default();
        let out = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &serde_json::json!({
                "type": "error",
                "error": {
                    "type": "overloaded_error",
                    "message": "Overloaded"
                }
            }),
            &mut state,
        );

        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"type\":\"response.failed\""));
        assert!(joined.contains("\"type\":\"server_error\""));
        assert!(joined.contains("\"code\":\"server_is_overloaded\""));
    }

    #[test]
    fn anthropic_error_event_maps_context_to_openai_context_finish() {
        let mut state = StreamState::default();
        let out = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": "maximum context length exceeded"
                }
            }),
            &mut state,
        );

        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("\"finish_reason\":\"context_length_exceeded\""));
        assert!(joined.contains("\"code\":\"context_length_exceeded\""));
        assert!(joined.contains("[DONE]"));
    }

    #[test]
    fn anthropic_error_event_maps_non_specialized_failures_to_openai_error_finish() {
        for (error_type, message) in [
            ("overloaded_error", "Overloaded"),
            ("api_error", "Internal server error"),
            ("rate_limit_error", "Rate limited"),
            ("fallback_error", "Unknown Anthropic failure"),
        ] {
            let mut state = StreamState::default();
            let out = translate_sse_event(
                UpstreamFormat::Anthropic,
                UpstreamFormat::OpenAiCompletion,
                &serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": error_type,
                        "message": message
                    }
                }),
                &mut state,
            );

            let joined = out
                .into_iter()
                .map(|b| String::from_utf8_lossy(&b).to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                joined.contains("\"finish_reason\":\"error\""),
                "expected error finish for {error_type}: {joined}"
            );
            assert!(!joined.contains("\"finish_reason\":\"stop\""));
            assert!(joined.contains("[DONE]"));
        }
    }

    #[test]
    fn anthropic_error_event_preserves_specialized_openai_error_finishes() {
        for (message, finish_reason, code) in [
            (
                "maximum context length exceeded",
                "context_length_exceeded",
                "context_length_exceeded",
            ),
            (
                "Request blocked by content filter refusal",
                "content_filter",
                "content_filter",
            ),
        ] {
            let mut state = StreamState::default();
            let out = translate_sse_event(
                UpstreamFormat::Anthropic,
                UpstreamFormat::OpenAiCompletion,
                &serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "invalid_request_error",
                        "message": message
                    }
                }),
                &mut state,
            );

            let joined = out
                .into_iter()
                .map(|b| String::from_utf8_lossy(&b).to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                joined.contains(&format!("\"finish_reason\":\"{finish_reason}\"")),
                "expected specialized finish for {message}: {joined}"
            );
            assert!(
                joined.contains(&format!("\"code\":\"{code}\"")),
                "expected specialized code for {message}: {joined}"
            );
            assert!(joined.contains("[DONE]"));
        }
    }
}
