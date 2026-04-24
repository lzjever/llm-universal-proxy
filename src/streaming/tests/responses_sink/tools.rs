use super::*;

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
fn openai_chunk_to_responses_sse_decodes_request_scoped_custom_bridge_without_prefix() {
    let mut state = StreamState {
        request_scoped_tool_bridge_context: Some(typed_tool_bridge_context(
            "code_exec",
            "custom_text",
            "balanced",
        )),
        ..Default::default()
    };
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_custom",
                    "function": {
                        "name": "code_exec",
                        "arguments": "{\"input\":\"print('hi')\"}"
                    }
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

    assert!(joined.contains("response.custom_tool_call_input.delta"));
    assert!(joined.contains("response.custom_tool_call_input.done"));
    assert!(joined.contains("\"type\":\"custom_tool_call\""));
    assert!(joined.contains("\"name\":\"code_exec\""));
    assert!(!joined.contains("\"type\":\"function_call\""));
}

#[test]
fn openai_chunk_to_responses_sse_rejects_reserved_prefix_function_names_without_bridge_context() {
    let mut state = StreamState::default();
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_custom",
                    "function": {
                        "name": "__llmup_custom__code_exec",
                        "arguments": "{\"input\":\"print('hi')\"}"
                    }
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

    assert!(joined.contains("\"type\":\"response.failed\""));
    assert!(joined.contains("\"type\":\"invalid_request_error\""));
    assert!(joined.contains("\"code\":\"reserved_openai_custom_bridge_prefix\""));
    assert!(!joined.contains("\"name\":\"__llmup_custom__code_exec\""));
    assert!(!joined.contains("__llmup_custom__"), "{joined}");
    assert!(!joined.contains("response.function_call_arguments.delta"));
    assert!(!joined.contains("response.custom_tool_call_input.delta"));
    assert!(
        state.fatal_rejection.is_some(),
        "reserved-prefix stream should be rejected"
    );
}

#[test]
fn openai_chunk_to_responses_sse_fails_closed_for_incomplete_or_invalid_tool_bridge_contexts() {
    let entry = serde_json::json!({
        "stable_name": "code_exec",
        "source_kind": "custom_text",
        "transport_kind": "function_object_wrapper",
        "wrapper_field": "input",
        "expected_canonical_shape": "single_required_string"
    });
    let contexts = [
        (
            "missing version",
            serde_json::json!({
                "compatibility_mode": "balanced",
                "entries": { "code_exec": entry.clone() }
            }),
        ),
        (
            "missing compatibility_mode",
            serde_json::json!({
                "version": 1,
                "entries": { "code_exec": entry.clone() }
            }),
        ),
        (
            "missing stable_name",
            serde_json::json!({
                "version": 1,
                "compatibility_mode": "balanced",
                "entries": {
                    "code_exec": {
                        "source_kind": "custom_text",
                        "transport_kind": "function_object_wrapper",
                        "wrapper_field": "input",
                        "expected_canonical_shape": "single_required_string"
                    }
                }
            }),
        ),
        (
            "non-integer version",
            serde_json::json!({
                "version": "1",
                "compatibility_mode": "balanced",
                "entries": { "code_exec": entry.clone() }
            }),
        ),
        (
            "future version",
            serde_json::json!({
                "version": 2,
                "compatibility_mode": "balanced",
                "entries": { "code_exec": entry }
            }),
        ),
    ];

    for (label, bridge_context) in contexts {
        let mut state = StreamState {
            request_scoped_tool_bridge_context: Some(bridge_context),
            ..Default::default()
        };
        let tool_chunk = serde_json::json!({
            "id": "chatcmpl-msg123",
            "created": 123,
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_custom",
                        "function": {
                            "name": "code_exec",
                            "arguments": "{\"input\":\"print('hi')\"}"
                        }
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

        assert!(
            joined.contains("response.function_call_arguments.delta"),
            "case = {label}, joined = {joined}"
        );
        assert!(
            joined.contains("\"type\":\"function_call\""),
            "case = {label}, joined = {joined}"
        );
        assert!(
            !joined.contains("response.custom_tool_call_input.delta"),
            "case = {label}, joined = {joined}"
        );
        assert!(
            !joined.contains("\"type\":\"custom_tool_call\""),
            "case = {label}, joined = {joined}"
        );
    }
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
fn openai_chunk_to_responses_sse_closes_function_call_before_minimax_usage_terminal() {
    let mut state = StreamState::default();
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
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
        "model": "MiniMax-M2.7-highspeed",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
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

    let _ = openai_chunk_to_responses_sse(&tool_chunk, &mut state);
    let finish_out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let usage_out = openai_chunk_to_responses_sse(&usage_chunk, &mut state);
    let finish_types = finish_out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .map(|event| event["type"].as_str().expect("event type").to_string())
        .collect::<Vec<_>>();
    let usage_types = usage_out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .map(|event| event["type"].as_str().expect("event type").to_string())
        .collect::<Vec<_>>();

    assert!(
        finish_types.contains(&"response.function_call_arguments.done".to_string()),
        "finish types = {finish_types:?}"
    );
    assert!(
        finish_types.contains(&"response.output_item.done".to_string()),
        "finish types = {finish_types:?}"
    );
    assert!(
        !finish_types.contains(&"response.completed".to_string()),
        "finish types = {finish_types:?}"
    );
    assert!(
        usage_types.contains(&"response.completed".to_string()),
        "usage types = {usage_types:?}"
    );
    assert!(
        !usage_types.contains(&"response.function_call_arguments.done".to_string()),
        "usage types = {usage_types:?}"
    );
}

#[test]
fn openai_chunk_to_responses_sse_closes_custom_tool_call_before_minimax_usage_terminal() {
    let mut state = StreamState::default();
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
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
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
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

    let _ = openai_chunk_to_responses_sse(&tool_chunk, &mut state);
    let finish_out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let usage_out = openai_chunk_to_responses_sse(&usage_chunk, &mut state);
    let finish_types = finish_out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .map(|event| event["type"].as_str().expect("event type").to_string())
        .collect::<Vec<_>>();
    let usage_types = usage_out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .map(|event| event["type"].as_str().expect("event type").to_string())
        .collect::<Vec<_>>();

    assert!(
        finish_types.contains(&"response.custom_tool_call_input.done".to_string()),
        "finish types = {finish_types:?}"
    );
    assert!(
        finish_types.contains(&"response.output_item.done".to_string()),
        "finish types = {finish_types:?}"
    );
    assert!(
        !finish_types.contains(&"response.completed".to_string()),
        "finish types = {finish_types:?}"
    );
    assert!(
        usage_types.contains(&"response.completed".to_string()),
        "usage types = {usage_types:?}"
    );
    assert!(
        !usage_types.contains(&"response.custom_tool_call_input.done".to_string()),
        "usage types = {usage_types:?}"
    );
}
