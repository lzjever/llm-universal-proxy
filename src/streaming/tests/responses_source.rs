use super::*;

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
fn responses_custom_tool_call_events_bridge_into_function_tool_calls_and_done_suffix() {
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
    let done_chunks = responses_event_to_openai_chunks(
        &serde_json::json!({
            "type": "response.custom_tool_call_input.done",
            "output_index": 0,
            "item_id": "custom_item_1",
            "input": "print('hi')"
        }),
        &mut state,
    );

    assert_eq!(
        added_chunks[0]["choices"][0]["delta"]["tool_calls"][0]["type"],
        "function"
    );
    assert_eq!(
        added_chunks[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["name"],
        "code_exec"
    );
    assert_eq!(
        added_chunks[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
        "{\"input\":\"print('"
    );
    assert_eq!(
        delta_chunks[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
        "hi')"
    );
    assert_eq!(
        done_chunks[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
        "\"}"
    );
    let tool_call = state.openai_tool_calls.get(&0).expect("tool call state");
    assert_eq!(tool_call.name, "code_exec");
    assert_eq!(tool_call.arguments, "{\"input\":\"print('hi')\"}");
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
fn responses_stream_usage_preserves_audio_prediction_and_unknown_detail_fields() {
    let event = serde_json::json!({
        "type": "response.completed",
        "response": {
            "id": "resp_usage",
            "usage": {
                "input_tokens": 11,
                "output_tokens": 7,
                "total_tokens": 18,
                "service_tier": "priority",
                "input_tokens_details": {
                    "cached_tokens": 3,
                    "audio_tokens": 2,
                    "future_prompt_detail": 4
                },
                "output_tokens_details": {
                    "reasoning_tokens": 1,
                    "audio_tokens": 5,
                    "accepted_prediction_tokens": 6,
                    "rejected_prediction_tokens": 2,
                    "future_completion_detail": 8
                }
            }
        }
    });
    let mut state = StreamState::default();
    let chunks = responses_event_to_openai_chunks(&event, &mut state);
    let finish_chunk = chunks
        .iter()
        .find(|chunk| chunk["choices"][0]["finish_reason"].is_string())
        .expect("finish chunk");

    assert_eq!(finish_chunk["usage"]["service_tier"], "priority");
    assert_eq!(
        finish_chunk["usage"]["prompt_tokens_details"]["cached_tokens"],
        3
    );
    assert_eq!(
        finish_chunk["usage"]["prompt_tokens_details"]["audio_tokens"],
        2
    );
    assert_eq!(
        finish_chunk["usage"]["prompt_tokens_details"]["future_prompt_detail"],
        4
    );
    assert_eq!(
        finish_chunk["usage"]["completion_tokens_details"]["reasoning_tokens"],
        1
    );
    assert_eq!(
        finish_chunk["usage"]["completion_tokens_details"]["audio_tokens"],
        5
    );
    assert_eq!(
        finish_chunk["usage"]["completion_tokens_details"]["accepted_prediction_tokens"],
        6
    );
    assert_eq!(
        finish_chunk["usage"]["completion_tokens_details"]["rejected_prediction_tokens"],
        2
    );
    assert_eq!(
        finish_chunk["usage"]["completion_tokens_details"]["future_completion_detail"],
        8
    );
}

#[test]
fn translate_sse_event_responses_to_anthropic_bridges_custom_tool_call_successfully() {
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
    let done_event = serde_json::json!({
        "type": "response.custom_tool_call_input.done",
        "output_index": 0,
        "item_id": "ctc_1",
        "input": "print('hi')"
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
    assert!(
        first_joined.contains("\"type\":\"message_start\""),
        "{first_joined}"
    );
    assert!(
        first_joined.contains("\"type\":\"tool_use\""),
        "{first_joined}"
    );
    assert!(
        first_joined.contains("\"name\":\"code_exec\""),
        "{first_joined}"
    );
    assert!(
        first_joined.contains("{\\\"input\\\":\\\"print('"),
        "{first_joined}"
    );
    assert!(!first_joined.contains("event: error"), "{first_joined}");

    let second = translate_sse_event(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &delta_event,
        &mut state,
    );
    let second_joined = second
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        second_joined.contains("input_json_delta"),
        "{second_joined}"
    );
    assert!(second_joined.contains("hi')"), "{second_joined}");

    let third = translate_sse_event(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &done_event,
        &mut state,
    );
    let third_joined = third
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(third_joined.contains("input_json_delta"), "{third_joined}");
    assert!(third_joined.contains("\\\"}"), "{third_joined}");

    let fourth = translate_sse_event(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &complete_event,
        &mut state,
    );
    let fourth_joined = fourth
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        fourth_joined.contains("\"stop_reason\":\"tool_use\""),
        "{fourth_joined}"
    );
    assert!(
        fourth_joined.contains("\"type\":\"message_stop\""),
        "{fourth_joined}"
    );
}

#[test]
fn translate_sse_event_anthropic_to_responses_decodes_bridged_custom_tool_use() {
    let mut state = StreamState::default();
    state.request_scoped_tool_bridge_context = Some(serde_json::json!({
        "compatibility_mode": "balanced",
        "entries": {
            "code_exec": {
                "source_kind": "custom_text",
                "transport_kind": "function_object_wrapper",
                "wrapper_field": "input",
                "expected_canonical_shape": "single_required_string"
            }
        }
    }));
    let joined = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-test" }
        }),
        &mut state,
    )
    .into_iter()
    .chain(translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "tool_use",
                "id": "call_custom",
                "name": "code_exec",
                "input": { "input": "print('hi')" }
            }
        }),
        &mut state,
    ))
    .chain(translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use" }
        }),
        &mut state,
    ))
    .chain(translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "message_stop"
        }),
        &mut state,
    ))
    .map(|b| String::from_utf8_lossy(&b).to_string())
    .collect::<Vec<_>>()
    .join("\n");

    assert!(joined.contains("\"type\":\"custom_tool_call\""), "{joined}");
    assert!(
        joined.contains("response.custom_tool_call_input.delta"),
        "{joined}"
    );
    assert!(
        joined.contains("response.custom_tool_call_input.done"),
        "{joined}"
    );
    assert!(joined.contains("\"name\":\"code_exec\""), "{joined}");
    assert!(!joined.contains("\"type\":\"function_call\""), "{joined}");
    assert!(!joined.contains("response.failed"), "{joined}");
}

#[test]
fn translate_sse_event_anthropic_to_responses_malformed_bridged_payload_falls_back_to_function() {
    let mut state = StreamState::default();
    state.request_scoped_tool_bridge_context = Some(serde_json::json!({
        "compatibility_mode": "balanced",
        "entries": {
            "code_exec": {
                "source_kind": "custom_text",
                "transport_kind": "function_object_wrapper",
                "wrapper_field": "input",
                "expected_canonical_shape": "single_required_string"
            }
        }
    }));
    let joined = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-test" }
        }),
        &mut state,
    )
    .into_iter()
    .chain(translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "tool_use",
                "id": "call_custom",
                "name": "code_exec",
                "input": { "output": "missing input" }
            }
        }),
        &mut state,
    ))
    .chain(translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use" }
        }),
        &mut state,
    ))
    .chain(translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "message_stop"
        }),
        &mut state,
    ))
    .map(|b| String::from_utf8_lossy(&b).to_string())
    .collect::<Vec<_>>()
    .join("\n");

    assert!(joined.contains("\"type\":\"function_call\""), "{joined}");
    assert!(
        joined.contains("response.function_call_arguments.delta"),
        "{joined}"
    );
    assert!(joined.contains("\"name\":\"code_exec\""), "{joined}");
    assert!(
        !joined.contains("response.custom_tool_call_input.delta"),
        "{joined}"
    );
    assert!(!joined.contains("response.failed"), "{joined}");
}

#[test]
fn translate_sse_event_responses_to_anthropic_preserves_reasoning_before_text_and_completion() {
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
    let events = translate_sse_event(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &reasoning_event,
        &mut state,
    )
    .into_iter()
    .chain(translate_sse_event(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &content_event,
        &mut state,
    ))
    .chain(translate_sse_event(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &complete_event,
        &mut state,
    ))
    .map(|bytes| parse_sse_json(&bytes))
    .collect::<Vec<_>>();

    assert_eq!(events[0]["type"], "message_start");
    assert_eq!(events[1]["type"], "content_block_start");
    assert_eq!(events[1]["content_block"]["type"], "thinking");
    assert_eq!(events[2]["type"], "content_block_delta");
    assert_eq!(events[2]["delta"]["type"], "thinking_delta");
    assert_eq!(events[2]["delta"]["thinking"], "think");
    assert_eq!(events[3]["type"], "content_block_stop");
    assert_eq!(events[4]["type"], "content_block_start");
    assert_eq!(events[4]["content_block"]["type"], "text");
    assert_eq!(events[5]["type"], "content_block_delta");
    assert_eq!(events[5]["delta"]["type"], "text_delta");
    assert_eq!(events[5]["delta"]["text"], "Hi");
    assert_eq!(events[6]["type"], "content_block_stop");
    assert_eq!(events[7]["type"], "message_delta");
    assert_eq!(events[7]["delta"]["stop_reason"], "end_turn");
    assert_eq!(events[8]["type"], "message_stop");
    assert!(events.iter().all(|event| event["type"] != "error"));
}

#[test]
fn responses_event_audio_and_transcript_events_fail_closed_instead_of_silent_drop() {
    for event in [
        serde_json::json!({
            "type": "response.audio.delta",
            "delta": "AAAA"
        }),
        serde_json::json!({
            "type": "response.audio.transcript.delta",
            "delta": "hello"
        }),
    ] {
        let mut state = StreamState::default();
        let chunks = responses_event_to_openai_chunks(&event, &mut state);

        assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
        assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
        assert_eq!(
            chunks[0]["error"]["code"],
            "unsupported_openai_responses_stream_event"
        );
    }
}
