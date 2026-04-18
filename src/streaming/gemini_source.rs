use super::state::*;
use super::*;

pub(super) fn gemini_finish_reason_to_openai_stream(
    finish_reason: &str,
    has_tool_calls: bool,
) -> String {
    gemini_finish_reason_to_openai(Some(finish_reason), has_tool_calls)
}

pub(super) fn unsupported_gemini_output_part_kind(part: &Value) -> Option<String> {
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
        gemini_merge_usage_metadata_into_state(state, usage_meta);
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
            if gemini_candidate_less_partial_is_bufferable(response) {
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
