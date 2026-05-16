use super::state::*;
use super::wire::*;
use super::*;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

pub(super) fn stop_thinking_block_claude(state: &mut StreamState, out: &mut Vec<Vec<u8>>) {
    if !state.thinking_block_started {
        return;
    }
    out.push(format_sse_event(
        "content_block_stop",
        &serde_json::json!({ "type": "content_block_stop", "index": state.thinking_block_index }),
    ));
    state.thinking_block_started = false;
}

pub(super) fn ensure_thinking_block_claude(state: &mut StreamState, out: &mut Vec<Vec<u8>>) {
    if state.thinking_block_started {
        return;
    }
    state.thinking_block_index = state.next_block_index;
    state.next_block_index += 1;
    state.thinking_block_started = true;
    out.push(format_sse_event(
        "content_block_start",
        &serde_json::json!({
            "type": "content_block_start",
            "index": state.thinking_block_index,
            "content_block": { "type": "thinking", "thinking": "" }
        }),
    ));
}

pub(super) fn stop_text_block_claude(state: &mut StreamState, out: &mut Vec<Vec<u8>>) {
    if !state.text_block_started || state.text_block_closed {
        return;
    }
    state.text_block_closed = true;
    out.push(format_sse_event(
        "content_block_stop",
        &serde_json::json!({ "type": "content_block_stop", "index": state.text_block_index }),
    ));
}

pub(super) fn minimax_reasoning_details_text(value: Option<&Value>) -> Option<String> {
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

pub(super) fn is_minimax_chunk(chunk: &Value, state: &StreamState) -> bool {
    chunk
        .get("model")
        .and_then(Value::as_str)
        .or(state.model.as_deref())
        .map(|model| model.starts_with("MiniMax-"))
        .unwrap_or(false)
}

pub(super) fn normalize_openai_stream_text(incoming: &str, seen: &mut String) -> Option<String> {
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

pub(super) fn openai_chunk_reasoning_delta(
    delta: &Value,
    state: &mut StreamState,
) -> Option<String> {
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

pub(super) fn openai_chunk_content_delta(delta: &Value, state: &mut StreamState) -> Option<String> {
    let content = delta.get("content").and_then(Value::as_str)?;
    normalize_openai_stream_text(content, &mut state.openai_seen_content)
}

pub(super) fn openai_chunk_refusal_delta(delta: &Value, state: &mut StreamState) -> Option<String> {
    let refusal = delta.get("refusal").and_then(Value::as_str)?;
    normalize_openai_stream_text(refusal, &mut state.openai_seen_refusal)
}

pub(super) fn openai_chunk_annotations_delta(delta: &Value) -> Vec<Value> {
    delta
        .get("annotations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub(super) const INTERNAL_NON_REPLAYABLE_TOOL_CALL_FIELD: &str = "_llmup_non_replayable_tool_call";
const INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON: &str = "incomplete_arguments";
const INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION: u64 = 1;
const INTERNAL_NON_REPLAYABLE_TOOL_CALL_SIGNATURE_FIELD: &str = "sig";
const INTERNAL_REPLAY_MARKER_KEY_ENV: &str = "LLMUP_INTERNAL_REPLAY_MARKER_KEY";

fn internal_replay_marker_key_stream() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Some(existing) = std::env::var(INTERNAL_REPLAY_MARKER_KEY_ENV)
            .ok()
            .filter(|value| !value.is_empty())
        {
            return existing;
        }
        let generated = uuid::Uuid::new_v4().to_string();
        std::env::set_var(INTERNAL_REPLAY_MARKER_KEY_ENV, &generated);
        generated
    })
}

fn raw_json_is_valid_object_stream(raw: &str) -> bool {
    !raw.trim().is_empty()
        && serde_json::from_str::<Value>(raw).is_ok_and(|value| value.is_object())
}

pub(super) fn non_replayable_tool_call_signature_stream(name: &str, raw: &str) -> String {
    let payload = serde_json::json!({
        "v": INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION,
        "reason": INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON,
        "name": name,
        "raw": raw
    });
    let encoded = serde_json::to_vec(&payload).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(internal_replay_marker_key_stream().as_bytes());
    hasher.update([0]);
    hasher.update(encoded);
    hex::encode(hasher.finalize())
}

pub(super) fn signed_non_replayable_tool_call_marker_stream(name: &str, raw: &str) -> Value {
    serde_json::json!({
        "reason": INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON,
        "v": INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION,
        INTERNAL_NON_REPLAYABLE_TOOL_CALL_SIGNATURE_FIELD: non_replayable_tool_call_signature_stream(name, raw)
    })
}

fn trusted_non_replayable_tool_call_marker_for_name_and_raw_stream(
    name: &str,
    raw: &str,
    marker: &Value,
) -> Option<Value> {
    let marker = marker.as_object()?;
    if marker.get("reason").and_then(Value::as_str)
        != Some(INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON)
    {
        return None;
    }
    if marker.get("v").and_then(Value::as_u64) != Some(INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION) {
        return None;
    }
    let signature = marker
        .get(INTERNAL_NON_REPLAYABLE_TOOL_CALL_SIGNATURE_FIELD)
        .and_then(Value::as_str)?;
    (signature == non_replayable_tool_call_signature_stream(name, raw))
        .then(|| Value::Object(marker.clone()))
}

fn tool_call_partial_replay_text_stream(name: &str, raw: &str) -> String {
    match raw.trim() {
        "" => format!("Tool call `{name}` had incomplete arguments."),
        trimmed => format!("Tool call `{name}` with partial arguments: {trimmed}"),
    }
}

fn responses_stream_tool_call_should_be_marked_non_replayable(
    tool_type: &str,
    arguments: &str,
    incomplete_reason: Option<&str>,
    terminated_without_finish_reason: bool,
) -> bool {
    if matches!(
        incomplete_reason,
        Some("max_output_tokens") | Some("pause_turn")
    ) {
        if arguments.trim().is_empty() {
            return false;
        }
        if tool_type == "custom" {
            return true;
        }
        return !raw_json_is_valid_object_stream(arguments);
    }

    terminated_without_finish_reason
        && tool_type != "custom"
        && !raw_json_is_valid_object_stream(arguments)
}

fn maybe_mark_responses_stream_tool_call_item_non_replayable(
    item: &mut Value,
    name: &str,
    tool_type: &str,
    arguments: &str,
    incomplete_reason: Option<&str>,
    terminated_without_finish_reason: bool,
) {
    if responses_stream_tool_call_should_be_marked_non_replayable(
        tool_type,
        arguments,
        incomplete_reason,
        terminated_without_finish_reason,
    ) {
        let Some(object) = item.as_object_mut() else {
            return;
        };
        object.insert(
            INTERNAL_NON_REPLAYABLE_TOOL_CALL_FIELD.to_string(),
            signed_non_replayable_tool_call_marker_stream(name, arguments),
        );
    }
}

pub(super) fn responses_message_part_delta_event_type(
    kind: ResponsesMessagePartKind,
) -> &'static str {
    match kind {
        ResponsesMessagePartKind::OutputText => "response.output_text.delta",
        ResponsesMessagePartKind::Refusal => "response.refusal.delta",
    }
}

pub(super) fn responses_message_part_done_event_type(
    kind: ResponsesMessagePartKind,
) -> &'static str {
    match kind {
        ResponsesMessagePartKind::OutputText => "response.output_text.done",
        ResponsesMessagePartKind::Refusal => "response.refusal.done",
    }
}

pub(super) fn responses_message_part_value(part: &ResponsesMessagePartState) -> Value {
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

pub(super) fn next_responses_seq(state: &mut StreamState) -> u64 {
    state.responses_seq += 1;
    state.responses_seq
}

pub(super) fn reserve_responses_output_index(state: &mut StreamState) -> u64 {
    let idx = state.responses_next_output_index;
    state.responses_next_output_index += 1;
    idx
}

pub(super) fn responses_reasoning_output_index(state: &mut StreamState) -> u64 {
    if let Some(idx) = state.responses_reasoning_output_index {
        idx
    } else {
        let idx = reserve_responses_output_index(state);
        state.responses_reasoning_output_index = Some(idx);
        idx
    }
}

pub(super) fn responses_message_output_index(state: &mut StreamState) -> u64 {
    if let Some(idx) = state.responses_message_output_index {
        idx
    } else {
        let idx = reserve_responses_output_index(state);
        state.responses_message_output_index = Some(idx);
        idx
    }
}

pub(super) fn responses_tool_output_index(state: &mut StreamState, tc_idx: usize) -> u64 {
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

pub(super) fn ensure_responses_message_item_added(
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

pub(super) fn ensure_responses_message_part(
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

fn responses_message_part_has_meaningful_content(part: &ResponsesMessagePartState) -> bool {
    !part.text.is_empty() || !part.annotations.is_empty()
}

fn responses_message_has_meaningful_content(state: &StreamState) -> bool {
    state
        .responses_message_parts
        .iter()
        .any(responses_message_part_has_meaningful_content)
}

fn reset_responses_message_segment(state: &mut StreamState) {
    state.output_item_id = None;
    state.output_item_added = false;
    state.responses_content_part_added = false;
    state.responses_output_text.clear();
    state.responses_message_parts.clear();
    state.responses_text_part_index = None;
    state.responses_refusal_part_index = None;
    state.responses_message_output_index = None;
}

fn responses_terminal_message_phase(finish_reason: Option<&str>) -> Option<&'static str> {
    match finish_reason {
        Some("stop") => Some("final_answer"),
        Some("tool_calls") => Some("commentary"),
        _ => None,
    }
}

fn emit_responses_message_done(
    state: &mut StreamState,
    response_id: &str,
    phase: Option<&str>,
    out: &mut Vec<Vec<u8>>,
) -> Option<ResponsesCompletedMessageState> {
    if !state.output_item_added || !responses_message_has_meaningful_content(state) {
        return None;
    }

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

    let mut item = serde_json::json!({
        "id": item_id,
        "type": "message",
        "status": "completed",
        "role": "assistant",
        "content": content
    });
    if let Some(phase) = phase {
        item["phase"] = Value::String(phase.to_string());
    }
    let output_item_done_ev = serde_json::json!({
        "type": "response.output_item.done",
        "sequence_number": next_responses_seq(state),
        "response_id": response_id,
        "output_index": output_index,
        "item": item
    });
    out.push(format_sse_event(
        "response.output_item.done",
        &output_item_done_ev,
    ));

    let item = output_item_done_ev["item"].clone();
    reset_responses_message_segment(state);
    Some(ResponsesCompletedMessageState { output_index, item })
}

pub(super) fn emit_openai_responses_terminal(
    state: &mut StreamState,
    response_id: &str,
    created: u64,
    _idx: u64,
) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let finish_reason = state.finish_reason.clone();
    let finish_reason = finish_reason.as_deref();
    let terminated_without_finish_reason = finish_reason.is_none();

    let incomplete_reason = match finish_reason {
        Some("length") => Some("max_output_tokens"),
        Some("content_filter") => Some("content_filter"),
        Some("pause_turn") => Some("pause_turn"),
        _ => None,
    };
    let failed_error = match finish_reason {
        Some("context_length_exceeded") => Some(serde_json::json!({
            "type": "invalid_request_error",
            "code": "context_length_exceeded",
            "message": "Your input exceeds the context window of this model. Please adjust your input and try again."
        })),
        Some("error") => state.openai_terminal_error.clone().or_else(|| {
            Some(serde_json::json!({
                "type": "server_error",
                "code": "error",
                "message": "The provider returned an error."
            }))
        }),
        Some("tool_error") => Some(serde_json::json!({
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

    let degraded_tool_call_texts = emit_pending_responses_tool_call_done_events(
        state,
        response_id,
        incomplete_reason,
        terminated_without_finish_reason,
        true,
        &mut out,
    );

    if !degraded_tool_call_texts.is_empty() {
        let output_index = responses_message_output_index(state);
        ensure_responses_message_item_added(state, response_id, output_index, &mut out);
        let content_index = ensure_responses_message_part(
            state,
            response_id,
            output_index,
            ResponsesMessagePartKind::OutputText,
            &mut out,
        );
        if let Some(part) = state.responses_message_parts.get_mut(content_index) {
            if !part.text.is_empty() {
                part.text.push_str("\n\n");
            }
            part.text.push_str(&degraded_tool_call_texts.join("\n\n"));
        }
    }

    if let Some(message_item) = emit_responses_message_done(
        state,
        response_id,
        responses_terminal_message_phase(finish_reason),
        &mut out,
    ) {
        state.responses_completed_message_items.push(message_item);
    }

    let mut output_items: Vec<(u64, Value)> = Vec::new();
    if state.responses_reasoning_added {
        let output_index = state
            .responses_reasoning_output_index
            .unwrap_or_else(|| responses_reasoning_output_index(state));
        output_items.push((
            output_index,
            serde_json::json!({
                "id": state.responses_reasoning_id,
                "type": "reasoning",
                "summary": [{ "type": "summary_text", "text": state.responses_reasoning_text }]
            }),
        ));
    }
    output_items.extend(
        state
            .responses_completed_message_items
            .iter()
            .map(|message| (message.output_index, message.item.clone())),
    );
    let mut tool_call_output = state.openai_tool_calls.values().collect::<Vec<_>>();
    tool_call_output.sort_by_key(|tc| tc.index);
    for tool_call in tool_call_output {
        if let Some(marker) = tool_call.non_replayable_marker.as_ref() {
            if trusted_non_replayable_tool_call_marker_for_name_and_raw_stream(
                &tool_call.name,
                &tool_call.arguments,
                marker,
            )
            .is_some()
            {
                continue;
            }
        }
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
            maybe_mark_responses_stream_tool_call_item_non_replayable(
                &mut item,
                &tool_call.name,
                tool_call_state_type(tool_call),
                &tool_call.arguments,
                incomplete_reason,
                terminated_without_finish_reason,
            );
            output_items.push((
                tool_call.block_index.unwrap_or(tool_call.index) as u64,
                item,
            ));
        }
    }
    output_items.sort_by_key(|(output_index, _)| *output_index);
    let output = output_items
        .into_iter()
        .map(|(_, item)| item)
        .collect::<Vec<_>>();

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

pub(super) fn anthropic_error_event(error_type: &str, message: &str) -> Vec<u8> {
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

fn reject_openai_responses_stream(
    state: &mut StreamState,
    response_id: &str,
    created: u64,
    code: &str,
    message: impl Into<String>,
) -> Vec<Vec<u8>> {
    let message = mark_stream_fatal_rejection(state, message);
    state.openai_tool_calls.clear();
    state.finish_reason = Some("error".to_string());
    state.responses_terminal_sent = true;
    let failed = serde_json::json!({
        "type": "response.failed",
        "sequence_number": next_responses_seq(state),
        "response": {
            "id": response_id,
            "object": "response",
            "created_at": created,
            "status": "failed",
            "background": false,
            "error": {
                "type": "invalid_request_error",
                "code": code,
                "message": message
            },
            "incomplete_details": null,
            "usage": null,
            "output": []
        }
    });
    vec![format_sse_event("response.failed", &failed)]
}

pub(super) fn reject_anthropic_stream(
    state: &mut StreamState,
    error_type: &str,
    message: impl Into<String>,
) -> Vec<Vec<u8>> {
    let message = mark_stream_fatal_rejection(state, message);
    vec![anthropic_error_event(error_type, &message)]
}

pub(super) fn openai_chunk_to_claude_sse(chunk: &Value, state: &mut StreamState) -> Vec<Vec<u8>> {
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

    if let Some(message) =
        delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .and_then(|tool_calls| {
                tool_calls.iter().find_map(|tool_call| {
                    anthropic_tool_use_type_for_openai_tool_call(tool_call).err()
                })
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

    if let Some(reasoning) = reasoning_delta {
        if !reasoning.is_empty() {
            stop_text_block_claude(state, &mut out);
            ensure_thinking_block_claude(state, &mut out);
            let ev = serde_json::json!({
                "type": "content_block_delta",
                "index": state.thinking_block_index,
                "delta": { "type": "thinking_delta", "thinking": reasoning }
            });
            out.push(format_sse_event("content_block_delta", &ev));
        }
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

struct ResponsesToolCallEvent<'a> {
    response_id: &'a str,
    output_index: u64,
    call_id: &'a str,
    name: &'a str,
    tool_type: &'a str,
    proxied_tool_kind: Option<&'a str>,
}

fn emit_responses_tool_call_item_added(
    state: &mut StreamState,
    event: &ResponsesToolCallEvent<'_>,
    out: &mut Vec<Vec<u8>>,
) {
    let payload_field = responses_tool_call_payload_field(event.tool_type);
    let mut ev = serde_json::json!({
        "type": "response.output_item.added",
        "sequence_number": next_responses_seq(state),
        "response_id": event.response_id,
        "output_index": event.output_index,
        "item": {
            "id": format!("fc_{}", event.call_id),
            "type": responses_tool_call_item_type(event.tool_type),
            "call_id": event.call_id,
            "name": event.name,
        }
    });
    ev["item"][payload_field] = Value::String(String::new());
    if let Some(proxied_tool_kind) = event.proxied_tool_kind {
        ev["item"]["proxied_tool_kind"] = Value::String(proxied_tool_kind.to_string());
    }
    out.push(format_sse_event("response.output_item.added", &ev));
}

fn emit_responses_tool_call_delta(
    state: &mut StreamState,
    event: &ResponsesToolCallEvent<'_>,
    delta: &str,
    out: &mut Vec<Vec<u8>>,
) {
    if delta.is_empty() {
        return;
    }
    let mut ev = serde_json::json!({
        "type": responses_tool_call_delta_event_type(event.tool_type),
        "sequence_number": next_responses_seq(state),
        "response_id": event.response_id,
        "call_id": event.call_id,
        "name": event.name,
        "item_id": format!("fc_{}", event.call_id),
        "output_index": event.output_index,
        "delta": delta
    });
    if let Some(proxied_tool_kind) = event.proxied_tool_kind {
        ev["proxied_tool_kind"] = Value::String(proxied_tool_kind.to_string());
    }
    out.push(format_sse_event(
        responses_tool_call_delta_event_type(event.tool_type),
        &ev,
    ));
}

struct ResponsesToolCallDone {
    call_id: String,
    name: String,
    arguments: String,
    output_index: usize,
    tool_type: String,
    proxied_tool_kind: Option<String>,
}

struct ResponsesToolCallDoneContext<'a> {
    response_id: &'a str,
    incomplete_reason: Option<&'a str>,
    terminated_without_finish_reason: bool,
}

fn emit_responses_tool_call_done(
    state: &mut StreamState,
    tool_call: ResponsesToolCallDone,
    context: ResponsesToolCallDoneContext<'_>,
    out: &mut Vec<Vec<u8>>,
) {
    let ResponsesToolCallDone {
        call_id,
        name,
        arguments,
        output_index,
        tool_type,
        proxied_tool_kind,
    } = tool_call;
    let ResponsesToolCallDoneContext {
        response_id,
        incomplete_reason,
        terminated_without_finish_reason,
    } = context;

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
    maybe_mark_responses_stream_tool_call_item_non_replayable(
        &mut output_item_done_ev["item"],
        &name,
        &tool_type,
        &arguments,
        incomplete_reason,
        terminated_without_finish_reason,
    );
    out.push(format_sse_event(
        "response.output_item.done",
        &output_item_done_ev,
    ));
}

fn emit_pending_responses_tool_call_done_events(
    state: &mut StreamState,
    response_id: &str,
    incomplete_reason: Option<&str>,
    terminated_without_finish_reason: bool,
    include_non_replayable_degraded_texts: bool,
    out: &mut Vec<Vec<u8>>,
) -> Vec<String> {
    let mut tool_call_keys = state.openai_tool_calls.keys().cloned().collect::<Vec<_>>();
    tool_call_keys.sort_unstable();
    let mut completed_tool_calls = Vec::new();
    let mut degraded_tool_call_texts = Vec::new();
    for key in tool_call_keys {
        let Some(tool_call) = state.openai_tool_calls.get_mut(&key) else {
            continue;
        };
        if let Some(marker) = tool_call.non_replayable_marker.as_ref() {
            if trusted_non_replayable_tool_call_marker_for_name_and_raw_stream(
                &tool_call.name,
                &tool_call.arguments,
                marker,
            )
            .is_some()
            {
                if include_non_replayable_degraded_texts {
                    tool_call.responses_done = true;
                    degraded_tool_call_texts.push(tool_call_partial_replay_text_stream(
                        &tool_call.name,
                        &tool_call.arguments,
                    ));
                }
                continue;
            }
        }
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
        emit_responses_tool_call_done(
            state,
            ResponsesToolCallDone {
                call_id,
                name,
                arguments,
                output_index,
                tool_type,
                proxied_tool_kind,
            },
            ResponsesToolCallDoneContext {
                response_id,
                incomplete_reason,
                terminated_without_finish_reason,
            },
            out,
        );
    }
    degraded_tool_call_texts
}

fn flush_pending_responses_tool_call(
    state: &mut StreamState,
    response_id: &str,
    tc_idx: usize,
    finalize: bool,
    out: &mut Vec<Vec<u8>>,
) {
    let output_index = responses_tool_output_index(state, tc_idx);
    let resolved = {
        let Some(entry) = state.openai_tool_calls.get_mut(&tc_idx) else {
            return;
        };
        if entry.responses_item_added || entry.id.is_none() {
            return;
        }
        if entry
            .non_replayable_marker
            .as_ref()
            .and_then(|marker| {
                trusted_non_replayable_tool_call_marker_for_name_and_raw_stream(
                    &entry.name,
                    &entry.arguments,
                    marker,
                )
            })
            .is_some()
        {
            return;
        }

        let call_id = entry
            .id
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut tool_type = tool_call_state_type(entry).to_string();
        let name = entry.name.clone();
        let mut arguments = entry.arguments.clone();
        let proxied_tool_kind = entry.proxied_tool_kind.clone();

        if tool_type != "custom"
            && proxied_tool_kind.is_none()
            && request_scoped_openai_custom_bridge_expects_canonical_input_wrapper_stream(
                state.request_scoped_tool_bridge_context.as_ref(),
                &entry.name,
            )
        {
            if let Some(decoded_input) =
                openai_custom_bridge_decode_arguments_stream(&entry.arguments)
            {
                tool_type = "custom".to_string();
                arguments = decoded_input;
                entry.tool_type = Some(tool_type.clone());
                entry.arguments = arguments.clone();
            } else if !finalize {
                return;
            }
        }

        entry.responses_item_added = true;
        entry.responses_item_id = Some(format!("fc_{call_id}"));
        (call_id, name, arguments, tool_type, proxied_tool_kind)
    };

    let (call_id, name, arguments, tool_type, proxied_tool_kind) = resolved;
    let event = ResponsesToolCallEvent {
        response_id,
        output_index,
        call_id: &call_id,
        name: &name,
        tool_type: &tool_type,
        proxied_tool_kind: proxied_tool_kind.as_deref(),
    };
    emit_responses_tool_call_item_added(state, &event, out);
    emit_responses_tool_call_delta(state, &event, &arguments, out);
}

pub(super) fn flush_pending_responses_tool_calls(
    state: &mut StreamState,
    response_id: &str,
    finalize: bool,
    out: &mut Vec<Vec<u8>>,
) {
    let mut tool_call_keys = state.openai_tool_calls.keys().cloned().collect::<Vec<_>>();
    tool_call_keys.sort_unstable();
    for key in tool_call_keys {
        flush_pending_responses_tool_call(state, response_id, key, finalize, out);
    }
}

pub(super) fn openai_chunk_to_responses_sse(
    chunk: &Value,
    state: &mut StreamState,
) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if state.responses_terminal_sent {
        return out;
    }
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
        state.responses_completed_message_items.clear();
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
            let event_type =
                responses_message_part_delta_event_type(ResponsesMessagePartKind::Refusal);
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
        if let Some(message_item) =
            emit_responses_message_done(state, &response_id, Some("commentary"), &mut out)
        {
            state.responses_completed_message_items.push(message_item);
        }
        for tc in tcs {
            let tc_idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            dedupe_tool_call_state_by_call_id(&mut state.openai_tool_calls, tc_idx, tc.get("id"));
            let output_index = responses_tool_output_index(state, tc_idx);
            if let Some(name) = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
            {
                if let Err(message) = validate_public_tool_name_not_reserved(name) {
                    out.extend(reject_openai_responses_stream(
                        state,
                        &response_id,
                        chunk.get("created").and_then(Value::as_u64).unwrap_or(0),
                        "reserved_openai_custom_bridge_prefix",
                        message,
                    ));
                    return out;
                }
            }
            let mut args_delta: Option<(String, String, String, String, Option<String>)> = None;
            let should_flush_pending;
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
                if let Some(marker) = tc.get(INTERNAL_NON_REPLAYABLE_TOOL_CALL_FIELD).cloned() {
                    entry.non_replayable_marker = Some(marker);
                }
                let had_item_added = entry.responses_item_added;
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
                if let Some(args) = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                {
                    if !args.is_empty() {
                        entry.arguments.push_str(args);
                        if had_item_added {
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
                let trusted_non_replayable = entry
                    .non_replayable_marker
                    .as_ref()
                    .and_then(|marker| {
                        trusted_non_replayable_tool_call_marker_for_name_and_raw_stream(
                            &entry.name,
                            &entry.arguments,
                            marker,
                        )
                    })
                    .is_some();
                if trusted_non_replayable {
                    args_delta = None;
                }
                should_flush_pending =
                    !trusted_non_replayable && !entry.responses_item_added && entry.id.is_some();
            }
            if should_flush_pending {
                flush_pending_responses_tool_call(state, &response_id, tc_idx, false, &mut out);
            }
            if let Some((call_id, name, args, tool_type, proxied_tool_kind)) = args_delta {
                let event = ResponsesToolCallEvent {
                    response_id: &response_id,
                    output_index,
                    call_id: &call_id,
                    name: &name,
                    tool_type: &tool_type,
                    proxied_tool_kind: proxied_tool_kind.as_deref(),
                };
                emit_responses_tool_call_delta(state, &event, &args, &mut out);
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
        flush_pending_responses_tool_calls(state, &response_id, true, &mut out);
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
    if matches!(finish_reason, Some("tool_calls")) {
        flush_pending_responses_tool_calls(state, &response_id, true, &mut out);
        let _ = emit_pending_responses_tool_call_done_events(
            state,
            &response_id,
            None,
            false,
            false,
            &mut out,
        );
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
        flush_pending_responses_tool_calls(state, &response_id, true, &mut out);
        out.extend(emit_openai_responses_terminal(
            state,
            &response_id,
            created,
            idx,
        ));
    }
    out
}
