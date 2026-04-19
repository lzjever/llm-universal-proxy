use super::*;

fn parse_sse_events(bytes: Vec<Vec<u8>>) -> Vec<Value> {
    bytes
        .into_iter()
        .map(|bytes| parse_sse_json(&bytes))
        .collect()
}

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
fn openai_chunk_to_claude_sse_emits_unsigned_thinking_for_reasoning_content() {
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
    let events = parse_sse_events(out1.into_iter().chain(out2).collect());
    assert_eq!(events[0]["type"], "message_start");
    assert_eq!(events[1]["type"], "content_block_start");
    assert_eq!(events[1]["content_block"]["type"], "thinking");
    assert_eq!(events[1]["content_block"]["thinking"], "");
    assert_eq!(events[2]["type"], "content_block_delta");
    assert_eq!(events[2]["delta"]["type"], "thinking_delta");
    assert_eq!(events[2]["delta"]["thinking"], "think");
    assert_eq!(events[3]["type"], "content_block_stop");
    assert_eq!(events[4]["type"], "message_delta");
    assert_eq!(events[4]["delta"]["stop_reason"], "end_turn");
    assert_eq!(events[5]["type"], "message_stop");
    assert!(events.iter().all(|event| event["type"] != "error"));
}

#[test]
fn openai_chunk_to_claude_sse_preserves_reasoning_text_and_tool_block_order() {
    let reasoning_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "role": "assistant", "reasoning_content": "need tool" }, "finish_reason": null }]
    });
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "content": "Calling tool." }, "finish_reason": null }]
    });
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_weather",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "usage": { "prompt_tokens": 1, "completion_tokens": 4, "total_tokens": 5 },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
    });
    let mut state = StreamState::default();
    let events = parse_sse_events(
        openai_chunk_to_claude_sse(&reasoning_chunk, &mut state)
            .into_iter()
            .chain(openai_chunk_to_claude_sse(&text_chunk, &mut state))
            .chain(openai_chunk_to_claude_sse(&tool_chunk, &mut state))
            .chain(openai_chunk_to_claude_sse(&finish_chunk, &mut state))
            .collect(),
    );

    assert_eq!(events[0]["type"], "message_start");
    assert_eq!(events[1]["type"], "content_block_start");
    assert_eq!(events[1]["content_block"]["type"], "thinking");
    assert_eq!(events[2]["type"], "content_block_delta");
    assert_eq!(events[2]["delta"]["type"], "thinking_delta");
    assert_eq!(events[2]["delta"]["thinking"], "need tool");
    assert_eq!(events[3]["type"], "content_block_stop");
    assert_eq!(events[3]["index"], 0);
    assert_eq!(events[4]["type"], "content_block_start");
    assert_eq!(events[4]["content_block"]["type"], "text");
    assert_eq!(events[5]["type"], "content_block_delta");
    assert_eq!(events[5]["delta"]["type"], "text_delta");
    assert_eq!(events[5]["delta"]["text"], "Calling tool.");
    assert_eq!(events[6]["type"], "content_block_stop");
    assert_eq!(events[6]["index"], 1);
    assert_eq!(events[7]["type"], "content_block_start");
    assert_eq!(events[7]["content_block"]["type"], "tool_use");
    assert_eq!(events[7]["content_block"]["id"], "call_weather");
    assert_eq!(events[7]["content_block"]["name"], "get_weather");
    assert_eq!(events[8]["type"], "content_block_delta");
    assert_eq!(events[8]["delta"]["type"], "input_json_delta");
    assert_eq!(events[8]["delta"]["partial_json"], "{\"city\":\"Tokyo\"}");
    assert_eq!(events[9]["type"], "content_block_stop");
    assert_eq!(events[9]["index"], 2);
    assert_eq!(events[10]["type"], "message_delta");
    assert_eq!(events[10]["delta"]["stop_reason"], "tool_use");
    assert_eq!(events[11]["type"], "message_stop");
    assert!(events.iter().all(|event| event["type"] != "error"));
}

#[test]
fn openai_chunk_to_claude_sse_continues_after_reasoning_into_text_and_finish() {
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
    let events = parse_sse_events(
        openai_chunk_to_claude_sse(&reasoning_chunk, &mut state)
            .into_iter()
            .chain(openai_chunk_to_claude_sse(&content_chunk, &mut state))
            .chain(openai_chunk_to_claude_sse(&finish_chunk, &mut state))
            .collect(),
    );

    assert_eq!(events[0]["type"], "message_start");
    assert_eq!(events[1]["type"], "content_block_start");
    assert_eq!(events[1]["content_block"]["type"], "thinking");
    assert_eq!(events[2]["type"], "content_block_delta");
    assert_eq!(events[2]["delta"]["thinking"], "think");
    assert_eq!(events[3]["type"], "content_block_stop");
    assert_eq!(events[4]["type"], "content_block_start");
    assert_eq!(events[4]["content_block"]["type"], "text");
    assert_eq!(events[5]["type"], "content_block_delta");
    assert_eq!(events[5]["delta"]["text"], "Hi");
    assert_eq!(events[6]["type"], "content_block_stop");
    assert_eq!(events[7]["type"], "message_delta");
    assert_eq!(events[8]["type"], "message_stop");
    assert!(events.iter().all(|event| event["type"] != "error"));
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
