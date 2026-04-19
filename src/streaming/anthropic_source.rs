use super::state::*;
use super::*;

fn raw_json_is_valid_object_anthropic_source(raw: &str) -> bool {
    !raw.trim().is_empty()
        && serde_json::from_str::<Value>(raw).is_ok_and(|value| value.is_object())
}

fn anthropic_tool_use_partial_replay_text(name: &str, raw: &str) -> String {
    match raw.trim() {
        "" => format!("Tool call `{name}` had incomplete arguments."),
        trimmed => format!("Tool call `{name}` with partial arguments: {trimmed}"),
    }
}

fn finalize_claude_tool_use_block(
    state: &mut StreamState,
    block_index: usize,
    out: &mut Vec<Value>,
) {
    enum FinalizedToolUse {
        Structured {
            openai_index: usize,
            id: Option<Value>,
            name: String,
            arguments: String,
            proxied_tool_kind: Option<String>,
        },
        Text {
            message: String,
        },
    }

    let finalized = {
        let Some(tool_use) = state.claude_tool_uses.get(&block_index) else {
            return;
        };
        if tool_use.finalized {
            return;
        }
        if tool_use.zero_arg_candidate
            && !tool_use.saw_input_json_delta
            && tool_use.arguments.trim().is_empty()
        {
            Some(FinalizedToolUse::Structured {
                openai_index: tool_use.openai_index,
                id: tool_use.id.clone(),
                name: tool_use.name.clone(),
                arguments: "{}".to_string(),
                proxied_tool_kind: tool_use.proxied_tool_kind.clone(),
            })
        } else if raw_json_is_valid_object_anthropic_source(&tool_use.arguments) {
            Some(FinalizedToolUse::Structured {
                openai_index: tool_use.openai_index,
                id: tool_use.id.clone(),
                name: tool_use.name.clone(),
                arguments: tool_use.arguments.clone(),
                proxied_tool_kind: tool_use.proxied_tool_kind.clone(),
            })
        } else {
            Some(FinalizedToolUse::Text {
                message: anthropic_tool_use_partial_replay_text(
                    &tool_use.name,
                    &tool_use.arguments,
                ),
            })
        }
    };

    match finalized {
        Some(FinalizedToolUse::Structured {
            openai_index,
            id,
            name,
            arguments,
            proxied_tool_kind,
        }) => {
            emit_openai_assistant_role_if_needed(state, out);
            let mut tool_call = serde_json::json!({
                "index": openai_index,
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": arguments
                }
            });
            if let Some(proxied_tool_kind) = proxied_tool_kind {
                tool_call["proxied_tool_kind"] = Value::String(proxied_tool_kind);
            }
            out.push(openai_chunk(
                state,
                serde_json::json!({ "tool_calls": [tool_call] }),
                None,
            ));
            if let Some(tool_use) = state.claude_tool_uses.get_mut(&block_index) {
                tool_use.arguments = arguments;
                tool_use.zero_arg_candidate = false;
                tool_use.start_arguments_emitted = true;
                tool_use.arguments_seeded_from_start = false;
                tool_use.finalized = true;
            }
        }
        Some(FinalizedToolUse::Text { message }) => {
            emit_openai_assistant_role_if_needed(state, out);
            out.push(openai_chunk(
                state,
                serde_json::json!({ "content": message }),
                None,
            ));
            if let Some(tool_use) = state.claude_tool_uses.get_mut(&block_index) {
                tool_use.zero_arg_candidate = false;
                tool_use.finalized = true;
                tool_use.arguments_seeded_from_start = false;
            }
        }
        None => {}
    }
}

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
                    let seeded_thinking = block
                        .and_then(|b| b.get("thinking").and_then(Value::as_str))
                        .unwrap_or("")
                        .to_string();
                    let signature = block
                        .and_then(|b| b.get("signature").and_then(Value::as_str))
                        .map(str::to_string);
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
                            thinking: seeded_thinking,
                            signature,
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
                    let serialized_input = block
                        .get("input")
                        .filter(|input| !input.is_null())
                        .and_then(|input| serde_json::to_string(input).ok())
                        .filter(|serialized| serialized != "null")
                        .unwrap_or_default();
                    let zero_arg_candidate = serialized_input == "{}";
                    let seeded_arguments = if zero_arg_candidate {
                        String::new()
                    } else {
                        serialized_input
                    };
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
                            proxied_tool_kind: (block_ty == Some("server_tool_use"))
                                .then(|| "anthropic_server_tool_use".to_string()),
                            zero_arg_candidate,
                            saw_input_json_delta: false,
                            arguments_seeded_from_start: !seeded_arguments.is_empty(),
                            start_arguments_emitted: false,
                            finalized: false,
                        },
                    );
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
                    let Some(block_index) = idx else {
                        return reject_openai_stream(
                            state,
                            "invalid_request_error",
                            "unsupported_anthropic_stream_event",
                            "Anthropic thinking_delta is missing a block index.",
                        );
                    };
                    let Some(block_state) = state.claude_blocks.get_mut(&block_index) else {
                        return reject_openai_stream(
                            state,
                            "invalid_request_error",
                            "unsupported_anthropic_stream_event",
                            "Anthropic thinking_delta referenced an unknown block.",
                        );
                    };
                    if block_state.kind != Some(ClaudeBlockKind::Thinking) {
                        return reject_openai_stream(
                            state,
                            "invalid_request_error",
                            "unsupported_anthropic_stream_event",
                            "Anthropic thinking_delta is only valid for thinking blocks.",
                        );
                    };
                    if let Some(t) = delta.and_then(|d| d.get("thinking").and_then(Value::as_str)) {
                        if !t.is_empty() {
                            block_state.thinking.push_str(t);
                        }
                    }
                }
                Some("input_json_delta") => {
                    if let Some(tc) = idx.and_then(|i| state.claude_tool_uses.get_mut(&i)) {
                        tc.saw_input_json_delta = true;
                    }
                    if let Some(pj) =
                        delta.and_then(|d| d.get("partial_json").and_then(Value::as_str))
                    {
                        if let Some(tc) = idx.and_then(|i| state.claude_tool_uses.get_mut(&i)) {
                            if tc.zero_arg_candidate {
                                tc.zero_arg_candidate = false;
                            }
                            if tc.arguments_seeded_from_start && !tc.arguments.is_empty() {
                                tc.arguments = merge_seeded_tool_arguments(&tc.arguments, pj);
                                tc.arguments_seeded_from_start = false;
                            } else {
                                tc.arguments.push_str(pj);
                            }
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
            if let Some(i) = idx {
                finalize_claude_tool_use_block(state, i, &mut out);
            }
            if let Some(i) = idx {
                let buffered_reasoning = state.claude_blocks.get(&i).and_then(|block_state| {
                    if block_state.kind == Some(ClaudeBlockKind::Thinking) {
                        (!block_state.omitted && !block_state.thinking.is_empty())
                            .then(|| block_state.thinking.clone())
                    } else {
                        None
                    }
                });
                if let Some(reasoning) = buffered_reasoning {
                    emit_openai_assistant_role_if_needed(state, &mut out);
                    out.push(openai_chunk(
                        state,
                        serde_json::json!({ "reasoning_content": reasoning }),
                        None,
                    ));
                }
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
                let pending_tool_use_indices = state
                    .claude_tool_uses
                    .iter()
                    .filter_map(|(idx, tool_use)| (!tool_use.finalized).then_some(*idx))
                    .collect::<Vec<_>>();
                for idx in pending_tool_use_indices {
                    finalize_claude_tool_use_block(state, idx, &mut out);
                }
                let has_structured_tool_use = state
                    .claude_tool_uses
                    .values()
                    .any(|tool_use| tool_use.start_arguments_emitted);
                let fr = state.finish_reason.clone().map_or_else(
                    || {
                        if has_structured_tool_use {
                            "tool_calls".to_string()
                        } else {
                            "stop".to_string()
                        }
                    },
                    |finish_reason| {
                        if finish_reason == "tool_calls" && !has_structured_tool_use {
                            "stop".to_string()
                        } else {
                            finish_reason
                        }
                    },
                );
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
