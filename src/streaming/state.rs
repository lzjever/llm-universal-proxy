use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFatalRejection {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ClaudeBlockKind {
    Text,
    Thinking,
    ToolUse,
    ServerToolUse,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ClaudeBlockState {
    pub(super) kind: Option<ClaudeBlockKind>,
    pub(super) thinking: String,
    pub(super) signature: Option<String>,
    pub(super) annotations: Vec<Value>,
    pub(super) omitted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ClaudeThinkingProvenanceState {
    pub(super) block_index: usize,
    pub(super) signature: Option<String>,
    pub(super) omitted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponsesMessagePartKind {
    OutputText,
    Refusal,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ResponsesMessagePartState {
    pub(super) kind: Option<ResponsesMessagePartKind>,
    pub(super) text: String,
    pub(super) annotations: Vec<Value>,
}

/// Stream transformer state (per 9router initState).
#[derive(Debug, Default)]
pub struct StreamState {
    pub message_id: Option<String>,
    pub model: Option<String>,
    pub request_scoped_tool_bridge_context: Option<Value>,
    pub openai_tool_call_index: usize,
    pub claude_tool_use_index: usize,
    pub openai_tool_calls: std::collections::HashMap<usize, ToolCallState>,
    pub claude_tool_uses: std::collections::HashMap<usize, ClaudeToolUseState>,
    pub(super) claude_blocks: std::collections::HashMap<usize, ClaudeBlockState>,
    pub(super) claude_thinking_provenance: Vec<ClaudeThinkingProvenanceState>,
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
    pub(super) gemini_candidate_index: Option<usize>,
    pub(super) openai_choice_index: Option<usize>,
    pub openai_role_sent: bool,
    // OpenAI Responses API client output state
    pub responses_seq: u64,
    pub responses_started: bool,
    pub output_item_id: Option<String>,
    pub output_item_added: bool,
    pub responses_content_part_added: bool,
    pub responses_output_text: String,
    pub(super) responses_message_parts: Vec<ResponsesMessagePartState>,
    pub(super) responses_text_part_index: Option<usize>,
    pub(super) responses_refusal_part_index: Option<usize>,
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
    pub non_replayable_marker: Option<Value>,
    pub custom_input_text: Option<String>,
    pub custom_input_done: bool,
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
    pub proxied_tool_kind: Option<String>,
    pub zero_arg_candidate: bool,
    pub saw_input_json_delta: bool,
    pub arguments_seeded_from_start: bool,
    pub start_arguments_emitted: bool,
    pub finalized: bool,
}

pub(super) fn dedupe_tool_call_state_by_call_id(
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
            if entry.non_replayable_marker.is_none() {
                entry.non_replayable_marker = existing_entry.non_replayable_marker.clone();
            }
            if entry.custom_input_text.is_none() {
                entry.custom_input_text = existing_entry.custom_input_text.clone();
            }
            entry.custom_input_done |= existing_entry.custom_input_done;
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

pub(super) fn merge_seeded_tool_arguments(seed: &str, delta: &str) -> String {
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

pub(super) fn openai_stream_tool_call_type(value: &Value) -> &'static str {
    match value.get("type").and_then(Value::as_str) {
        Some("custom") | Some("custom_tool_call") => "custom",
        _ => "function",
    }
}

pub(super) fn escape_json_string_content_stream(value: &str) -> String {
    serde_json::to_string(value)
        .map(|quoted| quoted[1..quoted.len().saturating_sub(1)].to_string())
        .unwrap_or_default()
}

pub(super) fn openai_custom_bridge_start_delta_stream(input: &str) -> String {
    format!("{{\"input\":\"{}", escape_json_string_content_stream(input))
}

pub(super) fn openai_custom_bridge_input_delta_stream(input: &str) -> String {
    escape_json_string_content_stream(input)
}

pub(super) fn openai_custom_bridge_done_delta_stream() -> &'static str {
    "\"}"
}

pub(super) fn openai_custom_bridge_decode_arguments_stream(arguments: &str) -> Option<String> {
    let value: Value = serde_json::from_str(arguments).ok()?;
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    object
        .get("input")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(super) fn request_scoped_openai_custom_bridge_expects_canonical_input_wrapper_stream(
    bridge_context: Option<&Value>,
    name: &str,
) -> bool {
    let Some(entry) = bridge_context
        .and_then(|ctx| ctx.get("entries"))
        .and_then(Value::as_object)
        .and_then(|entries| entries.get(name))
        .and_then(Value::as_object)
    else {
        return false;
    };

    entry.get("transport_kind").and_then(Value::as_str) == Some("function_object_wrapper")
        && entry.get("wrapper_field").and_then(Value::as_str) == Some("input")
        && entry
            .get("expected_canonical_shape")
            .and_then(Value::as_str)
            == Some("single_required_string")
}

pub(super) fn gemini_candidate_index(candidate: &Value) -> usize {
    candidate
        .get("index")
        .or_else(|| candidate.get("candidateIndex"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize
}

pub(super) fn tool_call_state_type(state: &ToolCallState) -> &str {
    state.tool_type.as_deref().unwrap_or("function")
}

pub(super) fn responses_tool_call_item_type(tool_type: &str) -> &'static str {
    if tool_type == "custom" {
        "custom_tool_call"
    } else {
        "function_call"
    }
}

pub(super) fn responses_tool_call_delta_event_type(tool_type: &str) -> &'static str {
    if tool_type == "custom" {
        "response.custom_tool_call_input.delta"
    } else {
        "response.function_call_arguments.delta"
    }
}

pub(super) fn responses_tool_call_done_event_type(tool_type: &str) -> &'static str {
    if tool_type == "custom" {
        "response.custom_tool_call_input.done"
    } else {
        "response.function_call_arguments.done"
    }
}

pub(super) fn responses_tool_call_payload_field(tool_type: &str) -> &'static str {
    if tool_type == "custom" {
        "input"
    } else {
        "arguments"
    }
}

pub(super) fn openai_usage_to_anthropic_usage_stream(usage: &Value) -> Value {
    serde_json::json!({
        "input_tokens": usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
        "output_tokens": usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0)
    })
}

pub(super) fn copy_unknown_usage_fields(
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

pub(super) fn clone_usage_details_object_stream(details: Option<&Value>) -> Option<Value> {
    let details = details?.as_object()?;
    (!details.is_empty()).then(|| Value::Object(details.clone()))
}

pub(super) fn gemini_usage_field_u64(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub(super) fn gemini_merge_usage_metadata_into_state(state: &mut StreamState, usage_meta: &Value) {
    let existing = state.usage.clone().unwrap_or_else(|| serde_json::json!({}));
    let existing_prompt = gemini_usage_field_u64(&existing, "prompt_tokens");
    let existing_completion = gemini_usage_field_u64(&existing, "completion_tokens");
    let existing_total = gemini_usage_field_u64(&existing, "total_tokens");
    let existing_reasoning = existing
        .get("completion_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let existing_cached = existing
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let existing_candidates = existing_completion.saturating_sub(existing_reasoning);

    let prompt_tokens = usage_meta
        .get("promptTokenCount")
        .and_then(Value::as_u64)
        .map(|value| value.max(existing_prompt))
        .unwrap_or(existing_prompt);
    let candidates_tokens = usage_meta
        .get("candidatesTokenCount")
        .and_then(Value::as_u64)
        .map(|value| value.max(existing_candidates))
        .unwrap_or(existing_candidates);
    let thoughts_tokens = usage_meta
        .get("thoughtsTokenCount")
        .and_then(Value::as_u64)
        .map(|value| value.max(existing_reasoning))
        .unwrap_or(existing_reasoning);
    let completion_tokens = candidates_tokens + thoughts_tokens;
    let total_tokens = usage_meta
        .get("totalTokenCount")
        .and_then(Value::as_u64)
        .map(|value| value.max(existing_total))
        .unwrap_or_else(|| existing_total.max(prompt_tokens + completion_tokens));
    let cached_tokens = usage_meta
        .get("cachedContentTokenCount")
        .and_then(Value::as_u64)
        .map(|value| value.max(existing_cached))
        .unwrap_or(existing_cached);

    let mut merged = existing;
    merged["prompt_tokens"] = serde_json::json!(prompt_tokens);
    merged["completion_tokens"] = serde_json::json!(completion_tokens);
    merged["total_tokens"] = serde_json::json!(total_tokens);

    if cached_tokens > 0 {
        if !merged["prompt_tokens_details"].is_object() {
            merged["prompt_tokens_details"] = serde_json::json!({});
        }
        merged["prompt_tokens_details"]["cached_tokens"] = serde_json::json!(cached_tokens);
    }
    if thoughts_tokens > 0 {
        if !merged["completion_tokens_details"].is_object() {
            merged["completion_tokens_details"] = serde_json::json!({});
        }
        merged["completion_tokens_details"]["reasoning_tokens"] =
            serde_json::json!(thoughts_tokens);
    }

    state.usage = Some(merged);
}

pub(super) fn gemini_candidate_less_partial_is_bufferable(response: &Value) -> bool {
    response
        .as_object()
        .map(|obj| {
            obj.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "responseId"
                        | "response_id"
                        | "modelVersion"
                        | "model_version"
                        | "usageMetadata"
                        | "usage_metadata"
                        | "createTime"
                        | "create_time"
                )
            })
        })
        .unwrap_or(false)
}

pub(super) fn gemini_nonportable_tool_call_message(
    tool_type: &str,
    proxied_tool_kind: Option<&str>,
) -> String {
    if tool_type == "custom" {
        return custom_tools_not_portable_message(UpstreamFormat::Google);
    }
    if let Some(kind) = proxied_tool_kind {
        return format!(
            "OpenAI proxied tool kind `{kind}` cannot be faithfully translated to Gemini."
        );
    }
    "OpenAI tool call cannot be faithfully translated to Gemini.".to_string()
}

pub(super) fn openai_chunk(
    state: &StreamState,
    delta: Value,
    finish_reason: Option<&str>,
) -> Value {
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

pub(super) fn emit_openai_assistant_role_if_needed(state: &mut StreamState, out: &mut Vec<Value>) {
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

pub(super) fn mark_stream_fatal_rejection(
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

pub(super) fn reject_openai_stream(
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

pub(super) const GEMINI_DUMMY_THOUGHT_SIGNATURE_STREAM: &str = "skip_thought_signature_validator";

pub(super) fn responses_usage_to_openai_usage_stream(usage: &Value) -> Value {
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

    if let Some(details) = clone_usage_details_object_stream(
        usage
            .get("input_tokens_details")
            .or(usage.get("prompt_tokens_details")),
    ) {
        mapped["prompt_tokens_details"] = details;
    }

    if let Some(details) = clone_usage_details_object_stream(
        usage
            .get("output_tokens_details")
            .or(usage.get("completion_tokens_details")),
    ) {
        mapped["completion_tokens_details"] = details;
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

pub(super) fn openai_usage_to_responses_usage_stream(usage: &Value) -> Value {
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

    if let Some(details) = clone_usage_details_object_stream(usage.get("prompt_tokens_details")) {
        mapped["input_tokens_details"] = details;
    }
    if let Some(details) = clone_usage_details_object_stream(usage.get("completion_tokens_details"))
    {
        mapped["output_tokens_details"] = details;
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

pub(super) fn responses_event_tool_call_index(event: &Value, state: &StreamState) -> Option<usize> {
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
