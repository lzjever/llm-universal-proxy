use super::state::*;
use super::*;

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
                    if let Some(t) = delta.and_then(|d| d.get("thinking").and_then(Value::as_str)) {
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
                        let chunk_json = if let Some(tc) =
                            idx.and_then(|i| state.claude_tool_uses.get_mut(&i))
                        {
                            let delta_to_emit =
                                if tc.arguments_seeded_from_start && !tc.arguments.is_empty() {
                                    tc.arguments = merge_seeded_tool_arguments(&tc.arguments, pj);
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

pub(super) fn convert_claude_stop_reason(r: &str) -> String {
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
