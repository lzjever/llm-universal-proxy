use super::state::*;
use super::wire::*;
use super::*;

pub(super) fn openai_finish_reason_to_gemini_stream(finish_reason: &str) -> &'static str {
    match finish_reason {
        "stop" | "tool_calls" => "STOP",
        "length" => "MAX_TOKENS",
        "content_filter" => "SAFETY",
        "pause_turn" | "context_length_exceeded" | "tool_error" | "error" => "OTHER",
        _ => "STOP",
    }
}

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

pub(super) fn emit_openai_responses_terminal(
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
                        responses_message_part_done_event_type(
                            ResponsesMessagePartKind::OutputText,
                        ),
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

pub(super) fn openai_chunk_to_gemini_sse(chunk: &Value, state: &mut StreamState) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if state.fatal_rejection.is_some() && chunk.get("error").is_none() {
        return out;
    }
    if let Some(usage) = chunk.get("usage") {
        state.usage = Some(usage.clone());
    }
    if let Some(error) = chunk.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Translated stream cannot be represented as Gemini SSE.");
        mark_stream_fatal_rejection(state, message);
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

    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tool_calls {
            let tool_type = openai_stream_tool_call_type(tc);
            let proxied_tool_kind = tc.get("proxied_tool_kind").and_then(Value::as_str);
            if tool_type != "function" || proxied_tool_kind.is_some() {
                let message = gemini_nonportable_tool_call_message(tool_type, proxied_tool_kind);
                mark_stream_fatal_rejection(state, message);
                return out;
            }
        }
    }

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
            if tc.get("type").is_some() {
                entry.tool_type = Some(openai_stream_tool_call_type(tc).to_string());
            }
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
            if let Some(proxied_tool_kind) = tc.get("proxied_tool_kind").and_then(Value::as_str) {
                entry.proxied_tool_kind = Some(proxied_tool_kind.to_string());
            }
            if tool_call_state_type(entry) != "function" || entry.proxied_tool_kind.is_some() {
                let message = gemini_nonportable_tool_call_message(
                    tool_call_state_type(entry),
                    entry.proxied_tool_kind.as_deref(),
                );
                mark_stream_fatal_rejection(state, message);
                return out;
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

pub(super) fn openai_chunk_to_responses_sse(
    chunk: &Value,
    state: &mut StreamState,
) -> Vec<Vec<u8>> {
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
