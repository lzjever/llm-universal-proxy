use super::openai_sink::normalize_openai_stream_text;
use super::state::*;
use super::*;

pub(super) fn responses_failed_code_to_openai_finish_stream(code: Option<&str>) -> &'static str {
    responses_failed_code_to_openai_finish(code)
}

fn finalize_custom_tool_call_bridge_delta(
    tool_call: &mut ToolCallState,
    full_input: &str,
) -> Option<String> {
    let emitted_input = tool_call.custom_input_text.as_mut()?;
    if tool_call.custom_input_done {
        return None;
    }

    let mut bridge_delta = String::new();
    if let Some(remainder) = full_input.strip_prefix(emitted_input.as_str()) {
        if !remainder.is_empty() {
            bridge_delta.push_str(&openai_custom_bridge_input_delta_stream(remainder));
            emitted_input.push_str(remainder);
        }
    }
    bridge_delta.push_str(openai_custom_bridge_done_delta_stream());
    tool_call.arguments.push_str(&bridge_delta);
    tool_call.custom_input_done = true;
    Some(bridge_delta)
}

fn responses_terminal_custom_tool_bridge_chunk(
    event: &Value,
    state: &mut StreamState,
) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(output) = event
        .get("response")
        .and_then(|response| response.get("output"))
        .and_then(Value::as_array)
    else {
        return out;
    };

    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("custom_tool_call") {
            continue;
        }
        let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(full_input) = item.get("input").and_then(Value::as_str) else {
            continue;
        };
        let Some(idx) = state.openai_tool_calls.iter().find_map(|(idx, tool_call)| {
            (tool_call.id.as_ref().and_then(Value::as_str) == Some(call_id)).then_some(*idx)
        }) else {
            continue;
        };
        let payload = {
            let Some(tool_call) = state.openai_tool_calls.get_mut(&idx) else {
                continue;
            };
            let Some(bridge_delta) = finalize_custom_tool_call_bridge_delta(tool_call, full_input)
            else {
                continue;
            };
            serde_json::json!({
                "tool_calls": [{
                    "index": idx,
                    "id": call_id,
                    "type": "function",
                    "function": { "arguments": bridge_delta }
                }]
            })
        };
        out.push(openai_chunk(state, payload, None));
    }

    out
}

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

    if ty.starts_with("response.audio.") {
        return reject_openai_stream(
            state,
            "invalid_request_error",
            "unsupported_openai_responses_stream_event",
            "OpenAI Responses audio streaming events cannot be translated losslessly.",
        );
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
        if let Some(delta) =
            normalize_openai_stream_text(raw_refusal, &mut state.openai_seen_refusal)
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
            let raw_name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Err(message) = validate_public_tool_name_not_reserved(&raw_name) {
                return reject_openai_stream(
                    state,
                    "invalid_request_error",
                    "reserved_openai_custom_bridge_prefix",
                    message,
                );
            }
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
            let is_custom = item_ty == Some("custom_tool_call");
            let tool_type = "function";
            let custom_input = item.get("input").and_then(Value::as_str).unwrap_or("");
            let name = raw_name.clone();
            let arguments = if is_custom {
                openai_custom_bridge_start_delta_stream(custom_input)
            } else {
                item.get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
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
                    custom_input_text: is_custom.then_some(custom_input.to_string()),
                    custom_input_done: false,
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
                if ty == "response.custom_tool_call_input.delta" {
                    tc.tool_type.get_or_insert_with(|| "function".to_string());
                } else {
                    tc.tool_type.get_or_insert_with(|| tool_type.to_string());
                }
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
                let emitted_delta = if ty == "response.custom_tool_call_input.delta" {
                    tc.custom_input_text
                        .get_or_insert_with(String::new)
                        .push_str(delta);
                    openai_custom_bridge_input_delta_stream(delta)
                } else {
                    delta.to_string()
                };
                tc.arguments.push_str(&emitted_delta);
                tc.proxied_tool_kind.clone()
            };
            let mut tool_call_delta = serde_json::json!({
                "index": idx,
                "type": if ty == "response.custom_tool_call_input.delta" {
                    "function"
                } else {
                    tool_type
                },
                "function": {
                    "arguments": if ty == "response.custom_tool_call_input.delta" {
                        openai_custom_bridge_input_delta_stream(delta)
                    } else {
                        delta.to_string()
                    }
                }
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

    if ty == "response.custom_tool_call_input.done" {
        let idx = responses_event_tool_call_index(event, state)
            .unwrap_or_else(|| state.openai_tool_call_index.saturating_sub(1));
        let payload = {
            let Some(tool_call) = state.openai_tool_calls.get_mut(&idx) else {
                return out;
            };
            let full_input = event
                .get("input")
                .and_then(Value::as_str)
                .or(tool_call.custom_input_text.as_deref())
                .unwrap_or("")
                .to_string();
            let Some(bridge_delta) =
                finalize_custom_tool_call_bridge_delta(tool_call, full_input.as_str())
            else {
                return out;
            };
            serde_json::json!({
                "tool_calls": [{
                    "index": idx,
                    "id": tool_call.id.clone(),
                    "type": "function",
                    "function": { "arguments": bridge_delta }
                }]
            })
        };
        out.push(openai_chunk(state, payload, None));
        return out;
    }

    if ty == "response.completed" || ty == "response.incomplete" || ty == "response.failed" {
        out.extend(responses_terminal_custom_tool_bridge_chunk(event, state));
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
