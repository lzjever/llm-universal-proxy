use super::*;

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
